import assert from 'node:assert/strict';
import { test } from 'node:test';

import { chooseFrame, describeError, displayHost, isSecureUrl, isWebUrl, originOf, pageInfo } from '../src/lib/policy.js';

test('web pages only', () => {
  assert.equal(isWebUrl('https://example.com/'), true);
  assert.equal(isWebUrl('http://example.com/'), true);
  for (const u of ['chrome://settings', 'about:blank', 'file:///etc/passwd', 'moz-extension://x/', 'nonsense']) {
    assert.equal(isWebUrl(u), false, u);
  }
  assert.equal(isSecureUrl('https://example.com'), true);
  assert.equal(isSecureUrl('http://example.com'), false);
  assert.equal(originOf('https://Login.Example.com:443/a?b'), 'https://login.example.com');
  assert.equal(displayHost('https://www.example.com/x'), 'example.com');
});

test('page info carries consent only when given', () => {
  assert.deepEqual(pageInfo('https://a.test/f', 'https://a.test/'), { url: 'https://a.test/f', top_url: 'https://a.test/' });
  assert.deepEqual(pageInfo('http://a.test/', 'http://a.test/', { allowHttp: true, allowCrossSite: false }), {
    url: 'http://a.test/',
    top_url: 'http://a.test/',
    allow_http: true,
  });
});

test('frame choice prefers the top frame, then frames with forms', () => {
  assert.deepEqual(chooseFrame(undefined), { frameId: 0, url: null, kind: 'none' });
  const frames = new Map([
    [0, { url: 'https://shop.test/', kind: 'none' }],
    [7, { url: 'https://login.shop.test/', kind: 'login' }],
    [3, { url: 'https://ads.test/', kind: 'none' }],
  ]);
  assert.equal(chooseFrame(frames).frameId, 7);
  frames.set(0, { url: 'https://shop.test/', kind: 'login' });
  assert.equal(chooseFrame(frames).frameId, 0);
  assert.equal(chooseFrame(new Map([[0, { url: 'https://x.test/', kind: 'none' }]])).url, 'https://x.test/');
});

test('errors have readable explanations', () => {
  assert.match(describeError({ code: 'locked' }), /Unlock KeyRoam/);
  assert.match(describeError({ code: 'app_not_running' }), /Open the KeyRoam app/);
  assert.equal(describeError({ code: 'weird', message: 'raw' }), 'raw');
  assert.equal(describeError(null), '');
});
