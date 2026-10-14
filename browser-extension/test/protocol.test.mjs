import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { test } from 'node:test';

const require = createRequire(import.meta.url);
const P = require('../src/lib/protocol.js');

test('requests carry version, id and type', () => {
  const a = P.request('list', { url: 'https://example.com/', top_url: 'https://example.com/' });
  const b = P.request('status');
  assert.equal(a.v, 1);
  assert.equal(a.type, 'list');
  assert.equal(a.url, 'https://example.com/');
  assert.equal(typeof a.id, 'string');
  assert.notEqual(a.id, b.id);
  assert.throws(() => P.request('export_vault'), { code: 'invalid_request' });
});

test('responses are unwrapped or turned into errors', () => {
  const req = P.request('status');
  assert.deepEqual(P.parseResponse(req, { v: 1, id: req.id, ok: { state: 'unlocked' } }), { state: 'unlocked' });
  assert.throws(() => P.parseResponse(req, { v: 1, id: req.id, error: { code: 'locked', message: 'locked' } }), {
    code: 'locked',
  });
  // An answer for another request, or garbage, is refused.
  assert.throws(() => P.parseResponse(req, { v: 1, id: 'other', ok: {} }), { code: 'internal' });
  assert.throws(() => P.parseResponse(req, undefined), { code: 'internal' });
  assert.throws(() => P.parseResponse(req, { v: 1, id: req.id }), { code: 'internal' });
});

test('native messaging failures are classified', () => {
  assert.equal(P.nativeFailure(new Error('Specified native messaging host not found.')).code, 'host_missing');
  assert.equal(P.nativeFailure(new Error('No such native application io.github.x')).code, 'host_missing');
  assert.equal(P.nativeFailure(new Error('Native host has exited.')).code, 'app_not_running');
});

test('errors are described for people', () => {
  assert.match(P.describe(new P.KeyRoamError('locked', 'x')), /locked/i);
  assert.match(P.describe(new P.KeyRoamError('host_missing', 'x')), /Set up browsers/);
  assert.equal(P.describe(new P.KeyRoamError('weird', 'raw message')), 'raw message');
});

test('origins of web URLs only', () => {
  assert.equal(P.originOf('https://Example.com:443/a?b#c'), 'https://example.com');
  assert.equal(P.originOf('http://localhost:3000/x'), 'http://localhost:3000');
  assert.equal(P.originOf('chrome://settings'), null);
  assert.equal(P.originOf('file:///etc/passwd'), null);
  assert.equal(P.originOf('not a url'), null);
});
