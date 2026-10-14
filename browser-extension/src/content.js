/*
 * KeyRoam extension: content script, in every frame of http(s) pages.
 *
 * - Reports whether the frame shows a login form (when the popup asks).
 * - Fills a login the user picked in the popup, after checking that the frame
 *   is still on the origin the background script checked with KeyRoam.
 * - Notices submitted logins and lets the background script decide whether
 *   to offer "save" or "update". The prompt is drawn in a closed shadow root
 *   and only reacts to real (trusted) clicks.
 */
(() => {
  'use strict';
  if (globalThis.__keyroamContentLoaded) return;
  globalThis.__keyroamContentLoaded = true;

  const ext = globalThis.browser || globalThis.chrome;
  const Forms = globalThis.KeyRoamForms;
  const isTop = window === window.top;

  // ----- fields -----

  function isVisible(el) {
    if (el.disabled || el.type === 'hidden') return false;
    const r = el.getBoundingClientRect();
    if (r.width < 2 || r.height < 2) return false;
    const s = getComputedStyle(el);
    return s.visibility !== 'hidden' && s.display !== 'none' && Number(s.opacity) > 0.05;
  }

  function inputsIn(root) {
    return Array.from((root || document).querySelectorAll('input'));
  }

  function describe(el) {
    return {
      type: (el.getAttribute('type') || 'text').toLowerCase(),
      name: el.name || '',
      id: el.id || '',
      autocomplete: el.getAttribute('autocomplete') || '',
      visible: isVisible(el),
      value: el.value || '',
    };
  }

  /** The inputs of the form around the first visible password field. */
  function loginScope() {
    const pw = inputsIn(document).find((el) => el.type === 'password' && isVisible(el));
    if (!pw) return null;
    return inputsIn(pw.form || document);
  }

  function setValue(el, value) {
    el.focus({ preventScroll: true });
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
  }

  function fill(msg) {
    if (location.origin !== msg.expectedOrigin) return { filled: false };
    const inputs = loginScope();
    if (!inputs) return { filled: false };
    const found = Forms.findLoginFields(inputs.map(describe));
    if (found.password < 0) return { filled: false };
    if (found.username >= 0 && msg.username) setValue(inputs[found.username], msg.username);
    setValue(inputs[found.password], msg.password);
    return { filled: true };
  }

  // ----- submissions -----

  let lastSent = { key: '', at: 0 };

  function capture(root) {
    const sub = Forms.extractSubmission(inputsIn(root).map(describe));
    if (!sub) return;
    const key = `${sub.username}\n${sub.password}`;
    const now = Date.now();
    if (key === lastSent.key && now - lastSent.at < 3000) return;
    lastSent = { key, at: now };
    ext.runtime.sendMessage({ kind: 'submitted', username: sub.username, password: sub.password }).catch(() => {});
  }

  const SUBMIT_TEXT = /log\s*in|sign\s*in|sign\s*up|register|continue|next|submit|save|anmelden|connexion/i;

  function isSubmitControl(el) {
    if (!el) return false;
    if (el.matches('input[type=submit], input[type=image]')) return true;
    if (el.matches('button')) return (el.getAttribute('type') || 'submit').toLowerCase() === 'submit' || SUBMIT_TEXT.test(el.textContent || '');
    if (el.matches('[role=button], a')) return SUBMIT_TEXT.test(el.textContent || '');
    return false;
  }

  document.addEventListener(
    'submit',
    (e) => {
      if (e.target instanceof HTMLFormElement) capture(e.target);
    },
    true,
  );
  document.addEventListener(
    'click',
    (e) => {
      if (!e.isTrusted) return;
      const control = e.target instanceof Element ? e.target.closest('button, input, [role=button], a') : null;
      if (isSubmitControl(control)) capture(control.form || document);
    },
    true,
  );
  document.addEventListener(
    'keydown',
    (e) => {
      if (e.isTrusted && e.key === 'Enter' && e.target instanceof HTMLInputElement) capture(e.target.form || document);
    },
    true,
  );

  // ----- save prompt (top frame only) -----

  let promptHost = null;

  function removePrompt() {
    if (promptHost) promptHost.remove();
    promptHost = null;
  }

  function el(tag, attrs, text) {
    const n = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) n.setAttribute(k, v);
    if (text) n.textContent = text;
    return n;
  }

  const PROMPT_CSS = `
    :host { all: initial; }
    .card { position: fixed; top: 16px; right: 16px; z-index: 2147483647; width: 320px; box-sizing: border-box;
      font: 14px/1.4 system-ui, -apple-system, "Segoe UI", sans-serif; color: #1b1d24; background: #fff;
      border: 1px solid #d6d9e3; border-radius: 12px; box-shadow: 0 8px 28px rgba(16, 24, 40, .18); padding: 14px 16px; }
    .title { font-weight: 600; margin: 0 0 4px; display: flex; align-items: center; gap: 8px; }
    .badge { width: 20px; height: 20px; border-radius: 6px; background: #3157d5; color: #fff; display: grid;
      place-items: center; font-size: 12px; font-weight: 700; }
    .detail { margin: 0 0 12px; color: #4a4f5c; overflow-wrap: anywhere; }
    .row { display: flex; gap: 8px; justify-content: flex-end; }
    button { font: inherit; border-radius: 8px; padding: 6px 14px; cursor: pointer; border: 1px solid #c4c8d4;
      background: #fff; color: #1b1d24; }
    button.primary { background: #3157d5; border-color: #3157d5; color: #fff; }
    button:disabled { opacity: .6; cursor: default; }
    .status { margin: 8px 0 0; color: #4a4f5c; }
    @media (prefers-color-scheme: dark) {
      .card { background: #1f2229; color: #eceef3; border-color: #3a3f4b; }
      .detail, .status { color: #b6bbc7; }
      button { background: #2a2e37; color: #eceef3; border-color: #4a4f5c; }
    }`;

  function showPrompt(p) {
    if (!isTop || !p) return;
    removePrompt();
    promptHost = document.createElement('keyroam-prompt');
    const shadow = promptHost.attachShadow({ mode: 'closed' });
    shadow.appendChild(el('style', {}, PROMPT_CSS));
    const card = el('div', { class: 'card', role: 'dialog', 'aria-label': 'KeyRoam' });
    const title = el('p', { class: 'title' });
    title.append(el('span', { class: 'badge', 'aria-hidden': 'true' }, 'K'));
    title.append(p.action === 'update' ? 'Update password in KeyRoam?' : 'Save login to KeyRoam?');
    const who = p.username ? `${p.username} on ${p.site}` : p.site;
    const detail = el('p', { class: 'detail' }, p.action === 'update' && p.title ? `${p.title} (${who})` : who);
    const row = el('div', { class: 'row' });
    const later = el('button', { type: 'button' }, 'Not now');
    const accept = el('button', { type: 'button', class: 'primary' }, p.action === 'update' ? 'Update' : 'Save');
    const status = el('p', { class: 'status', 'aria-live': 'polite' });
    row.append(later, accept);
    card.append(title, detail, row, status);
    shadow.appendChild(card);
    document.documentElement.appendChild(promptHost);

    const answer = async (e, yes) => {
      if (!e.isTrusted) return;
      later.disabled = accept.disabled = true;
      const r = await ext.runtime.sendMessage({ kind: 'prompt-answer', id: p.id, accept: yes }).catch(() => null);
      if (!yes) return removePrompt();
      if (r && r.ok) {
        status.textContent = r.ok.result === 'unchanged' ? 'Already saved in KeyRoam.' : 'Saved in KeyRoam.';
        setTimeout(removePrompt, 2000);
      } else {
        status.textContent = (r && r.error && r.error.message) || 'KeyRoam could not save this login.';
        later.disabled = accept.disabled = false;
      }
    };
    later.addEventListener('click', (e) => answer(e, false));
    accept.addEventListener('click', (e) => answer(e, true));
  }

  // ----- messages from the background script -----

  ext.runtime.onMessage.addListener((msg, sender, sendResponse) => {
    if (sender.id !== ext.runtime.id || !msg) return false;
    switch (msg.kind) {
      case 'probe': {
        const inputs = loginScope();
        sendResponse({ hasLogin: !!inputs && Forms.findLoginFields(inputs.map(describe)).password >= 0 });
        return false;
      }
      case 'fill':
        sendResponse(fill(msg));
        return false;
      case 'show-prompt':
        showPrompt(msg.prompt);
        sendResponse({ shown: isTop });
        return false;
    }
    return false;
  });

  if (isTop) {
    ext.runtime
      .sendMessage({ kind: 'page-ready' })
      .then((r) => r && r.ok && r.ok.prompt && showPrompt(r.ok.prompt))
      .catch(() => {});
  }
})();
