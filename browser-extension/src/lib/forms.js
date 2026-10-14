/*
 * KeyRoam extension: finding login fields and reading submitted logins.
 *
 * Works on plain field descriptors rather than DOM nodes, so the rules can be
 * tested without a browser. The content script builds the descriptors:
 *   { type, name, id, autocomplete, visible, value }
 */
(function (root, factory) {
  const api = factory();
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.KeyRoamForms = api;
})(typeof self !== 'undefined' ? self : globalThis, function () {
  'use strict';

  const USERNAME_TYPES = ['text', 'email', 'tel', ''];
  const USERNAME_AUTOCOMPLETE = ['username', 'email'];
  const USERNAME_HINT = /user|login|email|e-mail|account|ident|nick|member/i;

  function tokens(f) {
    return String(f.autocomplete || '')
      .toLowerCase()
      .split(/\s+/)
      .filter(Boolean);
  }

  function hasToken(f, t) {
    return tokens(f).includes(t);
  }

  function indexed(fields) {
    return fields.map((f, i) => Object.assign({}, f, { i, type: String(f.type || 'text').toLowerCase() }));
  }

  function isPassword(f) {
    return f.type === 'password';
  }

  function isUsernameCandidate(f) {
    return USERNAME_TYPES.includes(f.type) && !hasToken(f, 'one-time-code') && !hasToken(f, 'off');
  }

  /** The username field that belongs with the password at `passwordIndex`. */
  function pickUsername(visible, passwordIndex) {
    const explicit = visible.find((f) => isUsernameCandidate(f) && USERNAME_AUTOCOMPLETE.some((t) => hasToken(f, t)));
    if (explicit) return explicit;
    const before = visible.filter((f) => isUsernameCandidate(f) && f.i < passwordIndex);
    const hinted = before.filter((f) => USERNAME_HINT.test(`${f.name} ${f.id}`) || f.type === 'email');
    if (hinted.length) return hinted[hinted.length - 1];
    return before.length ? before[before.length - 1] : null;
  }

  /**
   * Fields to fill: `{ username, password }` as indexes into `fields`, or -1.
   * Prefers the "current password" field, so sign-up and change-password
   * forms get the existing password, not the new one.
   */
  function findLoginFields(fields) {
    const visible = indexed(fields).filter((f) => f.visible);
    const passwords = visible.filter(isPassword);
    if (!passwords.length) return { username: -1, password: -1 };
    const password =
      passwords.find((f) => hasToken(f, 'current-password')) ||
      passwords.find((f) => !hasToken(f, 'new-password')) ||
      passwords[0];
    const username = pickUsername(visible, password.i);
    return { username: username ? username.i : -1, password: password.i };
  }

  /**
   * The login being submitted: `{ username, password }`, or null when no
   * password was entered. For change-password and sign-up forms the new
   * password is the one to save.
   */
  function extractSubmission(fields) {
    const visible = indexed(fields).filter((f) => f.visible);
    const filled = visible.filter((f) => isPassword(f) && f.value);
    if (!filled.length) return null;
    const fresh = filled.filter((f) => hasToken(f, 'new-password'));
    let password;
    if (fresh.length) {
      password = fresh[fresh.length - 1].value;
    } else if (filled.length >= 2 && filled[filled.length - 1].value === filled[filled.length - 2].value) {
      // current / new / confirm: the repeated value is the new password.
      password = filled[filled.length - 1].value;
    } else {
      password = filled[0].value;
    }
    const user = pickUsername(
      visible.filter((f) => !isUsernameCandidate(f) || f.value),
      filled[0].i,
    );
    return { username: user ? String(user.value).trim() : '', password };
  }

  return { findLoginFields, extractSubmission };
});
