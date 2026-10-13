import assert from 'node:assert/strict';
import { test } from 'node:test';

import { Background, SAVE_TTL_MS } from '../src/lib/background-core.js';

const EXT = 'chrome-extension://ext-id/';
const TAB = { id: 5, url: 'https://login.example.com/signin' };

function fakeApi({ tab = TAB, fillResult = { filled: true } } = {}) {
  const session = new Map();
  const toTabs = [];
  const badges = [];
  return {
    runtime: { id: 'ext-id', getURL: (p) => `${EXT}${p}` },
    tabs: {
      query: async () => (tab ? [tab] : []),
      sendMessage: async (tabId, msg, opts) => {
        toTabs.push({ tabId, msg, frameId: opts?.frameId });
        return msg.type.startsWith('fill') ? fillResult : {};
      },
    },
    storage: {
      session: {
        get: async (k) => (session.has(k) ? { [k]: session.get(k) } : {}),
        set: async (o) => Object.entries(o).forEach(([k, v]) => session.set(k, v)),
        remove: async (k) => session.delete(k),
      },
    },
    action: { setBadgeText: async (o) => badges.push(o) },
    toTabs,
    badges,
    session,
  };
}

function fakeClient(answer) {
  const calls = [];
  return {
    calls,
    request: async (m) => {
      calls.push(structuredClone(m));
      return answer(m);
    },
  };
}

const app = (overrides = {}) =>
  fakeClient((m) => {
    if (overrides[m.type]) return overrides[m.type](m);
    switch (m.type) {
      case 'hello':
        return { ok: { app: 'KeyRoam', protocol: 1, unlocked: true } };
      case 'logins':
        return { ok: { origin: 'https://login.example.com', logins: [{ id: 'e1', title: 'Example', username: 'alice', match: 'exact' }] } };
      case 'credential':
        return { ok: { id: m.id, username: 'alice', password: 'synthetic-secret' } };
      case 'check_save':
        return { ok: { action: 'new', site: 'example.com' } };
      case 'save':
        return { ok: { id: 'new-id', updated: Boolean(m.update_id) } };
      default:
        return { error: { code: 'invalid_request', message: m.type } };
    }
  });

const content = (frameId = 0, url = TAB.url) => ({ id: 'ext-id', tab: TAB, frameId, url });
const popup = { id: 'ext-id', url: `${EXT}popup/popup.html` };
const saveBar = { id: 'ext-id', url: `${EXT}savebar/savebar.html`, tab: TAB, frameId: 9 };

test('messages are accepted only from the right senders', async () => {
  const client = app();
  const bg = new Background({ api: fakeApi(), client });
  const denied = [
    [{ type: 'popup_state' }, { id: 'other-extension', url: `${EXT}popup/popup.html` }],
    [{ type: 'fill', id: 'e1' }, content()],
    [{ type: 'save_answer', save: true }, content()],
    [{ type: 'generate' }, content()],
    [{ type: 'page_ready' }, popup],
    [{ type: 'submitted', password: 'x' }, saveBar],
    [{ type: 'popup_state' }, { id: 'ext-id', url: `${EXT}other.html` }],
    [{ type: 'forms' }, { id: 'ext-id', url: 'https://example.com/' }],
    [null, popup],
  ];
  for (const [msg, sender] of denied) {
    const r = await bg.onMessage(msg, sender);
    assert.equal(r.error?.code, 'invalid_request', JSON.stringify([msg, sender]));
  }
  assert.equal(client.calls.length, 0);
});

test('popup lists logins for the frame holding the form and fills that frame', async () => {
  const api = fakeApi();
  const client = app();
  const bg = new Background({ api, client });
  await bg.onMessage({ type: 'page_ready', kind: 'none' }, content(0));
  await bg.onMessage({ type: 'page_ready', kind: 'login' }, content(4, 'https://auth.example.com/frame'));

  const state = await bg.onMessage({ type: 'popup_state' }, popup);
  assert.equal(state.supported, true);
  assert.equal(state.host, 'login.example.com');
  assert.equal(state.frameHost, 'auth.example.com');
  assert.equal(state.logins[0].id, 'e1');
  const logins = client.calls.find((c) => c.type === 'logins');
  assert.deepEqual(logins.page, { url: 'https://auth.example.com/frame', top_url: TAB.url });

  const r = await bg.onMessage({ type: 'fill', id: 'e1' }, popup);
  assert.deepEqual(r, { ok: true });
  const sent = api.toTabs.find((t) => t.msg.type === 'fill');
  assert.equal(sent.frameId, 4);
  assert.equal(sent.msg.origin, 'https://auth.example.com');
  assert.equal(sent.msg.password, 'synthetic-secret');
});

test('consent is passed only when the user gave it', async () => {
  const client = app();
  const bg = new Background({ api: fakeApi({ tab: { id: 5, url: 'http://router.test/' } }), client });
  await bg.onMessage({ type: 'popup_state' }, popup);
  await bg.onMessage({ type: 'popup_state', allowHttp: true }, popup);
  await bg.onMessage({ type: 'fill', id: 'e1', allowHttp: 'yes' }, popup);
  const pages = client.calls.filter((c) => c.page).map((c) => c.page.allow_http ?? false);
  assert.deepEqual(pages, [false, true, false]);
});

test('nothing is filled when KeyRoam refuses or no form exists', async () => {
  const api = fakeApi();
  const bg = new Background({ api, client: app({ credential: () => ({ error: { code: 'locked', message: 'locked' } }) }) });
  assert.equal((await bg.onMessage({ type: 'fill', id: 'e1' }, popup)).error.code, 'locked');
  assert.equal(api.toTabs.length, 0);

  const noForm = fakeApi({ fillResult: { filled: false } });
  const bg2 = new Background({ api: noForm, client: app() });
  assert.equal((await bg2.onMessage({ type: 'fill', id: 'e1' }, popup)).error.code, 'no_form');

  const locked = new Background({ api: fakeApi(), client: app({ hello: () => ({ ok: { unlocked: false } }) }) });
  const st = await locked.onMessage({ type: 'popup_state' }, popup);
  assert.equal(st.error.code, 'locked');
  assert.deepEqual(st.logins, []);

  const unsupported = new Background({ api: fakeApi({ tab: { id: 1, url: 'chrome://settings' } }), client: app() });
  assert.deepEqual(await unsupported.onMessage({ type: 'popup_state' }, popup), { supported: false });
});

test('save prompt after submission: stored without exposure, answered from KeyRoam pages', async () => {
  let now = 1_000;
  const api = fakeApi();
  const client = app();
  const bg = new Background({ api, client, now: () => now });
  await bg.onMessage({ type: 'submitted', username: ' bob ', password: 'synthetic-bob' }, content(0));
  const check = client.calls.find((c) => c.type === 'check_save');
  assert.equal(check.username, 'bob');
  assert.deepEqual(api.badges.at(-1), { tabId: 5, text: '1' });
  assert.ok(api.toTabs.some((t) => t.msg.type === 'show_save_bar' && t.frameId === 0));

  // The next page asks whether to show the bar.
  assert.deepEqual(await bg.onMessage({ type: 'page_ready', kind: 'none' }, content(0)), { saveBar: true });
  assert.deepEqual(await bg.onMessage({ type: 'page_ready', kind: 'none' }, content(2)), { saveBar: false });

  const info = await bg.onMessage({ type: 'save_info' }, saveBar);
  assert.deepEqual(info.save, { site: 'example.com', action: 'new', username: 'bob', title: null });
  const st = await bg.onMessage({ type: 'popup_state' }, popup);
  assert.ok(!JSON.stringify(st).includes('synthetic-bob'), 'the popup never receives the password');

  const r = await bg.onMessage({ type: 'save_answer', save: true }, saveBar);
  assert.deepEqual(r, { ok: { id: 'new-id', updated: false } });
  const save = client.calls.find((c) => c.type === 'save');
  assert.equal(save.password, 'synthetic-bob');
  assert.equal(save.update_id, undefined);
  assert.equal(api.session.size, 0);
  assert.deepEqual(api.badges.at(-1), { tabId: 5, text: '' });
  assert.ok(api.toTabs.some((t) => t.msg.type === 'hide_save_bar'));
  // Answering twice does nothing.
  assert.equal((await bg.onMessage({ type: 'save_answer', save: true }, saveBar)).error.code, 'not_found');
  now += 1;
});

test('update prompts carry the entry id; declined and expired prompts are dropped', async () => {
  let now = 0;
  const api = fakeApi();
  const client = app({ check_save: () => ({ ok: { action: 'update', id: 'e1', title: 'Example', site: 'example.com' } }) });
  const bg = new Background({ api, client, now: () => now });
  await bg.onMessage({ type: 'submitted', username: 'alice', password: 'rotated' }, content(0));
  await bg.onMessage({ type: 'save_answer', save: true }, popup);
  assert.equal(client.calls.find((c) => c.type === 'save').update_id, 'e1');

  await bg.onMessage({ type: 'submitted', username: 'alice', password: 'rotated-2' }, content(0));
  assert.deepEqual(await bg.onMessage({ type: 'save_answer', save: false }, saveBar), { ok: true });
  assert.equal(client.calls.filter((c) => c.type === 'save').length, 1);

  await bg.onMessage({ type: 'submitted', username: 'alice', password: 'rotated-3' }, content(0));
  now += SAVE_TTL_MS + 1;
  assert.equal((await bg.onMessage({ type: 'save_info' }, saveBar)).save, null);
  assert.equal(api.session.size, 0);
});

test('already-saved logins and non-web pages never prompt', async () => {
  const api = fakeApi();
  const client = app({ check_save: () => ({ ok: { action: 'none', id: 'e1', site: 'example.com' } }) });
  const bg = new Background({ api, client });
  await bg.onMessage({ type: 'submitted', username: 'alice', password: 'same' }, content(0));
  await bg.onMessage({ type: 'submitted', username: 'alice', password: '' }, content(0));
  await bg.onMessage({ type: 'submitted', username: 'alice', password: 'x' }, { ...content(0), url: 'about:blank' });
  assert.equal(api.session.size, 0);
  assert.equal(client.calls.length, 1);
});

test('change-password forms reuse the username filled on that site', async () => {
  const client = app();
  const bg = new Background({ api: fakeApi(), client });
  await bg.onMessage({ type: 'fill', id: 'e1' }, popup);
  await bg.onMessage({ type: 'submitted', username: '', password: 'new-pw' }, content(0));
  assert.equal(client.calls.find((c) => c.type === 'check_save').username, 'alice');
  // Not on another site.
  await bg.onMessage({ type: 'submitted', username: '', password: 'new-pw' }, content(0, 'https://other.test/'));
  assert.equal(client.calls.filter((c) => c.type === 'check_save')[1].username, '');
});

test('keyboard shortcut fills only an unambiguous login', async () => {
  const api = fakeApi();
  const bg = new Background({ api, client: app() });
  assert.deepEqual(await bg.fillShortcut(), { ok: true });
  const two = new Background({
    api: fakeApi(),
    client: app({
      logins: () => ({ ok: { logins: [{ id: 'a' }, { id: 'b' }] } }),
    }),
  });
  assert.deepEqual(await two.fillShortcut(), { ok: false });
});

test('frames are forgotten when the tab loads a new page, in any report order', async () => {
  const bg = new Background({ api: fakeApi(), client: app() });
  // The iframe's script may run before the top frame's.
  await bg.onMessage({ type: 'page_ready', kind: 'login' }, content(4, 'https://auth.example.com/frame'));
  await bg.onMessage({ type: 'page_ready', kind: 'none' }, content(0));
  assert.equal(bg.frames.get(5).size, 2);
  bg.onTabUpdated(5, { title: 'x' });
  assert.equal(bg.frames.get(5).size, 2);
  bg.onTabUpdated(5, { status: 'loading', url: 'https://example.com/next' });
  assert.equal(bg.frames.has(5), false);
});

test('closing a tab forgets its frames and pending prompt', async () => {
  const api = fakeApi();
  const bg = new Background({ api, client: app() });
  await bg.onMessage({ type: 'submitted', username: 'bob', password: 'p' }, content(0));
  assert.equal(api.session.size, 1);
  await bg.onTabRemoved(5);
  assert.equal(api.session.size, 0);
  assert.equal(bg.frames.has(5), false);
});
