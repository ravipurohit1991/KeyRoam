import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { test } from 'node:test';

const require = createRequire(import.meta.url);
const { chooseTarget } = require('../src/lib/frames.js');
const { PromptStore } = require('../src/lib/prompts.js');

const TOP = 'https://shop.example/checkout';

test('the top frame wins when it has a login form', () => {
  const frames = [
    { frameId: 0, url: TOP, hasLogin: true },
    { frameId: 3, url: 'https://accounts.example/embed', hasLogin: true },
  ];
  assert.deepEqual(chooseTarget(frames, TOP), { frameId: 0, url: TOP, hasLogin: true });
});

test('an embedded login form is used when the page has none', () => {
  const frames = [
    { frameId: 0, url: TOP, hasLogin: false },
    { frameId: 9, url: 'https://ads.example/', hasLogin: false },
    { frameId: 7, url: 'https://accounts.example/embed', hasLogin: true },
    { frameId: 5, url: 'about:blank', hasLogin: true },
  ];
  assert.deepEqual(chooseTarget(frames, TOP), { frameId: 7, url: 'https://accounts.example/embed', hasLogin: true });
});

test('without any form the top frame is listed', () => {
  assert.deepEqual(chooseTarget([], TOP), { frameId: 0, url: TOP, hasLogin: false });
  assert.deepEqual(chooseTarget([{ frameId: 0, url: TOP, hasLogin: false }], TOP), {
    frameId: 0,
    url: TOP,
    hasLogin: false,
  });
});

test('prompts are kept per tab and expire', () => {
  let now = 1000;
  const store = new PromptStore({ ttlMs: 60_000, now: () => now });
  const p = store.add({ tabId: 4, username: 'alice', password: 'synthetic-1', action: 'save', site: 'example.com' });
  assert.equal(store.forTab(4), p);
  assert.equal(store.get(4, p.id), p);
  assert.equal(store.get(4, 'other'), null);
  assert.equal(store.get(5, p.id), null);

  // A newer submission replaces the prompt for that tab.
  const q = store.add({ tabId: 4, username: 'bob', password: 'synthetic-2', action: 'save', site: 'example.com' });
  assert.notEqual(q.id, p.id);
  assert.equal(store.get(4, p.id), null);

  now += 61_000;
  assert.equal(store.forTab(4), null);
});

test('the page only sees the public view', () => {
  const store = new PromptStore();
  const p = store.add({ tabId: 1, username: 'alice', password: 'synthetic-secret', action: 'update', site: 's', title: 'T' });
  const view = PromptStore.view(p);
  assert.deepEqual(Object.keys(view).sort(), ['action', 'id', 'site', 'title', 'username']);
  assert.ok(!JSON.stringify(view).includes('synthetic-secret'));
});

test('prompts survive a background restart through serialisation', () => {
  const a = new PromptStore();
  const p = a.add({ tabId: 2, username: 'alice', password: 'synthetic-1', action: 'save', site: 's' });
  const b = new PromptStore();
  b.load(JSON.parse(JSON.stringify(a.toJSON())));
  assert.equal(b.get(2, p.id).password, 'synthetic-1');
  b.remove(2);
  assert.equal(b.forTab(2), null);
});
