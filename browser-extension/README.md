# KeyRoam browser extension

Fills and saves logins from the KeyRoam desktop app in Chrome, Edge, Brave,
Vivaldi and Firefox (Manifest V3).

The extension holds no vault and no keys. It talks to the **KeyRoam desktop
app** through `keyroam-native-host`, a small helper shipped with the app:

```
extension ──native messaging──► keyroam-native-host ──authenticated loopback bridge──► KeyRoam app (vault)
```

## Security model

- **Unlocked app only.** While KeyRoam is locked, the app answers every
  request except `hello` and `generate` with `locked`.
- **Same site only.** The app releases a credential only to a page whose
  registrable domain (Public Suffix List, private section included) matches a
  URL saved in the entry. `alice.github.io` never sees `bob.github.io`'s login,
  and an https login is never offered on the http version of its site.
- **No silent filling.** Nothing is filled until you pick a login in the
  popup or press the fill shortcut (Ctrl/⌘+Shift+L, which fills only when
  exactly one login matches). The content script fills only if its frame is
  still on the origin the credential was released for.
- **Explicit consent** is required to fill an http page or a login form
  embedded in a frame from another site. The app re-checks both rules.
- **Page URLs come from the browser** (`sender.url`, `tab.url`), never from
  page content.
- **Save prompts** appear in an iframe of an extension page, which the website
  cannot read or script. A pending prompt's password is held in memory-only
  `storage.session` (inaccessible to content scripts) for at most 3 minutes.
- **Least privilege:** `nativeMessaging` and `storage`, plus host access to
  http(s) pages for the form-detection content script.

## Install (development)

1. Install the KeyRoam desktop app, open **Menu → Browser integration…**,
   turn it on and click **Register with browsers**. (Or run
   `keyroam-native-host install` from a terminal.)
2. Build the extension:

   ```sh
   cd browser-extension
   node build.mjs          # → dist/chromium and dist/firefox
   ```

3. Load it:
   - **Chrome / Edge / Brave / Vivaldi:** open `chrome://extensions`, enable
     Developer mode, **Load unpacked** → `dist/chromium`. The manifest key
     gives it the fixed ID `ihplgegclknoghjpemmnfaoeogcahpba`, which the
     native host allows.
   - **Firefox:** open `about:debugging#/runtime/this-firefox`, **Load
     Temporary Add-on…** → `dist/firefox/manifest.json` (or the
     `KeyRoam-extension-firefox.zip` download). Add-on ID
     `keyroam@ravipurohit1991.github.io`. If the popup asks for access to
     websites, click **Allow access to websites** and reload the page:
     Firefox lets you withhold that access, and without it KeyRoam cannot
     see login forms.

The manifest declares both a service worker (Chromium) and background
scripts (Firefox), so `src/` and the Chromium package also load in Firefox;
`dist/firefox` additionally runs on Firefox 115–120.

A store-published build gets a store-assigned Chromium ID; register it with
`keyroam-native-host install --extension-id <id>`.

## Test

```sh
npm test                    # unit tests: forms, policy, native client, background, manifests

# End to end with real Chromium, the real native host and bridge:
cargo build -p keyroam-native-host --bins --examples
node build.mjs && node --test "e2e/*.e2e.js"
```

The end-to-end test needs Playwright with Chromium and `openssl`. It serves a
login page over https on 127.0.0.1, registers the host with
`keyroam-native-host install`, fills a synthetic login, submits a new one and
saves it through the save prompt.

## Layout

| Path | Purpose |
|---|---|
| `src/background.js`, `src/lib/background-core.js` | Message routing, fill and save flows |
| `src/lib/native.js` | Request/response client over the native-messaging port |
| `src/lib/policy.js` | URL helpers, frame choice, error texts |
| `src/content/forms.js` | Login, sign-up and change-password form detection (pure) |
| `src/content/content.js` | Reads forms, fills, captures submissions, shows the save prompt |
| `src/popup/` | Toolbar popup |
| `src/savebar/` | The save/update prompt shown on the page |
| `build.mjs` | Produces the Chromium and Firefox packages |
