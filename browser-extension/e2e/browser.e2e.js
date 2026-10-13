// End-to-end: real Chromium with the built extension, the real native host
// registered through `keyroam-native-host install`, and the real bridge and
// protocol handler (crates/keyroam-native-host/examples/e2e_app.rs).
//
//   cargo build -p keyroam-native-host --bins --examples
//   node build.mjs && node --test "e2e/*.e2e.js"
//
// Needs Playwright with Chromium and openssl. Only synthetic credentials.

import assert from 'node:assert/strict';
import { execFileSync, execSync, spawn } from 'node:child_process';
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { createServer } from 'node:https';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { after, before, test } from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = fileURLToPath(new URL('.', import.meta.url));
const repo = join(here, '..', '..');
const exe = process.platform === 'win32' ? '.exe' : '';
const host = join(repo, 'target', 'debug', `keyroam-native-host${exe}`);
const appBin = join(repo, 'target', 'debug', 'examples', `e2e_app${exe}`);
const extension = join(here, '..', 'dist', 'chromium');

async function loadPlaywright() {
  try {
    return await import('playwright');
  } catch {
    const root = execSync('npm root -g').toString().trim();
    return import(pathToFileURL(join(root, 'playwright', 'index.mjs')).href);
  }
}

const LOGIN_PAGE = `<!doctype html><title>Sign in</title>
<form method="post" action="/welcome">
  <input type="email" name="email" autocomplete="username" placeholder="Email">
  <input type="password" name="password" autocomplete="current-password" placeholder="Password">
  <button type="submit">Sign in</button>
</form>`;

let tmp, server, origin, app, context, worker;
const saved = [];

before(async () => {
  tmp = mkdtempSync(join(tmpdir(), 'keyroam-e2e-'));
  const home = join(tmp, 'home');
  const data = join(tmp, 'data');
  mkdirSync(join(home, '.config', 'chromium'), { recursive: true });

  execFileSync('openssl', [
    'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-subj', '/CN=127.0.0.1',
    '-addext', 'subjectAltName=IP:127.0.0.1,DNS:localhost',
    '-keyout', join(tmp, 'key.pem'), '-out', join(tmp, 'cert.pem'),
  ], { stdio: 'ignore' });
  server = createServer({ key: readFileSync(join(tmp, 'key.pem')), cert: readFileSync(join(tmp, 'cert.pem')) }, (req, res) => {
    res.setHeader('content-type', 'text/html; charset=utf-8');
    if (req.url === '/login') return res.end(LOGIN_PAGE);
    if (req.url === '/welcome') return req.resume().on('end', () => res.end('<!doctype html><title>Welcome</title><p>Welcome'));
    if (req.url === '/embed') {
      const port = server.address().port;
      return res.end(`<!doctype html><title>Embed</title><iframe src="https://localhost:${port}/login" width=600 height=300></iframe>`);
    }
    res.statusCode = 404;
    res.end();
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  origin = `https://127.0.0.1:${server.address().port}`;

  // The stand-in app publishes its bridge in the data directory.
  app = spawn(appBin, [data, origin], { stdio: ['pipe', 'pipe', 'inherit'] });
  await new Promise((resolve, reject) => {
    app.stdout.on('data', (chunk) => {
      for (const line of chunk.toString().split('\n')) {
        if (line === 'READY') resolve();
        if (line.startsWith('SAVED ')) saved.push(line.slice(6));
      }
    });
    app.on('exit', (code) => reject(new Error(`e2e_app exited with ${code}`)));
  });

  // Register the host the way users do.
  const env = { ...process.env, HOME: home, XDG_CONFIG_HOME: join(home, '.config'), KEYROAM_HOME: data };
  const out = execFileSync(host, ['install'], { env }).toString();
  assert.match(out, /Chromium/);
  // On Linux, Chromium reads user-level hosts from its user-data directory;
  // the test profile is not the default one, so give it the same manifest.
  const manifest = 'io.github.ravipurohit1991.keyroam.json';
  mkdirSync(join(tmp, 'profile', 'NativeMessagingHosts'), { recursive: true });
  copyFileSync(
    join(home, '.config', 'chromium', 'NativeMessagingHosts', manifest),
    join(tmp, 'profile', 'NativeMessagingHosts', manifest),
  );

  const { chromium } = await loadPlaywright();
  context = await chromium.launchPersistentContext(join(tmp, 'profile'), {
    channel: 'chromium',
    headless: true,
    ignoreHTTPSErrors: true,
    args: [`--disable-extensions-except=${extension}`, `--load-extension=${extension}`],
    env,
  });
  worker = context.serviceWorkers()[0] ?? (await context.waitForEvent('serviceworker'));
});

after(async () => {
  await context?.close();
  app?.stdin.end();
  app?.kill();
  server?.close();
  if (tmp) rmSync(tmp, { recursive: true, force: true });
});

const bg = (fn, arg) => worker.evaluate(fn, arg);

/** Set KEYROAM_E2E_SCREENSHOTS to a folder to keep screenshots. */
async function screenshot(page, name) {
  const dir = process.env.KEYROAM_E2E_SCREENSHOTS;
  if (dir) await page.screenshot({ path: join(dir, `${name}.png`) });
}

test('the extension has the ID the native host allows', () => {
  assert.equal(new URL(worker.url()).host, 'ihplgegclknoghjpemmnfaoeogcahpba');
});

test('fill a saved login, then offer to save a new one after sign-in', async () => {
  const page = await context.newPage();
  await page.goto(`${origin}/login`);
  await page.bringToFront();
  await page.waitForTimeout(500); // content script reports the form

  const state = await bg(() => globalThis.keyroamBackground.popupState());
  assert.equal(state.error, null, JSON.stringify(state.error));
  assert.deepEqual(
    state.logins.map((l) => [l.title, l.username]),
    [['Local test', 'alice@example.test']],
    'only the login saved for this site is offered',
  );

  const filled = await bg((id) => globalThis.keyroamBackground.fill(id), state.logins[0].id);
  assert.deepEqual(filled, { ok: true });
  assert.equal(await page.inputValue('input[name=email]'), 'alice@example.test');
  assert.equal(await page.inputValue('input[name=password]'), 'synthetic-e2e-secret');

  // A different login typed by the user is offered for saving after submit.
  await page.fill('input[name=email]', 'bob@example.test');
  await page.fill('input[name=password]', 'synthetic-new-login');
  await page.click('button[type=submit]');
  await page.waitForURL('**/welcome');
  let bar;
  for (let i = 0; i < 50 && !bar; i++) {
    bar = page.frames().find((f) => f.url().endsWith('/savebar/savebar.html'));
    if (!bar) await page.waitForTimeout(100);
  }
  assert.ok(bar, 'save prompt shown');
  await bar.waitForSelector('#bar:not([hidden])');
  assert.match(await bar.textContent('#question'), /Save login for 127\.0\.0\.1/);
  assert.equal(await bar.textContent('#detail'), 'bob@example.test');
  await screenshot(page, 'save-prompt');
  await bar.click('#save');
  for (let i = 0; i < 50 && saved.length === 0; i++) await page.waitForTimeout(100);
  assert.deepEqual(saved, ['127.0.0.1 bob@example.test']);
  await page.waitForTimeout(200);
  assert.equal(page.frames().some((f) => f.url().includes('savebar')), false, 'prompt removed');
  await page.close();
});

test('a login form embedded from another site needs consent', async () => {
  const page = await context.newPage();
  await page.goto(`${origin}/embed`);
  await page.bringToFront();
  await page.waitForTimeout(800);
  const state = await bg(() => globalThis.keyroamBackground.popupState());
  assert.equal(state.error?.code, 'cross_site_frame');
  const consented = await bg(() => globalThis.keyroamBackground.popupState({ allowCrossSite: true }));
  assert.equal(consented.error, null);
  // localhost is a different site from 127.0.0.1: nothing saved for it.
  assert.deepEqual(consented.logins, []);
  await page.close();
});

test('the popup renders', async () => {
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (e) => errors.push(e));
  const id = new URL(worker.url()).host;
  // Opened as a tab, the popup's "active tab" is itself: not a website.
  await page.goto(`chrome-extension://${id}/popup/popup.html`);
  await page.waitForSelector('#status:not([hidden])');
  assert.match(await page.textContent('#status'), /websites only/);
  await page.click('#generate');
  await page.waitForSelector('#generated-row:not([hidden])');
  assert.equal((await page.inputValue('#generated')).length, 20);
  await screenshot(page, 'popup');
  assert.deepEqual(errors, []);
  await page.close();
});
