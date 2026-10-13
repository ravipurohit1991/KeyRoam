import { api } from '../lib/browser.js';

const $ = (id) => document.getElementById(id);

async function answer(save) {
  $('save').disabled = true;
  $('dismiss').disabled = true;
  await api.runtime.sendMessage({ type: 'save_answer', save });
}

async function init() {
  const { save } = (await api.runtime.sendMessage({ type: 'save_info' })) ?? {};
  if (!save) return;
  const update = save.action === 'update';
  $('question').textContent = update ? `Update password in KeyRoam?` : `Save login for ${save.site} in KeyRoam?`;
  $('detail').textContent = update
    ? `${save.title ?? save.site}${save.username ? ` — ${save.username}` : ''}`
    : save.username || 'No username';
  $('save').textContent = update ? 'Update' : 'Save';
  $('save').addEventListener('click', () => answer(true));
  $('dismiss').addEventListener('click', () => answer(false));
  $('bar').hidden = false;
}

init();
