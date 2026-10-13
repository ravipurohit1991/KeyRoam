// Login-form detection. Pure functions over field descriptors, so they can be
// tested without a DOM; content.js builds the descriptors from real inputs.
//
// A descriptor is {type, name, id, autocomplete, placeholder, ariaLabel,
// label, visible, disabled, readOnly}.
(function (root) {
  'use strict';

  const USERNAME_HINT = /user|login|logon|e-?mail|account|acct|ident|member|nick|handle|benutzer|usuario|utilisateur|courriel/i;
  const NOT_USERNAME =
    /search|query|captcha|coupon|promo|otp|one.?time|verif|2fa|mfa|totp|token|zip|postal|city|street|address|first.?name|last.?name|full.?name|card|cvc|cvv/i;
  const NEW_PASSWORD_HINT = /new|confirm|repeat|retype|again|create|choose|register|sign.?up/i;
  const CURRENT_PASSWORD_HINT = /current|old|existing/i;
  const TEXT_LIKE = new Set(['text', 'email', 'tel', '']);

  function hints(d) {
    return [d.name, d.id, d.placeholder, d.ariaLabel, d.label].filter(Boolean).join(' ');
  }

  function usable(d) {
    return Boolean(d.visible) && !d.disabled && !d.readOnly;
  }

  /** 'current', 'new' or 'unknown'. */
  function passwordKind(d) {
    const ac = (d.autocomplete || '').toLowerCase();
    if (ac.includes('new-password')) return 'new';
    if (ac.includes('current-password')) return 'current';
    const h = hints(d);
    if (CURRENT_PASSWORD_HINT.test(h)) return 'current';
    if (NEW_PASSWORD_HINT.test(h)) return 'new';
    return 'unknown';
  }

  function isUsernameCandidate(d) {
    if (!usable(d) || !TEXT_LIKE.has((d.type || '').toLowerCase())) return false;
    const ac = (d.autocomplete || '').toLowerCase();
    if (ac.includes('username') || ac.includes('email')) return true;
    if (ac.includes('one-time-code')) return false;
    return !NOT_USERNAME.test(hints(d));
  }

  function usernameScore(d) {
    const ac = (d.autocomplete || '').toLowerCase();
    if (ac.includes('username')) return 3;
    if (ac.includes('email') || (d.type || '').toLowerCase() === 'email' || USERNAME_HINT.test(hints(d))) return 2;
    return 1;
  }

  const NONE = Object.freeze({ kind: 'none', username: null, current: null, newPasswords: [] });

  /**
   * Classify one form (fields in document order).
   * kind: 'login' (username + password), 'username' (identifier-first step),
   * 'signup' (new password only), 'change' (current and new password) or 'none'.
   * Indexes refer to `fields`.
   */
  function analyzeForm(fields) {
    const passwords = [];
    fields.forEach((d, i) => {
      if ((d.type || '').toLowerCase() === 'password' && usable(d)) passwords.push(i);
    });
    const end = passwords.length ? passwords[0] : fields.length;
    let username = null;
    let best = 0;
    for (let i = 0; i < end; i++) {
      if (!isUsernameCandidate(fields[i])) continue;
      const score = usernameScore(fields[i]);
      // The closest strong candidate before the password wins.
      if (score >= best) {
        best = score;
        username = i;
      }
    }

    if (passwords.length === 0) {
      return username !== null && best >= 2 ? { kind: 'username', username, current: null, newPasswords: [] } : NONE;
    }

    const kinds = passwords.map((i) => passwordKind(fields[i]));
    if (passwords.length === 1) {
      return kinds[0] === 'new'
        ? { kind: 'signup', username, current: null, newPasswords: [passwords[0]] }
        : { kind: 'login', username, current: passwords[0], newPasswords: [] };
    }

    let current = null;
    const newPasswords = [];
    const unknown = [];
    passwords.forEach((i, k) => {
      if (kinds[k] === 'current' && current === null) current = i;
      else if (kinds[k] === 'new') newPasswords.push(i);
      else unknown.push(i);
    });
    if (current === null && newPasswords.length === 0) {
      // No hints at all: current + new + confirm, or new + confirm.
      if (unknown.length >= 3) current = unknown.shift();
      newPasswords.push(...unknown);
    } else {
      // An unhinted field before the new ones is the current password only
      // in a three-field form; "password, confirm" is a sign-up.
      for (const i of unknown) {
        if (current === null && passwords.length >= 3 && i < Math.min(...newPasswords)) current = i;
        else newPasswords.push(i);
      }
      newPasswords.sort((a, b) => a - b);
    }
    let kind = 'signup';
    if (current !== null) kind = newPasswords.length ? 'change' : 'login';
    return { kind, username, current, newPasswords };
  }

  /**
   * The login to offer for saving when a form is submitted, from the
   * fields' current values: the new password if the form sets one (all
   * confirmations must agree), otherwise the entered password.
   */
  function submittedLogin(analysis, values) {
    if (!['login', 'signup', 'change'].includes(analysis.kind)) return null;
    let password = '';
    if (analysis.newPasswords.length) {
      password = values[analysis.newPasswords[0]] || '';
      if (analysis.newPasswords.some((i) => values[i] !== password)) return null;
    } else if (analysis.current !== null) {
      password = values[analysis.current] || '';
    }
    if (!password) return null;
    const username = analysis.username !== null ? (values[analysis.username] || '').trim() : '';
    return { username, password };
  }

  const RANK = { login: 0, username: 1, change: 2, signup: 3, none: 9 };

  /** The most fillable kind among analyses, for reporting a page. */
  function bestKind(analyses) {
    return analyses.reduce((best, a) => (RANK[a.kind] < RANK[best] ? a.kind : best), 'none');
  }

  const api = { analyzeForm, passwordKind, isUsernameCandidate, submittedLogin, bestKind, RANK };
  root.KeyRoamForms = api;
})(globalThis);
