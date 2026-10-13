// Request/response over a native-messaging port.
//
// Every request gets a fresh `rid`; the KeyRoam app echoes it, so answers are
// matched to requests even if they arrive out of order. Requests always
// resolve (never reject) with either `{ok: …}` or `{error: {code, message}}`.

export function errorReply(code, message) {
  return { error: { code, message } };
}

export class NativeClient {
  /**
   * @param {() => object} connect opens a port (`runtime.connectNative`).
   * @param {object} options
   * @param {() => string | undefined} options.lastError the browser's last error
   *   message, read when the port disconnects (Chromium reports it there).
   */
  constructor(connect, { timeoutMs = 20000, idleMs = 30000, lastError = () => undefined, timers = globalThis } = {}) {
    this.connect = connect;
    this.timeoutMs = timeoutMs;
    this.idleMs = idleMs;
    this.lastError = lastError;
    this.timers = timers;
    this.port = null;
    this.seq = 0;
    this.pending = new Map();
    this.idleTimer = null;
  }

  request(message) {
    return new Promise((resolve) => {
      let port;
      try {
        port = this.#open();
      } catch (e) {
        resolve(errorReply('host_not_found', String(e?.message ?? e)));
        return;
      }
      this.#cancelIdle();
      const rid = ++this.seq;
      const timer = this.timers.setTimeout(() => {
        this.pending.delete(rid);
        resolve(errorReply('timeout', 'KeyRoam did not answer in time.'));
        this.#scheduleIdle();
      }, this.timeoutMs);
      this.pending.set(rid, { resolve, timer });
      try {
        port.postMessage({ ...message, rid });
      } catch (e) {
        this.timers.clearTimeout(timer);
        this.pending.delete(rid);
        this.port = null;
        resolve(errorReply('host_exited', String(e?.message ?? e)));
      }
    });
  }

  #open() {
    if (this.port) return this.port;
    const port = this.connect();
    port.onMessage.addListener((m) => this.#onMessage(m));
    port.onDisconnect.addListener((p) => this.#onDisconnect(port, p));
    this.port = port;
    return port;
  }

  #onMessage(m) {
    const entry = this.pending.get(m?.rid);
    if (!entry) return;
    this.pending.delete(m.rid);
    this.timers.clearTimeout(entry.timer);
    const { rid: _rid, ...reply } = m;
    entry.resolve(reply);
    this.#scheduleIdle();
  }

  #onDisconnect(port, p) {
    if (this.port !== port) return;
    this.port = null;
    this.#cancelIdle();
    const message = p?.error?.message ?? this.lastError() ?? 'The KeyRoam helper stopped.';
    const code = /not found|no such native application/i.test(message) ? 'host_not_found' : 'host_exited';
    for (const [rid, entry] of this.pending) {
      this.timers.clearTimeout(entry.timer);
      entry.resolve(errorReply(code, message));
      this.pending.delete(rid);
    }
  }

  // Close the port when idle so the helper process exits and the browser can
  // suspend the background script.
  #scheduleIdle() {
    if (this.pending.size > 0 || !this.port) return;
    this.#cancelIdle();
    this.idleTimer = this.timers.setTimeout(() => this.disconnect(), this.idleMs);
  }

  #cancelIdle() {
    if (this.idleTimer !== null) {
      this.timers.clearTimeout(this.idleTimer);
      this.idleTimer = null;
    }
  }

  disconnect() {
    this.#cancelIdle();
    const port = this.port;
    this.port = null;
    port?.disconnect();
  }
}
