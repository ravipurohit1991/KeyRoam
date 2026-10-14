// Background logic, written against an injected WebExtension `api` and a
// NativeClient so it can be tested without a browser.
//
// Trust boundaries:
// * Page URLs come from the browser (`sender.url`, `tab.url`), never from
//   content-script message bodies.
// * Credentials are requested only after the user picks a login in the popup
//   (or presses the fill shortcut) and are sent only to the frame they were
//   released for; the content script re-checks that frame's origin.
// * Save prompts are answered only from KeyRoam's own extension pages.

import { chooseFrame, displayHost, isWebUrl, originOf, pageInfo } from './policy.js';

export const HOST_NAME = 'io.github.ravipurohit1991.keyroam';
export const SAVE_TTL_MS = 3 * 60 * 1000;
/** How long frames get to answer `report_forms`. */
export const REDISCOVER_MS = 150;

const SAVE_KEY = (tabId) => `pending-save:${tabId}`;
const NO_FORM = { error: { code: 'no_form', message: 'no login form on this page' } };

function invalid(message = 'not allowed') {
  return { error: { code: 'invalid_request', message } };
}

export class Background {
  constructor({ api, client, now = () => Date.now(), rediscoverMs = REDISCOVER_MS }) {
    this.api = api;
    this.client = client;
    this.now = now;
    this.rediscoverMs = rediscoverMs;
    /** tabId → Map(frameId → {url, kind}) reported by content scripts. */
    this.frames = new Map();
    /** tabId → {origin, username} of the last fill, for change-password forms. */
    this.lastFill = new Map();
  }

  // ----- routing -----

  async onMessage(msg, sender) {
    if (!msg || typeof msg.type !== 'string' || sender?.id !== this.api.runtime.id) return invalid();
    const extensionBase = this.api.runtime.getURL('');
    const fromExtensionPage = typeof sender.url === 'string' && sender.url.startsWith(extensionBase);
    if (!fromExtensionPage) {
      if (!sender.tab) return invalid();
      return this.#fromContent(msg, sender);
    }
    if (sender.url.startsWith(this.api.runtime.getURL('savebar/'))) {
      if (!sender.tab) return invalid();
      return this.#fromSaveBar(msg, sender.tab.id);
    }
    if (sender.url.startsWith(this.api.runtime.getURL('popup/'))) {
      return this.#fromPopup(msg);
    }
    return invalid();
  }

  async #fromContent(msg, sender) {
    const tabId = sender.tab.id;
    const frameId = sender.frameId ?? 0;
    switch (msg.type) {
      case 'page_ready':
        this.#recordFrame(tabId, frameId, sender.url, msg.kind);
        return { saveBar: frameId === 0 && (await this.pendingSave(tabId)) !== null };
      case 'forms':
        this.#recordFrame(tabId, frameId, sender.url, msg.kind);
        return {};
      case 'submitted':
        return this.onSubmitted(tabId, sender.url, sender.tab.url, msg);
      default:
        return invalid();
    }
  }

  async #fromSaveBar(msg, tabId) {
    switch (msg.type) {
      case 'save_info':
        return { save: summary(await this.pendingSave(tabId)) };
      case 'save_answer':
        return this.answerSave(tabId, msg.save === true);
      default:
        return invalid();
    }
  }

  async #fromPopup(msg) {
    switch (msg.type) {
      case 'popup_state':
        return this.popupState(consentOf(msg));
      case 'fill':
        return this.fill(String(msg.id ?? ''), consentOf(msg));
      case 'generate':
        return this.client.request({ type: 'generate', length: msg.length });
      case 'fill_generated':
        return this.fillGenerated(String(msg.password ?? ''));
      case 'save_answer': {
        const tab = await this.activeTab();
        return tab ? this.answerSave(tab.id, msg.save === true) : invalid('no active tab');
      }
      default:
        return invalid();
    }
  }

  #recordFrame(tabId, frameId, url, kind) {
    if (!isWebUrl(url)) return;
    if (!this.frames.has(tabId)) this.frames.set(tabId, new Map());
    this.frames.get(tabId).set(frameId, { url, kind: typeof kind === 'string' ? kind : 'none' });
  }

  /** A new top-level document is loading: its frames will report again. */
  onTabUpdated(tabId, changeInfo) {
    if (changeInfo?.status === 'loading') this.frames.delete(tabId);
  }

  onTabRemoved(tabId) {
    this.frames.delete(tabId);
    this.lastFill.delete(tabId);
    return this.api.storage.session.remove(SAVE_KEY(tabId));
  }

  // ----- popup -----

  async activeTab() {
    const [tab] = await this.api.tabs.query({ active: true, currentWindow: true });
    return tab ?? null;
  }

  async #target(tab) {
    if (!this.frames.has(tab.id)) await this.#rediscover(tab.id);
    const frame = chooseFrame(this.frames.get(tab.id));
    return { frameId: frame.frameId, url: frame.url ?? tab.url };
  }

  /**
   * The frame map lives in memory, and the browser suspends an idle
   * background (Chromium's service worker, Firefox's event page). Ask every
   * frame of the tab to report its forms again; each answers with a `forms`
   * message, and the first reply resolves the call.
   */
  async #rediscover(tabId) {
    const reply = await this.api.tabs.sendMessage(tabId, { type: 'report_forms' }).catch(() => null);
    if (reply && this.rediscoverMs > 0) await new Promise((r) => setTimeout(r, this.rediscoverMs));
  }

  async popupState(consent = {}) {
    const tab = await this.activeTab();
    if (!tab || !isWebUrl(tab.url)) return { supported: false };
    const target = await this.#target(tab);
    const state = {
      supported: true,
      host: displayHost(tab.url),
      secure: tab.url.startsWith('https:') && target.url.startsWith('https:'),
      frameHost: target.frameId === 0 ? null : displayHost(target.url),
      unlocked: false,
      logins: [],
      error: null,
      save: summary(await this.pendingSave(tab.id)),
    };
    const hello = await this.client.request({ type: 'hello' });
    if (hello.error) return { ...state, error: hello.error };
    if (!hello.ok.unlocked) return { ...state, error: { code: 'locked', message: 'KeyRoam is locked' } };
    const res = await this.client.request({ type: 'logins', page: pageInfo(target.url, tab.url, consent) });
    if (res.error) return { ...state, unlocked: true, error: res.error };
    return { ...state, unlocked: true, logins: res.ok.logins };
  }

  async fill(id, consent = {}) {
    const tab = await this.activeTab();
    if (!tab || !isWebUrl(tab.url)) return { error: { code: 'unsupported_page', message: 'not a web page' } };
    const target = await this.#target(tab);
    const res = await this.client.request({
      type: 'credential',
      page: pageInfo(target.url, tab.url, consent),
      id,
    });
    if (res.error) return res;
    const origin = originOf(target.url);
    const filled = await this.api.tabs
      .sendMessage(
        tab.id,
        { type: 'fill', origin, username: res.ok.username, password: res.ok.password },
        { frameId: target.frameId },
      )
      .catch(() => null);
    if (!filled?.filled) return NO_FORM;
    this.lastFill.set(tab.id, { origin, username: res.ok.username });
    return { ok: true };
  }

  async fillGenerated(password) {
    const tab = await this.activeTab();
    if (!tab || !isWebUrl(tab.url) || !password) return invalid();
    const target = await this.#target(tab);
    const filled = await this.api.tabs
      .sendMessage(
        tab.id,
        { type: 'fill_new_password', origin: originOf(target.url), password },
        { frameId: target.frameId },
      )
      .catch(() => null);
    return filled?.filled ? { ok: true } : NO_FORM;
  }

  /** Keyboard shortcut: fill when exactly one login matches. */
  async fillShortcut() {
    const state = await this.popupState();
    if (state.logins?.length === 1) return this.fill(state.logins[0].id);
    return { ok: false };
  }

  // ----- save prompts -----

  async onSubmitted(tabId, frameUrl, topUrl, msg) {
    const password = typeof msg.password === 'string' ? msg.password : '';
    if (!isWebUrl(frameUrl) || !isWebUrl(topUrl) || !password) return {};
    let username = typeof msg.username === 'string' ? msg.username.trim() : '';
    const origin = originOf(frameUrl);
    if (!username) {
      // A change-password form rarely shows the username; use the login
      // KeyRoam filled on this site earlier.
      const last = this.lastFill.get(tabId);
      if (last?.origin === origin) username = last.username;
    }
    const res = await this.client.request({
      type: 'check_save',
      page: pageInfo(frameUrl, topUrl),
      username,
      password,
    });
    if (res.error || res.ok.action === 'none') return {};
    await this.api.storage.session.set({
      [SAVE_KEY(tabId)]: {
        frameUrl,
        topUrl,
        username,
        password,
        action: res.ok.action,
        id: res.ok.id ?? null,
        title: res.ok.title ?? null,
        site: res.ok.site,
        expires: this.now() + SAVE_TTL_MS,
      },
    });
    await this.#badge(tabId, '1');
    // The page may be navigating away; its successor asks on `page_ready`.
    await this.api.tabs.sendMessage(tabId, { type: 'show_save_bar' }, { frameId: 0 }).catch(() => {});
    return {};
  }

  async pendingSave(tabId) {
    const key = SAVE_KEY(tabId);
    const p = (await this.api.storage.session.get(key))[key];
    if (!p) return null;
    if (p.expires <= this.now()) {
      await this.#clearSave(tabId);
      return null;
    }
    return p;
  }

  async answerSave(tabId, save) {
    const p = await this.pendingSave(tabId);
    await this.#clearSave(tabId);
    if (!p) return { error: { code: 'not_found', message: 'nothing to save' } };
    if (!save) return { ok: true };
    const res = await this.client.request({
      type: 'save',
      page: pageInfo(p.frameUrl, p.topUrl),
      username: p.username,
      password: p.password,
      ...(p.action === 'update' && p.id ? { update_id: p.id } : {}),
    });
    return res;
  }

  async #clearSave(tabId) {
    await this.api.storage.session.remove(SAVE_KEY(tabId));
    await this.#badge(tabId, '');
    await this.api.tabs.sendMessage(tabId, { type: 'hide_save_bar' }, { frameId: 0 }).catch(() => {});
  }

  async #badge(tabId, text) {
    try {
      await this.api.action.setBadgeText({ tabId, text });
    } catch {
      // The tab may be gone.
    }
  }
}

function consentOf(msg) {
  return { allowHttp: msg.allowHttp === true, allowCrossSite: msg.allowCrossSite === true };
}

/** What a save prompt may show: never the password. */
function summary(p) {
  if (!p) return null;
  return { site: p.site, action: p.action, username: p.username, title: p.title };
}
