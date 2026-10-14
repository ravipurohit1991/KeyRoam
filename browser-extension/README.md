# KeyRoam browser extension

Fills and saves logins with the KeyRoam desktop app. Works with Chrome, Edge,
Brave and Chromium (Manifest V3, Chrome 116+) and with Firefox 128+.

The extension never stores or decrypts the vault. It asks the desktop app,
through the `keyroam-native-host` program, for the logins saved for the page
you are on. The app answers only while it is unlocked.

```
popup / content script ─► background script ──native messaging──► keyroam-native-host ──► KeyRoam app
```

## Set up

1. Install and open the KeyRoam desktop app (Windows or macOS).
2. In the app, open the menu and choose **Browser autofill… → Set up browsers**.
   This registers `keyroam-native-host` with every installed browser. On macOS,
   where the sandboxed app cannot do this itself, run:

   ```sh
   /Applications/KeyRoam.app/Contents/MacOS/keyroam-native-host install
   ```

3. Install the extension:
   - **Chrome, Edge, Brave:** unzip `KeyRoam-extension-chromium.zip` from a
     release. Open `chrome://extensions` (or `edge://extensions`), turn on
     **Developer mode**, choose **Load unpacked** and select the folder. Its id
     is always `ghgndmpoakmlmcagombpmikjdalnmiec`.
   - **Firefox:** open `about:debugging#/runtime/this-firefox`, choose **Load
     Temporary Add-on**, and select `manifest.json` from the unzipped
     `KeyRoam-extension-firefox.zip`. In the add-on's permissions, allow it to
     access websites.
4. Restart the browser.

Run `keyroam-native-host status` to see which browsers are registered, and
`keyroam-native-host uninstall` to remove the registrations. A store-signed
Chromium build gets a different id. Admit it with
`keyroam-native-host install --extension-id <id>`.

## Use

- **Fill:** click the KeyRoam toolbar button. It lists the logins saved for
  this site (matched by registrable domain, so `login.example.co.uk` and
  `www.example.co.uk` share logins, while `alice.github.io` and
  `bob.github.io` do not). Choose **Fill**.
- **HTTP pages and embedded forms:** on a page without HTTPS, or when the login
  form comes from another origin in an iframe, the popup explains the risk.
  You must tick **Fill anyway** before each such fill.
- **Save and update:** after you sign in with a new login, or with a changed
  password, KeyRoam offers to save it. The prompt also appears on the next
  page if the site navigates. Saved logins appear in the app and sync to
  your other devices.

## Develop

No dependencies. Node 20 or later runs the tests and the build.

```sh
npm test            # unit tests: forms, frames, prompts, protocol, manifests
npm run build       # dist/chromium and dist/firefox
npm run icons       # re-render src/icons from tools/make-icons.mjs
```

| Path | Purpose |
|---|---|
| `manifest.mjs` | One definition, two manifests (`service_worker` for Chromium, `scripts` + `gecko.id` for Firefox) |
| `src/background.js` | The only part that talks to KeyRoam. Routes popup and page messages |
| `src/content.js` | Detects login forms, fills, notices submissions, shows the save prompt |
| `src/popup.*` | Toolbar popup |
| `src/lib/` | Logic shared by the scripts above and covered by unit tests |

The protocol is defined in `crates/keyroam-autofill/src/protocol.rs`. The test
suite checks that the extension ids, host name and protocol version match the
Rust side.
