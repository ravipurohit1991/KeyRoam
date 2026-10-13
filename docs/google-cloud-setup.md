# Google Cloud setup

KeyRoam has no backend, but Google requires every app that calls the Drive API
to have an OAuth client registration. You create it once in a Google Cloud
project that you control.

## 1. Project and API

1. Open <https://console.cloud.google.com/> and create a project, for example
   `keyroam`.
2. Go to **APIs & Services → Library**, then search for and enable the
   **Google Drive API**.

## 2. OAuth consent screen

1. Go to **APIs & Services → OAuth consent screen** (shown as "Google Auth
   Platform" in newer consoles).
2. Choose user type **External**, then fill in the app name (KeyRoam), the
   support email and the developer contact.
3. Under **Data access / Scopes**, add exactly one scope:
   `https://www.googleapis.com/auth/drive.appdata`
   ("See, create, and delete its own configuration data in your Google Drive").
   Do not add any broader Drive scope.
4. While the app is in **Testing** status, add your own Google account(s) under
   **Test users**. Before a public release, the app must go through Google's
   verification for the scopes it uses. Check the console for the current
   classification of `drive.appdata` and its requirements.

## 3. OAuth clients

Create one client per platform family under **APIs & Services → Credentials →
Create credentials → OAuth client ID**:

| Platform | Application type | Notes |
|---|---|---|
| Windows / macOS / CLI | **Desktop app** | Uses a loopback redirect (`http://127.0.0.1:<port>`). Google issues a client secret, but for desktop apps it is not treated as confidential. |
| Android (milestone 6) | **Android** | Needs the package name and the SHA-1 of the signing certificate. |
| iOS (milestone 7) | **iOS** | Needs the bundle id. Uses the reversed client id as a custom URL scheme. |

## 4. Configure KeyRoam

For development, provide the desktop client through environment variables:

```sh
# bash / zsh
export KEYROAM_GOOGLE_CLIENT_ID="1234567890-abc.apps.googleusercontent.com"
export KEYROAM_GOOGLE_CLIENT_SECRET="GOCSPX-..."
```

```powershell
# PowerShell
$env:KEYROAM_GOOGLE_CLIENT_ID = "1234567890-abc.apps.googleusercontent.com"
$env:KEYROAM_GOOGLE_CLIENT_SECRET = "GOCSPX-..."
```

Release builds embed these values at compile time. The same variable names are
read by `option_env!` during `cargo build`, so end users never have to set them.

Then run `keyroam init` (new user) or `keyroam join` (another device).

## Revoking access

A user can remove KeyRoam's access at <https://myaccount.google.com/permissions>.
This also deletes the app's hidden `appDataFolder` data. That is one reason
KeyRoam keeps a local vault on every device and offers encrypted backup export.
