import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';

// forms.js is a classic content script; run it the way the browser does.
const context = vm.createContext({});
vm.runInContext(readFileSync(new URL('../src/content/forms.js', import.meta.url), 'utf8'), context);
const Forms = context.KeyRoamForms;

const field = (type, extra = {}) => ({ type, visible: true, disabled: false, readOnly: false, ...extra });
const plain = (x) => JSON.parse(JSON.stringify(x));

test('classic login form', () => {
  const fields = [field('text', { name: 'q', placeholder: 'Search' }), field('email', { name: 'email' }), field('password', { name: 'pass' })];
  assert.deepEqual(plain(Forms.analyzeForm(fields)), { kind: 'login', username: 1, current: 2, newPasswords: [] });
});

test('autocomplete hints win over field order', () => {
  const fields = [
    field('text', { autocomplete: 'username', name: 'a' }),
    field('text', { name: 'b' }),
    field('password', { autocomplete: 'current-password' }),
  ];
  assert.equal(Forms.analyzeForm(fields).username, 0);
});

test('hidden, disabled and read-only fields are ignored', () => {
  const fields = [
    field('text', { name: 'user' }),
    field('password', { visible: false }),
    field('password', { disabled: true }),
  ];
  assert.equal(Forms.analyzeForm(fields).kind, 'username');
  assert.equal(Forms.analyzeForm([field('text', { name: 'misc' })]).kind, 'none');
  assert.equal(Forms.analyzeForm([field('text', { name: 'user', readOnly: true })]).kind, 'none');
});

test('identifier-first login step', () => {
  const a = Forms.analyzeForm([field('email', { name: 'identifier' })]);
  assert.equal(a.kind, 'username');
  assert.equal(a.username, 0);
  // A lone search box is not a login.
  assert.equal(Forms.analyzeForm([field('text', { name: 'search' })]).kind, 'none');
});

test('sign-up and change-password forms', () => {
  const signup = Forms.analyzeForm([field('email'), field('password'), field('password', { name: 'confirm' })]);
  assert.equal(signup.kind, 'signup');
  assert.deepEqual(plain(signup.newPasswords), [1, 2]);

  const withHints = Forms.analyzeForm([
    field('password', { autocomplete: 'current-password' }),
    field('password', { autocomplete: 'new-password' }),
    field('password', { autocomplete: 'new-password' }),
  ]);
  assert.equal(withHints.kind, 'change');
  assert.equal(withHints.current, 0);
  assert.deepEqual(plain(withHints.newPasswords), [1, 2]);

  const noHints = Forms.analyzeForm([field('password'), field('password'), field('password')]);
  assert.equal(noHints.kind, 'change');
  assert.equal(noHints.current, 0);

  const single = Forms.analyzeForm([field('email'), field('password', { autocomplete: 'new-password' })]);
  assert.equal(single.kind, 'signup');
});

test('one-time codes and payment fields are not usernames', () => {
  assert.equal(Forms.isUsernameCandidate(field('text', { autocomplete: 'one-time-code' })), false);
  assert.equal(Forms.isUsernameCandidate(field('text', { name: 'cardnumber' })), false);
  assert.equal(Forms.isUsernameCandidate(field('tel', { name: 'login' })), true);
});

test('submitted values', () => {
  const login = Forms.analyzeForm([field('email'), field('password')]);
  assert.deepEqual(plain(Forms.submittedLogin(login, [' alice@example.test ', 'synthetic-1'])), {
    username: 'alice@example.test',
    password: 'synthetic-1',
  });
  assert.equal(Forms.submittedLogin(login, ['alice', '']), null);

  const change = Forms.analyzeForm([field('password'), field('password'), field('password')]);
  assert.deepEqual(plain(Forms.submittedLogin(change, ['old', 'new-1', 'new-1'])), { username: '', password: 'new-1' });
  // Mismatched confirmation: the site will reject it, so do not offer to save.
  assert.equal(Forms.submittedLogin(change, ['old', 'new-1', 'new-2']), null);
  assert.equal(Forms.submittedLogin(Forms.analyzeForm([field('email', { name: 'user' })]), ['alice']), null);
});

test('best kind for a page', () => {
  const k = (kind) => ({ kind });
  assert.equal(Forms.bestKind([k('signup'), k('login'), k('none')]), 'login');
  assert.equal(Forms.bestKind([]), 'none');
});
