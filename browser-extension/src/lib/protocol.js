/*
 * KeyRoam extension: the native-messaging protocol (see
 * crates/keyroam-autofill/src/protocol.rs). Shared by the background script
 * and the popup; also loaded by the Node tests.
 */
(function (root, factory) {
  const api = factory();
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.KeyRoamProtocol = api;
})(typeof self !== 'undefined' ? self : globalThis, function () {
  'use strict';

  /** Native-messaging host name registered by `keyroam-native-host install`. */
  const HOST_NAME = 'io.github.ravipurohit1991.keyroam';
  const PROTOCOL_VERSION = 1;
  const COMMANDS = ['status', 'list', 'fill', 'check_save', 'save'];

  class KeyRoamError extends Error {
    constructor(code, message) {
      super(message);
      this.name = 'KeyRoamError';
      this.code = code;
    }
  }

  let counter = 0;

  /** Build a request envelope for `type`. */
  function request(type, fields) {
    if (!COMMANDS.includes(type)) throw new KeyRoamError('invalid_request', `unknown command ${type}`);
    counter += 1;
    return Object.assign({ v: PROTOCOL_VERSION, id: `${Date.now().toString(36)}-${counter}`, type }, fields || {});
  }

  /** Unwrap a response to `req`, throwing a KeyRoamError for errors. */
  function parseResponse(req, resp) {
    if (!resp || typeof resp !== 'object') {
      throw new KeyRoamError('internal', 'KeyRoam did not answer.');
    }
    if (resp.id !== req.id) throw new KeyRoamError('internal', 'KeyRoam answered a different request.');
    if (resp.error) {
      throw new KeyRoamError(
        String(resp.error.code || 'internal'),
        String(resp.error.message || 'KeyRoam reported an error.'),
      );
    }
    if (!Object.prototype.hasOwnProperty.call(resp, 'ok')) {
      throw new KeyRoamError('internal', 'KeyRoam sent an unexpected answer.');
    }
    return resp.ok;
  }

  /** Classify a failure of runtime.sendNativeMessage itself. */
  function nativeFailure(err) {
    const msg = String((err && err.message) || err || '');
    if (/not found|native messaging host|no such native application|access to the specified native/i.test(msg)) {
      return new KeyRoamError('host_missing', msg);
    }
    return new KeyRoamError('app_not_running', msg);
  }

  const MESSAGES = {
    host_missing:
      'This browser is not connected to KeyRoam yet. In the KeyRoam app, open the menu, choose ' +
      '"Browser autofill…" and then "Set up browsers". Restart the browser afterwards.',
    app_not_running: 'Open the KeyRoam app to fill and save logins.',
    locked: 'KeyRoam is locked. Unlock the app, then try again.',
    no_vault: 'Set up KeyRoam in the app first.',
    not_fillable: 'KeyRoam does not fill on this kind of page.',
    unsupported_version: 'The KeyRoam app and this extension do not match. Update both.',
    no_match: 'This login is not saved for this site.',
    not_found: 'That login no longer exists.',
    no_form: 'No login form was found on this page.',
  };

  /** A sentence for the user. */
  function describe(err) {
    if (err && MESSAGES[err.code]) return MESSAGES[err.code];
    return (err && err.message) || 'Something went wrong.';
  }

  /** The origin of an http(s) URL, or null. */
  function originOf(url) {
    try {
      const u = new URL(url);
      return u.protocol === 'http:' || u.protocol === 'https:' ? u.origin : null;
    } catch (_) {
      return null;
    }
  }

  return { HOST_NAME, PROTOCOL_VERSION, COMMANDS, KeyRoamError, request, parseResponse, nativeFailure, describe, originOf };
});
