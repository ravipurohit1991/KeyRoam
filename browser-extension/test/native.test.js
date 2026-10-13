import assert from 'node:assert/strict';
import { test } from 'node:test';

import { NativeClient } from '../src/lib/native.js';

class FakeEvent {
  constructor() {
    this.listeners = [];
  }
  addListener(f) {
    this.listeners.push(f);
  }
  fire(...args) {
    for (const f of this.listeners) f(...args);
  }
}

class FakePort {
  constructor() {
    this.sent = [];
    this.onMessage = new FakeEvent();
    this.onDisconnect = new FakeEvent();
    this.disconnected = false;
  }
  postMessage(m) {
    this.sent.push(m);
  }
  disconnect() {
    this.disconnected = true;
  }
}

/** Manual timers: run them with `fire()`. */
function fakeTimers() {
  const timers = new Map();
  let next = 1;
  return {
    setTimeout(f, ms) {
      timers.set(next, { f, ms });
      return next++;
    },
    clearTimeout(id) {
      timers.delete(id);
    },
    fire(ms) {
      for (const [id, t] of [...timers]) {
        if (t.ms === ms) {
          timers.delete(id);
          t.f();
        }
      }
    },
    count: () => timers.size,
  };
}

test('requests are matched to responses by rid, in any order', async () => {
  const ports = [];
  const timers = fakeTimers();
  const client = new NativeClient(() => ports[ports.push(new FakePort()) - 1], { timers });
  const a = client.request({ type: 'hello' });
  const b = client.request({ type: 'logins', page: {} });
  assert.equal(ports.length, 1, 'one port for both requests');
  const [m1, m2] = ports[0].sent;
  assert.equal(m1.type, 'hello');
  assert.notEqual(m1.rid, m2.rid);
  ports[0].onMessage.fire({ rid: m2.rid, ok: { logins: [] } });
  ports[0].onMessage.fire({ rid: 999, ok: 'stray' });
  ports[0].onMessage.fire({ rid: m1.rid, ok: { unlocked: true } });
  assert.deepEqual(await a, { ok: { unlocked: true } });
  assert.deepEqual(await b, { ok: { logins: [] } });
});

test('a missing host or a crash answers every pending request', async () => {
  let lastError;
  const port = new FakePort();
  const timers = fakeTimers();
  const client = new NativeClient(() => port, { timers, lastError: () => lastError });
  const pending = client.request({ type: 'hello' });
  lastError = 'Specified native messaging host not found.';
  port.onDisconnect.fire(port);
  assert.equal((await pending).error.code, 'host_not_found');
  assert.equal(timers.count(), 0);

  // The next request reconnects.
  const port2 = new FakePort();
  const client2 = new NativeClient(() => port2, { timers });
  const p2 = client2.request({ type: 'hello' });
  port2.onDisconnect.fire({ error: { message: 'Native host has exited.' } });
  assert.equal((await p2).error.code, 'host_exited');

  const throwing = new NativeClient(
    () => {
      throw new Error('Access to the specified native messaging host is forbidden.');
    },
    { timers },
  );
  assert.equal((await throwing.request({ type: 'hello' })).error.code, 'host_not_found');
});

test('timeouts and idle disconnect', async () => {
  const ports = [];
  const timers = fakeTimers();
  const client = new NativeClient(() => ports[ports.push(new FakePort()) - 1], { timers, timeoutMs: 100, idleMs: 500 });
  const slow = client.request({ type: 'hello' });
  timers.fire(100);
  assert.equal((await slow).error.code, 'timeout');

  const fast = client.request({ type: 'hello' });
  ports[0].onMessage.fire({ rid: ports[0].sent[1].rid, ok: {} });
  await fast;
  assert.equal(ports[0].disconnected, false);
  timers.fire(500);
  assert.equal(ports[0].disconnected, true, 'idle port is closed so the host can exit');
  client.request({ type: 'hello' });
  assert.equal(ports.length, 2, 'a new request reopens the port');
});
