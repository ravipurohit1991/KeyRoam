/*
 * KeyRoam extension: background script (Chromium service worker / Firefox
 * event page).
 *
 * It is the only part that talks to KeyRoam, through the native-messaging
 * host. Page URLs always come from the browser (tabs, webNavigation and
 * message senders), never from page content, and KeyRoam re-checks every
 * fill against the page's site.
 */
'use strict';

if (typeof importScripts === 'function') {
  importScripts('lib/protocol.js', 'lib/frames.js', 'lib/prompts.js');
}

const ext = globalThis.browser || globalThis.chrome;
const P = globalThis.KeyRoamProtocol;
const { chooseTarget } = globalThis.KeyRoamFrames;
const { PromptStore } = globalThis.KeyRoamPrompts;

// ----- KeyRoam -----

async function keyroam(type, fields) {
  const req = P.request(type, fields);
  let resp;
  try {
    resp = await ext.runtime.sendNativeMessage(P.HOST_NAME, req);
  } catch (e) {
    throw P.nativeFailure(e);
  }
  return P.parseResponse(req, resp);
}

// ----- pending save prompts (kept in session storage across worker restarts) -----

const prompts = new PromptStore();
const sessionStore = ext.storage && ext.storage.session;
const restored = (async () => {
  try {
    if (sessionStore) prompts.load((await sessionStore.get('prompts')).prompts);
  } catch (_) {
    // Session storage unavailable: prompts live in memory only.
  }
})();

async function persistPrompts() {
  try {
    if (sessionStore) await sessionStore.set({ prompts: prompts.toJSON() });
  } catch (_) {
    // Best effort.
  }
}

async function showPrompt(tabId, prompt) {
  try {
    await ext.tabs.sendMessage(tabId, { kind: 'show-prompt', prompt: PromptStore.view(prompt) }, { frameId: 0 });
  } catch (_) {
    // The page is navigating; it asks for the prompt once loaded.
  }
}

// ----- popup -----

async function activeTab() {
  const [tab] = await ext.tabs.query({ active: true, currentWindow: true });
  if (!tab || !P.originOf(tab.url)) throw new P.KeyRoamError('not_fillable', 'not a web page');
  return tab;
}

/** All frames of the tab, each marked with whether it shows a login form. */
async function probeFrames(tabId) {
  const frames = (await ext.webNavigation.getAllFrames({ tabId })) || [];
  return Promise.all(
    frames.map(async (f) => {
      try {
        const r = await ext.tabs.sendMessage(tabId, { kind: 'probe' }, { frameId: f.frameId });
        return Object.assign({}, f, { hasLogin: !!(r && r.hasLogin) });
      } catch (_) {
        return Object.assign({}, f, { hasLogin: false });
      }
    }),
  );
}

async function popupState() {
  const tab = await activeTab();
  const target = chooseTarget(await probeFrames(tab.id), tab.url);
  const list = await keyroam('list', { url: target.url, top_url: tab.url });
  return Object.assign({ tabId: tab.id, frameId: target.frameId, hasForm: target.hasLogin }, list);
}

async function fill(msg) {
  // Only the active tab can be filled, and only from the popup.
  const tab = await activeTab();
  if (tab.id !== msg.tabId) throw new P.KeyRoamError('not_fillable', 'the tab changed');
  const frame = await ext.webNavigation.getFrame({ tabId: tab.id, frameId: msg.frameId });
  if (!frame || !P.originOf(frame.url)) throw new P.KeyRoamError('not_fillable', 'the frame is gone');
  const creds = await keyroam('fill', {
    url: frame.url,
    top_url: tab.url,
    entry_id: String(msg.entryId),
    consent: msg.consent === true,
  });
  let result;
  try {
    result = await ext.tabs.sendMessage(
      tab.id,
      { kind: 'fill', username: creds.username, password: creds.password, expectedOrigin: P.originOf(frame.url) },
      { frameId: msg.frameId },
    );
  } catch (_) {
    result = null;
  }
  if (!result || !result.filled) throw new P.KeyRoamError('no_form', 'no login form');
  return { filled: true };
}

// ----- content scripts -----

async function submitted(msg, sender) {
  const tab = sender.tab;
  const frameUrl = sender.url;
  if (!tab || !P.originOf(frameUrl) || !P.originOf(tab.url)) return { prompt: null };
  const username = String(msg.username || '').slice(0, 512);
  const password = String(msg.password || '');
  if (!password || password.length > 4096) return { prompt: null };
  let check;
  try {
    check = await keyroam('check_save', { url: frameUrl, top_url: tab.url, username, password });
  } catch (e) {
    // While KeyRoam is locked, offer to save anyway; the answer re-checks.
    if (e.code !== 'locked') return { prompt: null };
    check = { action: 'save', site: new URL(frameUrl).hostname };
  }
  if (check.action !== 'save' && check.action !== 'update') return { prompt: null };
  const prompt = prompts.add({
    tabId: tab.id,
    frameUrl,
    topUrl: tab.url,
    username,
    password,
    action: check.action,
    entryId: check.entry_id || null,
    site: check.site,
    title: check.title || null,
  });
  await persistPrompts();
  await showPrompt(tab.id, prompt);
  return { prompt: PromptStore.view(prompt) };
}

async function pageReady(sender) {
  const p = sender.tab && sender.frameId === 0 ? prompts.forTab(sender.tab.id) : null;
  return { prompt: p ? PromptStore.view(p) : null };
}

async function answerPrompt(msg, sender) {
  if (!sender.tab || sender.frameId !== 0) throw new P.KeyRoamError('invalid_request', 'not from the page');
  const p = prompts.get(sender.tab.id, msg.id);
  if (!p) throw new P.KeyRoamError('not_found', 'This prompt has expired.');
  if (!msg.accept) {
    prompts.remove(sender.tab.id);
    await persistPrompts();
    return { result: 'dismissed' };
  }
  // Re-check: the vault may have changed, or was locked when we asked.
  const fields = { url: p.frameUrl, top_url: p.topUrl, username: p.username, password: p.password };
  const check = await keyroam('check_save', fields);
  let result = 'unchanged';
  if (check.action === 'save' || check.action === 'update') {
    const saved = await keyroam('save', Object.assign({}, fields, check.entry_id ? { entry_id: check.entry_id } : {}));
    result = saved.result;
  }
  prompts.remove(sender.tab.id);
  await persistPrompts();
  return { result };
}

// ----- message routing -----

function fromPopup(sender) {
  return sender.id === ext.runtime.id && !sender.tab && String(sender.url || '').startsWith(ext.runtime.getURL(''));
}

async function route(msg, sender) {
  await restored;
  if (sender.id !== ext.runtime.id || !msg || typeof msg.kind !== 'string') {
    throw new P.KeyRoamError('invalid_request', 'unknown sender');
  }
  switch (msg.kind) {
    case 'popup-state':
      if (!fromPopup(sender)) break;
      return popupState();
    case 'fill':
      if (!fromPopup(sender)) break;
      return fill(msg);
    case 'submitted':
      return submitted(msg, sender);
    case 'page-ready':
      return pageReady(sender);
    case 'prompt-answer':
      return answerPrompt(msg, sender);
  }
  throw new P.KeyRoamError('invalid_request', `unexpected message ${msg.kind}`);
}

ext.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  route(msg, sender).then(
    (ok) => sendResponse({ ok }),
    (e) => sendResponse({ error: { code: e.code || 'internal', message: P.describe(e) } }),
  );
  return true; // respond asynchronously
});

ext.tabs.onRemoved.addListener((tabId) => {
  prompts.remove(tabId);
  persistPrompts();
});
