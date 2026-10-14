import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { CHROMIUM_KEY, FIREFOX_ID, manifestFor } from '../manifest.mjs';

const require = createRequire(import.meta.url);
const P = require('../src/lib/protocol.js');
const root = dirname(dirname(fileURLToPath(import.meta.url)));
const installRs = readFileSync(join(root, '..', 'crates', 'keyroam-autofill', 'src', 'install.rs'), 'utf8');
const protocolRs = readFileSync(join(root, '..', 'crates', 'keyroam-autofill', 'src', 'protocol.rs'), 'utf8');

const rustConst = (src, name) => src.match(new RegExp(`pub const ${name}: &str = "([^"]+)";`))[1];

/** Chromium derives an extension id from the SHA-256 of its public key. */
function chromiumId(keyBase64) {
  const hex = createHash('sha256').update(Buffer.from(keyBase64, 'base64')).digest('hex').slice(0, 32);
  return [...hex].map((c) => String.fromCharCode(97 + parseInt(c, 16))).join('');
}

test('extension ids and host name match the native host', () => {
  assert.equal(chromiumId(CHROMIUM_KEY), rustConst(installRs, 'CHROMIUM_EXTENSION_ID'));
  assert.equal(FIREFOX_ID, rustConst(installRs, 'FIREFOX_EXTENSION_ID'));
  assert.equal(P.HOST_NAME, rustConst(protocolRs, 'HOST_NAME'));
  assert.equal(String(P.PROTOCOL_VERSION), protocolRs.match(/pub const PROTOCOL_VERSION: u32 = (\d+);/)[1]);
});

test('chromium manifest', () => {
  const m = manifestFor('chromium', '1.2.3');
  assert.equal(m.manifest_version, 3);
  assert.equal(m.version, '1.2.3');
  assert.equal(m.key, CHROMIUM_KEY);
  assert.deepEqual(m.background, { service_worker: 'background.js' });
  assert.equal(m.browser_specific_settings, undefined);
  assert.ok(m.permissions.includes('nativeMessaging'));
});

test('firefox manifest', () => {
  const m = manifestFor('firefox', '1.2.3');
  assert.equal(m.key, undefined);
  assert.equal(m.browser_specific_settings.gecko.id, FIREFOX_ID);
  assert.equal(m.background.scripts.at(-1), 'background.js');
  assert.equal(m.background.service_worker, undefined);
  assert.throws(() => manifestFor('safari', '1'));
});

test('content scripts only run on web pages and every referenced file exists', () => {
  for (const target of ['chromium', 'firefox']) {
    const m = manifestFor(target, '0.0.0');
    assert.deepEqual(m.content_scripts[0].matches, ['http://*/*', 'https://*/*']);
    const files = [
      ...m.content_scripts.flatMap((c) => c.js),
      ...(m.background.scripts || [m.background.service_worker]),
      m.action.default_popup,
      ...Object.values(m.icons),
    ];
    for (const f of files) assert.ok(existsSync(join(root, 'src', f)), `${target}: missing ${f}`);
  }
  // The service worker imports the same libraries Firefox lists.
  const bg = readFileSync(join(root, 'src', 'background.js'), 'utf8');
  for (const lib of manifestFor('firefox', '0').background.scripts.slice(0, -1)) assert.ok(bg.includes(`'${lib}'`));
});

test('package version is the release version', () => {
  const pkg = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));
  const cargo = readFileSync(join(root, '..', 'Cargo.toml'), 'utf8');
  assert.equal(pkg.version, cargo.match(/\[workspace\.package\]\s*version = "([^"]+)"/)[1]);
});
