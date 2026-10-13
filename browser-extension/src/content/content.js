// KeyRoam content script: finds login forms, fills a login the user picked in
// the popup, and offers to save logins after a form is submitted.
//
// It never asks for credentials by itself: the background script sends them
// only after a user action, together with the origin they were released for,
// and they are filled only if this frame is still on that origin.
(function () {
  'use strict';

  const api = globalThis.browser ?? globalThis.chrome;
  const Forms = globalThis.KeyRoamForms;
  const isTop = window === window.top;
  const SKIPPED_TYPES = new Set(['hidden', 'submit', 'button', 'checkbox', 'radio', 'file', 'image', 'reset', 'range', 'color']);

  function send(message) {
    try {
      return api.runtime.sendMessage(message).catch(() => null);
    } catch {
      // The extension was reloaded; this script is orphaned.
      return Promise.resolve(null);
    }
  }

  // ----- reading the page -----

  function isVisible(el) {
    const r = el.getBoundingClientRect();
    if (r.width < 2 || r.height < 2) return false;
    const s = getComputedStyle(el);
    return s.visibility !== 'hidden' && s.display !== 'none' && Number(s.opacity) > 0.05;
  }

  function describe(el) {
    return {
      type: (el.type || '').toLowerCase(),
      name: el.name || '',
      id: el.id || '',
      autocomplete: el.getAttribute('autocomplete') || '',
      placeholder: el.placeholder || '',
      ariaLabel: el.getAttribute('aria-label') || '',
      label: el.labels && el.labels.length ? el.labels[0].textContent || '' : '',
      visible: isVisible(el),
      disabled: el.disabled,
      readOnly: el.readOnly,
    };
  }

  /** Each <form>, plus the inputs outside any form, analysed. */
  function groups() {
    const byRoot = new Map();
    for (const el of document.querySelectorAll('input')) {
      if (SKIPPED_TYPES.has((el.type || '').toLowerCase())) continue;
      const root = el.form || document;
      if (!byRoot.has(root)) byRoot.set(root, []);
      byRoot.get(root).push(el);
    }
    return [...byRoot].map(([root, els]) => ({ root, els, analysis: Forms.analyzeForm(els.map(describe)) }));
  }

  /** The group to act on: the one holding the focused field first, then by kind. */
  function bestGroup(kinds) {
    const focused = document.activeElement;
    const candidates = groups().filter((g) => kinds.includes(g.analysis.kind));
    candidates.sort(
      (a, b) =>
        (a.els.includes(focused) ? 0 : 1) - (b.els.includes(focused) ? 0 : 1) ||
        Forms.RANK[a.analysis.kind] - Forms.RANK[b.analysis.kind],
    );
    return candidates[0] || null;
  }

  // ----- reporting forms to the background -----

  let reportedKind = null;

  function pageKind() {
    return Forms.bestKind(groups().map((g) => g.analysis));
  }

  send({ type: 'page_ready', kind: (reportedKind = pageKind()) }).then((r) => {
    if (isTop && r && r.saveBar) showSaveBar();
  });

  let scan = null;
  new MutationObserver(() => {
    clearTimeout(scan);
    scan = setTimeout(() => {
      const kind = pageKind();
      if (kind !== reportedKind) {
        reportedKind = kind;
        send({ type: 'forms', kind });
      }
    }, 500);
  }).observe(document.documentElement, { childList: true, subtree: true, attributes: true, attributeFilter: ['type', 'style', 'class', 'hidden'] });

  // ----- filling -----

  const valueSetter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;

  function setValue(el, value) {
    el.focus();
    // Use the native setter so frameworks that track the value notice it.
    valueSetter.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
  }

  function fillLogin(username, password) {
    const g = bestGroup(['login', 'username', 'change', 'signup']);
    if (!g) return false;
    const { analysis, els } = g;
    if (analysis.username !== null && username) setValue(els[analysis.username], username);
    if (analysis.current !== null && password) setValue(els[analysis.current], password);
    return true;
  }

  function fillNewPassword(password) {
    const g = bestGroup(['signup', 'change']);
    if (!g) return false;
    for (const i of g.analysis.newPasswords) setValue(g.els[i], password);
    return true;
  }

  api.runtime.onMessage.addListener((msg, sender, sendResponse) => {
    if (sender.id !== api.runtime.id || !msg) return false;
    let filled = false;
    switch (msg.type) {
      case 'fill':
        if (msg.origin === location.origin) filled = fillLogin(msg.username, msg.password);
        break;
      case 'fill_new_password':
        if (msg.origin === location.origin) filled = fillNewPassword(msg.password);
        break;
      case 'show_save_bar':
        if (isTop) showSaveBar();
        break;
      case 'hide_save_bar':
        if (isTop) hideSaveBar();
        break;
      default:
        return false;
    }
    sendResponse({ filled });
    return false;
  });

  // ----- save prompts -----

  let lastSent = { key: null, at: 0 };

  function capture(target) {
    const root = (target && target.form) || (target instanceof HTMLFormElement ? target : null) || document;
    const g = groups().find((x) => x.root === root);
    if (!g) return;
    const login = Forms.submittedLogin(
      g.analysis,
      g.els.map((el) => el.value),
    );
    if (!login) return;
    // One submission often fires a click, a keydown and a submit event.
    const key = `${login.username}\u0000${login.password}`;
    const now = Date.now();
    if (lastSent.key === key && now - lastSent.at < 3000) return;
    lastSent = { key, at: now };
    setTimeout(() => {
      if (lastSent.key === key) lastSent = { key: null, at: 0 };
    }, 3000);
    send({ type: 'submitted', username: login.username, password: login.password });
  }

  document.addEventListener(
    'submit',
    (e) => {
      if (e.isTrusted) capture(e.target);
    },
    true,
  );
  document.addEventListener(
    'click',
    (e) => {
      if (!e.isTrusted || !(e.target instanceof Element)) return;
      const button = e.target.closest('button, input[type=submit], input[type=button], [role=button]');
      if (button) capture(button);
    },
    true,
  );
  document.addEventListener(
    'keydown',
    (e) => {
      if (e.isTrusted && e.key === 'Enter' && e.target instanceof HTMLInputElement) capture(e.target);
    },
    true,
  );

  // The prompt is an extension page in an iframe, so the website can neither
  // read nor script it.
  let bar = null;

  function showSaveBar() {
    if (bar) return;
    const host = document.createElement('div');
    host.style.cssText = 'all: initial; position: fixed; top: 12px; right: 12px; z-index: 2147483647;';
    const shadow = host.attachShadow({ mode: 'closed' });
    const frame = document.createElement('iframe');
    frame.src = api.runtime.getURL('savebar/savebar.html');
    frame.title = 'KeyRoam';
    frame.style.cssText =
      'width: 380px; height: 112px; border: 0; border-radius: 12px; box-shadow: 0 6px 24px rgba(0,0,0,.28); display: block;';
    shadow.appendChild(frame);
    (document.body || document.documentElement).appendChild(host);
    bar = host;
  }

  function hideSaveBar() {
    if (bar) bar.remove();
    bar = null;
  }
})();
