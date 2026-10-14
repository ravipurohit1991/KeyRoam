/*
 * KeyRoam extension: toolbar popup. Lists the logins saved for the current
 * site (titles and usernames only) and fills the one the user picks.
 */
'use strict';

const ext = globalThis.browser || globalThis.chrome;
const P = globalThis.KeyRoamProtocol;
const main = document.getElementById('main');

async function ask(msg) {
  const r = await ext.runtime.sendMessage(msg);
  if (!r) throw new P.KeyRoamError('internal', 'No answer from the extension.');
  if (r.error) throw new P.KeyRoamError(r.error.code, r.error.message);
  return r.ok;
}

function el(tag, props, ...children) {
  const n = document.createElement(tag);
  Object.assign(n, props || {});
  for (const c of children) if (c != null) n.append(c);
  return n;
}

function showError(err) {
  main.replaceChildren(
    el('p', { className: 'muted error' }, err.code ? P.describe(err) : String(err.message || err)),
    el('button', { className: 'secondary', onclick: load }, 'Try again'),
  );
}

function consentWarning(state) {
  if (!state.needs_consent) return null;
  const text = !state.secure
    ? 'This page is not secure (HTTP). A filled password could be seen by others on the network.'
    : `The login form comes from ${state.origin}, embedded in ${state.top_origin}.`;
  const box = el('input', { type: 'checkbox', id: 'consent' });
  return el('div', { className: 'warning' }, text, el('label', { htmlFor: 'consent' }, box, 'Fill anyway'));
}

function render(state) {
  const items = state.logins.map((login) => {
    const button = el('button', { type: 'button' }, 'Fill');
    button.addEventListener('click', async () => {
      const consent = document.getElementById('consent');
      button.disabled = true;
      try {
        await ask({
          kind: 'fill',
          tabId: state.tabId,
          frameId: state.frameId,
          entryId: login.id,
          consent: !!(consent && consent.checked),
        });
        window.close();
      } catch (e) {
        button.disabled = false;
        if (e.code === 'consent_required') {
          const warning = document.querySelector('.warning');
          if (warning) warning.scrollIntoView();
        } else {
          showError(e);
        }
      }
    });
    return el(
      'li',
      {},
      el('div', { className: 'login' }, el('strong', {}, login.title), el('span', {}, login.username || 'No username')),
      button,
    );
  });

  const consent = consentWarning(state);
  main.replaceChildren(
    el('p', { className: 'site' }, state.site),
    consent,
    state.hasForm ? null : el('p', { className: 'muted' }, 'No login form found on this page.'),
    items.length ? el('ul', {}, ...items) : el('p', { className: 'muted' }, `No logins saved for ${state.site}.`),
  );

  if (consent) {
    const box = document.getElementById('consent');
    const sync = () => main.querySelectorAll('li button').forEach((b) => (b.disabled = !box.checked));
    box.addEventListener('change', sync);
    sync();
  }
}

async function load() {
  main.replaceChildren(el('p', { className: 'muted' }, 'Loading…'));
  try {
    render(await ask({ kind: 'popup-state' }));
  } catch (e) {
    showError(e);
  }
}

load();
