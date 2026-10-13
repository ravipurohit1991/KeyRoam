import { api } from '../lib/browser.js';
import { describeError } from '../lib/policy.js';

const $ = (id) => document.getElementById(id);
const consent = { allowHttp: false, allowCrossSite: false };

function el(tag, props = {}, ...children) {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
}

function button(text, onClick, primary = false) {
  const b = el('button', { type: 'button', textContent: text, className: primary ? 'primary' : '' });
  b.addEventListener('click', onClick);
  return b;
}

function send(message) {
  return api.runtime.sendMessage(message).catch((e) => ({ error: { code: 'failed', message: String(e) } }));
}

function showStatus(text, action = null) {
  const status = $('status');
  status.replaceChildren(el('div', { textContent: text }));
  if (action) status.append(action);
  status.hidden = false;
}

function consentButton(code) {
  if (code === 'insecure_page') {
    return button('Show logins for this insecure page', () => {
      consent.allowHttp = true;
      refresh();
    });
  }
  if (code === 'cross_site_frame') {
    return button('Use the embedded form anyway', () => {
      consent.allowCrossSite = true;
      refresh();
    });
  }
  return null;
}

async function fill(id, b) {
  b.disabled = true;
  const r = await send({ type: 'fill', id, ...consent });
  if (r?.error) {
    showStatus(describeError(r.error));
    b.disabled = false;
    return;
  }
  window.close();
}

function renderSave(save) {
  $('save-card').hidden = !save;
  if (!save) return;
  const update = save.action === 'update';
  $('save-question').textContent = update
    ? `Update the password of ${save.title ?? save.site}${save.username ? ` (${save.username})` : ''}?`
    : `Save the login ${save.username ? `${save.username} ` : ''}for ${save.site}?`;
  $('save-yes').textContent = update ? 'Update' : 'Save';
}

function render(state) {
  $('status').hidden = true;
  $('logins').replaceChildren();
  $('host').textContent = state.supported ? state.host : '';
  renderSave(state.save);
  if (!state.supported) {
    showStatus(describeError({ code: 'unsupported_page' }));
    return;
  }
  if (state.frameHost && state.frameHost !== state.host) {
    $('host').textContent = `${state.host} → ${state.frameHost}`;
  }
  if (state.error) {
    showStatus(describeError(state.error), consentButton(state.error.code));
    return;
  }
  if (state.logins.length === 0) {
    $('logins').append(el('li', { className: 'empty', textContent: `No saved logins for ${state.host}.` }));
  }
  for (const login of state.logins) {
    const fillButton = button('Fill', () => fill(login.id, fillButton), true);
    $('logins').append(
      el(
        'li',
        {},
        el(
          'div',
          { className: 'who' },
          el('strong', { textContent: login.title }),
          el('span', { textContent: login.username || 'No username' }),
        ),
        fillButton,
      ),
    );
  }
}

async function refresh() {
  render(await send({ type: 'popup_state', ...consent }));
}

async function answerSave(save) {
  await send({ type: 'save_answer', save });
  refresh();
}

$('save-yes').addEventListener('click', () => answerSave(true));
$('save-no').addEventListener('click', () => answerSave(false));

$('generate').addEventListener('click', async () => {
  const r = await send({ type: 'generate', length: 20 });
  if (r?.error) {
    showStatus(describeError(r.error));
    return;
  }
  $('generated').value = r.ok.password;
  $('generated-row').hidden = false;
});
$('copy-generated').addEventListener('click', async () => {
  await navigator.clipboard.writeText($('generated').value);
  $('copy-generated').textContent = 'Copied';
});
$('fill-generated').addEventListener('click', async () => {
  const r = await send({ type: 'fill_generated', password: $('generated').value });
  if (r?.error) showStatus(describeError(r.error));
  else window.close();
});

refresh();
