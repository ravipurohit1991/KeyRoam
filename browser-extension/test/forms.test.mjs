import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { test } from 'node:test';

const require = createRequire(import.meta.url);
const { findLoginFields, extractSubmission } = require('../src/lib/forms.js');

const field = (type, extra = {}) => ({ type, name: '', id: '', autocomplete: '', visible: true, value: '', ...extra });

test('simple login form', () => {
  const fields = [field('hidden', { name: 'csrf', visible: false }), field('text', { name: 'user' }), field('password')];
  assert.deepEqual(findLoginFields(fields), { username: 1, password: 2 });
});

test('no password field means nothing to fill', () => {
  assert.deepEqual(findLoginFields([field('text'), field('email')]), { username: -1, password: -1 });
  assert.deepEqual(findLoginFields([field('password', { visible: false })]), { username: -1, password: -1 });
});

test('prefers hinted and autocomplete-marked username fields', () => {
  const fields = [
    field('text', { name: 'search' }),
    field('email', { name: 'login' }),
    field('text', { name: 'promo' }),
    field('password'),
  ];
  assert.equal(findLoginFields(fields).username, 1);
  const marked = [field('text', { name: 'q' }), field('text', { autocomplete: 'section-a username' }), field('password')];
  assert.equal(findLoginFields(marked).username, 1);
  // One-time-code fields are never treated as usernames.
  const otp = [field('text', { autocomplete: 'one-time-code' }), field('password')];
  assert.equal(findLoginFields(otp).username, -1);
});

test('fills the current password on change-password and sign-up forms', () => {
  const change = [
    field('password', { autocomplete: 'new-password' }),
    field('password', { autocomplete: 'current-password' }),
  ];
  assert.equal(findLoginFields(change).password, 1);
  const signup = [field('email'), field('password', { autocomplete: 'new-password' }), field('password')];
  assert.equal(findLoginFields(signup).password, 2);
});

test('reads a submitted login', () => {
  const fields = [field('email', { value: ' alice@example.test ' }), field('password', { value: 'synthetic-1' })];
  assert.deepEqual(extractSubmission(fields), { username: 'alice@example.test', password: 'synthetic-1' });
});

test('empty or hidden passwords are not submissions', () => {
  assert.equal(extractSubmission([field('text', { value: 'alice' }), field('password')]), null);
  assert.equal(extractSubmission([field('password', { value: 'x', visible: false })]), null);
});

test('change-password forms save the new password', () => {
  const fields = [
    field('text', { name: 'username', value: 'alice' }),
    field('password', { value: 'synthetic-old' }),
    field('password', { value: 'synthetic-new' }),
    field('password', { value: 'synthetic-new' }),
  ];
  assert.deepEqual(extractSubmission(fields), { username: 'alice', password: 'synthetic-new' });
  const marked = [
    field('password', { autocomplete: 'current-password', value: 'synthetic-old' }),
    field('password', { autocomplete: 'new-password', value: 'synthetic-new' }),
  ];
  assert.equal(extractSubmission(marked).password, 'synthetic-new');
});

test('password-only forms have an empty username', () => {
  assert.deepEqual(extractSubmission([field('password', { value: 'synthetic-1' })]), {
    username: '',
    password: 'synthetic-1',
  });
});
