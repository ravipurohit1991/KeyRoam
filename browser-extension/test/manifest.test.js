import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import { build, FIREFOX_ID } from '../build.mjs';

const root = new URL('../', import.meta.url);
const rustInstall = readFileSync(new URL('../crates/keyroam-autofill/src/install.rs', root), 'utf8');
const rustConst = (name) => rustInstall.match(new RegExp(`pub const ${name}: &str = "([^"]+)"`))[1];

/** Chromium derives an unpacked extension's ID from the manifest key. */
function extensionId(keyBase64) {
  const hex = createHash('sha256').update(Buffer.from(keyBase64, 'base64')).digest('hex').slice(0, 32);
  return [...hex].map((c) => String.fromCharCode(97 + parseInt(c, 16))).join('');
}

test('built manifests for Chromium and Firefox', () => {
  const out = mkdtempSync(join(tmpdir(), 'keyroam-ext-'));
  try {
    const { chromium, firefox } = build(out);
    for (const browser of ['chromium', 'firefox']) {
      const m = JSON.parse(readFileSync(join(out, browser, 'manifest.json'), 'utf8'));
      assert.equal(m.manifest_version, 3);
      // Every file the manifest names exists in the package.
      const files = [
        ...Object.values(m.icons),
        m.action.default_popup,
        ...m.content_scripts.flatMap((c) => c.js),
        ...m.web_accessible_resources.flatMap((w) => w.resources),
        m.background.service_worker ?? m.background.scripts[0],
      ];
      for (const f of files) assert.ok(existsSync(join(out, browser, f)), `${browser}: ${f}`);
    }

    // Least privilege: no tabs, cookies, history, scripting or clipboard access.
    assert.deepEqual(chromium.permissions.sort(), ['nativeMessaging', 'storage']);
    assert.deepEqual(chromium.host_permissions, ['http://*/*', 'https://*/*']);
    assert.match(chromium.content_security_policy.extension_pages, /script-src 'self'/);
    assert.equal(chromium.background.service_worker, 'background.js');

    // The IDs agree with what the native host allows.
    assert.equal(extensionId(chromium.key), rustConst('CHROMIUM_EXTENSION_ID'));
    assert.equal(FIREFOX_ID, rustConst('FIREFOX_EXTENSION_ID'));
    assert.equal(firefox.browser_specific_settings.gecko.id, FIREFOX_ID);
    assert.equal(firefox.key, undefined);
    assert.deepEqual(firefox.background, { scripts: ['background.js'], type: 'module' });
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
});

test('the background talks to the registered host name', () => {
  const core = readFileSync(new URL('src/lib/background-core.js', root), 'utf8');
  assert.match(core, new RegExp(`HOST_NAME = '${rustConst('HOST_NAME').replaceAll('.', '\\.')}'`));
});
