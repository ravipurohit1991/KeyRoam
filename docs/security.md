# KeyRoam security design

This document describes what KeyRoam protects, how, and what it does not yet
protect. It is the starting point for the milestone 8 security review.

## Goals

1. Google, or anyone with access to the user's Google Drive, learns nothing
   about vault contents and cannot modify them undetected.
2. A stolen device's vault file is protected by the master password, using a
   memory-hard KDF.
3. Sync never silently discards a change, including when devices edit offline
   or when an attacker replays old data.
4. KeyRoam has no developer-operated server, so there is no KeyRoam service to
   breach.

## Primitives

All primitives come from the audited RustCrypto crates. Nothing is hand-rolled.

| Purpose | Algorithm | Crate |
|---|---|---|
| Password → key-encryption key (KEK) | Argon2id v1.3. Default 64 MiB, t=3, p=1. 16-byte random salt | `argon2` |
| Authenticated encryption | XChaCha20-Poly1305, 192-bit random nonce per message | `chacha20poly1305` |
| Sub-key derivation | HKDF-SHA256 | `hkdf` |
| Header integrity | HMAC-SHA256 | `hmac` |
| Randomness | OS CSPRNG (`OsRng`) | `rand` |

Passwords are Unicode NFC-normalised before key derivation, so the same password
typed on different platforms gives the same key.

## Key hierarchy

```
master password ──Argon2id(salt, params)──► KEK
KEK ──XChaCha20-Poly1305──► wraps the vault key (random 256-bit, created once)
vault key ──HKDF "keyroam/v1/payload"─────► payload encryption key
vault key ──HKDF "keyroam/v1/header-auth"─► header MAC key
```

- Changing the master password only re-wraps the vault key, with a fresh salt.
  Replicas on other devices stay readable.
- The header (KDF params, wrapped key, revision, timestamps) is MAC'd with a
  vault-key sub-key. A device adopts a header from Drive only when it verifies
  under the vault key it already holds. An attacker who can write to Drive
  cannot push a header that locks users out or downgrades the KDF.
- The wrapping AEAD also binds the vault id and the full KDF parameters as
  associated data.

### Vault-key rotation

"Replace vault key" (`keyroam rotate-key`) needs the master password. It
creates a fresh random vault key with the next `key_epoch` and a version-2
header. The earlier keys travel in the header's `previous_keys`, encrypted
under the *new* key (HKDF "keyroam/v2/keyring", bound to the vault id and
epoch), so data that other devices wrote before they switch stays readable
to holders of the new key. The new key is never encrypted under an old one:
an old key, an old password or an old header gives no way to it.

- The rotating device re-encrypts its local vault and device secrets and
  uploads the new header (MAC'd with the new key) and its replica.
- Other devices cannot verify the new header with the key they hold. Their
  next sync stops with `key_rotated` and asks for the master password, which
  must unwrap the new key; the header's key list must also contain a key the
  device already holds, so a header cannot switch a device to another
  vault. The device then re-encrypts its own files and replica.
- Two devices rotating at the same time: the header that reached Drive first
  wins; the other device adopts it with the password.
- Older KeyRoam versions refuse version-2 headers ("update KeyRoam") rather
  than writing with the old key.

## Payload encryption

Each encrypted blob (local vault, Drive replica, backup, device secrets) is
bound through AEAD associated data to:

- its **purpose** (`local`, `replica`, `backup`, `local-secrets`),
- the **vault id**,
- the **device id** (where relevant).

Blobs cannot be swapped between vaults, devices or purposes. Plaintext is
length-prefixed and zero-padded to 1 KiB buckets before encryption, so a
ciphertext reveals only a coarse size class.

## KDF parameter policy

When reading a vault, KeyRoam rejects KDF parameters below the OWASP Argon2id
baseline (19 MiB, t=2) and above sane maxima (1 GiB, t=20, p=16). This prevents
both downgrade attacks and resource-exhaustion attacks through a malicious
header. The test suite uses the baseline parameters themselves. Argon2 is
compiled with optimisations even in debug builds, so no "insecure test mode"
exists.

## Google authorisation

- OAuth 2.0 authorisation-code flow with **PKCE (S256)**. The desktop flow uses
  a loopback redirect on `127.0.0.1:<random port>` (RFC 8252) and a random
  `state` value, compared in constant time.
- The Android app uses a mobile client without a secret. Google redirects to
  the reversed client id, a private-use URI scheme (RFC 8252 §7.1). The
  engine accepts only a redirect for that exact scheme and path with the
  expected `state`. If another app registered the same scheme and caught the
  redirect, it still could not redeem the code without the PKCE verifier,
  which never leaves KeyRoam.
- The only scope requested is `https://www.googleapis.com/auth/drive.appdata`,
  which gives access to KeyRoam's private app folder and nothing else in the
  user's Drive. Token responses that lack this scope are rejected.
- The refresh token is stored in `secrets.krs`, encrypted with the vault key and
  bound to the device. It is therefore usable only after the vault is unlocked.
- Endpoints must use HTTPS. Plain HTTP is accepted only for loopback addresses,
  which the test suite uses.
- Google "Desktop app" OAuth clients receive a client secret that is **not
  confidential**, because it ships inside the app. PKCE protects the
  authorisation code.

## Sync integrity

See [sync-protocol.md](sync-protocol.md). Security-relevant properties:

- Each device writes only its own replica file, so concurrent writers never
  clobber each other.
- The merge is a state-based CRDT (multi-value register per entry, using version
  vectors). Merging is commutative, associative and idempotent. **Replaying an
  old replica cannot remove or roll back newer data.**
- Replicas that fail authentication, or belong to another vault, are skipped
  with a visible warning and are never merged.
- Before merged remote data replaces the local vault file, the previous file is
  copied to a rotating local backup folder (last 10).

## Local storage

- `vault.krv`: the header plus the encrypted state. Written atomically (temp
  file, fsync, rename). On Unix the mode is 0600.
- `secrets.krs`: the encrypted Google refresh token.
- `sync-state.json`: non-secret bookkeeping (Drive file ids and versions, last
  sync time, last error).
- `backups/`: rotating copies of earlier `vault.krv` files.
- `settings.json`: non-secret device preferences (browser integration on/off).
- `autofill-bridge.json`: while browser integration runs, the bridge port and
  its random key (mode 0600 on Unix). Removed when the app stops serving.
- `.keyroam.lock`: advisory lock. Every read-modify-write of the vault (an
  edit, a sync merge, a password change) holds it and first merges what
  another KeyRoam process saved, so the desktop app and the CLI can share a
  data directory without losing edits.

## Browser autofill

```
extension ──native messaging──► keyroam-native-host ──loopback bridge──► desktop app (vault)
```

- **The extension holds no vault and no keys.** It receives one login, after
  the user picks it in the popup (or presses the fill shortcut when exactly
  one login matches), for the page it is on.
- **The app decides.** It answers only while unlocked, and releases a login
  only to a page whose registrable domain (Public Suffix List, private
  section included) matches one of the entry's URLs. A login saved for https
  is never offered on http. Filling an http page, or a form inside a frame of
  a different site than the tab, requires the user's explicit consent for
  that request; the app re-checks both rules rather than trusting the
  extension.
- **Page URLs come from the browser** (`sender.url`, `tab.url`), never from
  page content. The content script fills only if its frame is still on the
  origin the login was released for.
- **Bridge.** The browser starts the host; the host connects to the app on
  127.0.0.1 using the port and 256-bit key in `autofill-bridge.json`. Each
  connection starts with a mutual HMAC-SHA256 challenge-response, so the key
  never crosses the socket and neither side talks to an impostor. The browser
  only lets the extension IDs listed in the host manifest start the host.
- **Save prompts** run in an extension-page iframe that the website cannot
  read or script. The pending password is kept in memory-only
  `storage.session`, inaccessible to content scripts, for at most 3 minutes.
- **Off by default.** Browser integration is a per-device setting.

Malware running as the same OS user can read the bridge file and talk to the
app, as it could also read the screen or the keyboard. KeyRoam does not try to
defend against a compromised user account.

## Android

- **One engine.** The autofill service, the Credential Manager provider,
  background sync and biometric unlock call the same Rust engine as the UI
  (JNI), in the same process. Nothing is decrypted in Kotlin except the
  key-encryption key during a biometric unlock.
- **Who is asking.** Android reports the package of the app on screen (and,
  for Credential Manager, the calling package). Apps see only logins saved
  for that package (`androidapp://<package>`, or Google Password Manager's
  `android://…@<package>`). A web domain in the view structure is believed
  only from known browsers (`keyroam_autofill::android::TRUSTED_BROWSERS`),
  and then matched by registrable domain with the desktop rules, so logins
  saved for https are never offered on http. Any other app can put a web
  domain into its views, so for those apps it is ignored and the app sees
  only its own logins. KeyRoam never fills its own screens.
- **Locked means locked.** While the vault is locked, the services offer only
  an unlock prompt. A vault unlocked from such a prompt locks again after
  5 minutes unless the app is open, where its own inactivity timer applies.
- **Saving** happens only after the user accepts the system's save prompt or
  an app's Credential Manager request. A save can update only a login of the
  same app or site, and the old password stays in the entry's history.
- **Biometric unlock.** The KEK is encrypted with an AES-256-GCM key in the
  Android Keystore that requires a strong biometric for every use and is
  invalidated when biometrics are added or removed. The encrypted KEK lives
  in the app's no-backup folder. It is bound to the vault header's salt, so
  a master-password change or vault-key rotation makes it useless, and the
  app then turns biometric unlock off.
- **Background sync** runs only while the vault is unlocked in memory, because
  merging and uploading need the vault key. KeyRoam never stores the master
  password or the vault key for background use.

## Permanent purge

Deleting moves an entry to "recently deleted". **Delete permanently**
(`keyroam purge`) records the entry's version-vector clock as a purge mark in
the encrypted state. Merges join the marks and erase every version at or
before them, with the entry's whole history, on every device. A stale replica
cannot bring purged data back, but an edit made concurrently on a device that
had not seen the purged state is not covered and survives (nothing is lost
silently). Marks hold only an id and a clock.

## What Google (or a Drive attacker) can observe

- That the account uses KeyRoam, the number of devices, and when each syncs.
- The vault id, device ids, KDF parameters, header revision count and timestamps.
- Approximate vault size (1 KiB granularity).

## Known limitations (to address before release)

1. **Key rotation is not automatic.** Changing the master password does not
   change the vault key; someone who knows an *old* master password and can
   obtain an old header (for example from Drive revision history) could unwrap
   the old vault key and read data written with it. Use **Replace vault key**
   after a password may have leaked. Replicas of devices that never sync again
   stay encrypted under the old key in Drive.
2. **Memory hygiene is best-effort.** Keys and entry data are zeroised on drop.
   JSON serialisation and UI layers can still leave transient copies in memory.
3. **Deleted entries and history are retained until purged.** Deleted entries
   and up to 20 past versions per entry stay inside the encrypted vault so
   they can be recovered, until they are deleted permanently. Earlier copies
   can survive a purge in exported backups, the 10 rotating local backups,
   and Drive's own revision history of replica files (kept for up to 30 days).
4. **Concurrent master-password changes:** if two devices change the password
   at the same time, the copy already in Drive wins. The other device is told
   that the password changed elsewhere.
5. **Availability attacks.** Anyone with write access to the Drive app folder can
   delete files or serve stale ones. This cannot reveal or alter data, but it
   can stop sync. Local vaults and exported backups remain intact.
6. **Desktop app:** the app auto-locks after 5 min of inactivity and when the
   OS locks the screen or sleeps, and clears copied passwords from the
   clipboard after 30 s. Dart strings that hold secrets cannot be wiped, so
   they remain in memory until garbage-collected.
7. **CSV import:** a browser's CSV export holds passwords in plaintext.
   KeyRoam can overwrite and delete the file after importing, but SSDs,
   backups and file-system snapshots may keep copies.
8. **Browser extension:** passwords copied from the extension's generator are
   not cleared from the clipboard automatically.
9. **Android browser list.** A browser is recognised by its package name only.
   If a real browser is not installed, a malicious app could install itself
   under that browser's package name and claim any web domain. Checking each
   browser's signing certificate is planned.
10. **Android memory.** Logins handed to Android for filling, and the strings
   Kotlin builds for the engine, are Java strings that cannot be wiped.
