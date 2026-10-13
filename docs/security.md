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

## What Google (or a Drive attacker) can observe

- That the account uses KeyRoam, the number of devices, and when each syncs.
- The vault id, device ids, KDF parameters, header revision count and timestamps.
- Approximate vault size (1 KiB granularity).

## Known limitations (to address before release)

1. **No vault-key rotation.** Changing the master password does not change the
   vault key. Someone who knows an *old* master password and can still obtain
   the old header (for example from Drive revision history) could unwrap the
   vault key. Planned: a "rotate vault key" operation that re-encrypts all
   replicas.
2. **Memory hygiene is best-effort.** Keys and entry data are zeroised on drop.
   JSON serialisation and UI layers can still leave transient copies in memory.
3. **Deleted entries and history are retained.** Deleted entries and up to 20
   past versions per entry are kept inside the encrypted vault so they can be
   recovered. A "purge permanently" operation is planned.
4. **Concurrent master-password changes:** if two devices change the password
   at the same time, the copy already in Drive wins. The other device is told
   that the password changed elsewhere.
5. **Availability attacks.** Anyone with write access to the Drive app folder can
   delete files or serve stale ones. This cannot reveal or alter data, but it
   can stop sync. Local vaults and exported backups remain intact.
6. **No inter-process lock.** Two KeyRoam processes writing the same data
   directory at the same moment could lose a local edit. Planned: an advisory
   file lock.
7. **Clipboard handling and auto-lock** will come with the GUI apps (milestone 4).
