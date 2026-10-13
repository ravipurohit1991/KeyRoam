# KeyRoam sync protocol (v1)

KeyRoam synchronises through Google Drive's hidden `appDataFolder`. The design
works around one constraint: **Drive has no atomic compare-and-swap.** If
several devices read, modify and write one shared file, an update would
eventually be lost. KeyRoam therefore never has two devices write the same
vault-data file.

## Files in `appDataFolder`

| File | Written by | Content |
|---|---|---|
| `keyroam.header.json` | the creating device; rewritten on password change | Vault header: vault id, revision, KDF params, wrapped vault key, header MAC |
| `keyroam.replica.<device-id>.json` | **only** that device | That device's full encrypted vault state |

All replica payloads are encrypted with the vault key and bound to
`(purpose=replica, vault id, device id)`.

## Data model

- The vault state is a map from `entry id` to `EntryRecord`.
- An `EntryRecord` holds:
  - `versions`: the concurrent versions that no other known version
    supersedes. Normally there is one; more than one with differing content
    means a conflict.
  - `history`: up to 20 superseded versions, kept for recovery.
- Each `Version` has a **version vector** (`device id → counter`), a wall-clock
  timestamp (used for display only), the author device, a deletion flag and the
  entry data.

### Local edit

A local edit creates a version whose clock is the join of all current sibling
clocks, with the local device's counter incremented. It therefore supersedes
every version this device has seen, and those versions move into `history`.
Deletion is an edit that produces a tombstone.

### Merge

For each entry, union both sides' versions and keep only those that no other
version strictly dominates. History is unioned, sorted and capped. The
operation is commutative, associative and idempotent: every device converges to
the same state whatever order replicas are merged in, and stale data never
overrides newer data.

- Edits made without knowledge of each other (concurrent version vectors) both
  survive. The entry is flagged as a conflict until the user resolves it.
- Resolving a conflict is a local edit. It supersedes all siblings and keeps the
  others in history.
- Concurrent "edit vs delete" is a conflict too. A delete never silently wins
  over an unseen edit.

The core crate's property tests check these invariants over random
multi-device operation sequences: commutativity, associativity, idempotence,
convergence, and "no version is lost unless a later edit superseded it".

## Sync round ("Sync Now")

1. **List** `appDataFolder` (paginated).
2. **Header.**
   - None exists: upload the local header. This is the first sync after creating
     the vault.
   - One exists: download it. It must belong to the same vault and verify under
     the vault key.
     - Newer revision (or the same revision with different content): adopt it.
       The master password was changed elsewhere.
     - Older revision: upload the local header.
   - If several headers exist (devices created vaults at the same moment), the
     oldest is canonical.
3. **Pull replicas.** For every `keyroam.replica.*` file whose Drive `version`
   differs from the last merged version, download it, authenticate and decrypt
   it, then merge.
   - Replicas that fail authentication, belong to another vault, or cannot be
     parsed are skipped with a warning. They are not marked as seen, so the next
     sync retries them.
   - Replicas written by a newer, unsupported schema abort the sync with an
     "update KeyRoam" error, so that unknown data is never dropped.
   - The device's own replica is also merged if somebody else changed it (for
     example a copied profile).
4. **Save locally.** If anything changed, the previous `vault.krv` is copied to
   `backups/` and the merged vault is written atomically.
5. **Push.** If there are local changes, or the device has no replica yet,
   encrypt the state and update (or create) this device's replica.
6. Record the outcome (`sync-state.json`) for the status display.

A failure at any step leaves the local vault valid. Pending local changes stay
marked and upload on the next successful sync.

## When sync runs

| Trigger | Desktop | Android | iOS |
|---|---|---|---|
| App opened / vault unlocked | ✅ | ✅ | ✅ |
| After every local modification | ✅ | ✅ | ✅ |
| Connectivity regained | ✅ network change events | ✅ `ConnectivityManager` callback | ✅ `NWPathMonitor` while running |
| Periodic | while running | WorkManager periodic job (OS-scheduled) | BGAppRefreshTask (OS-scheduled, best effort) |
| Manual | Sync Now button | Sync Now button | Sync Now button |

Mobile operating systems restrict background execution. KeyRoam does **not**
promise real-time sync. Background sync on Android and iOS happens when the
OS allows it.

## Onboarding

- **New user:** master password → Google sign-in → refuse if the account already
  holds a vault → create the vault → upload the header and first replica.
- **Existing user, new device:** Google sign-in → download the header → master
  password (unwraps and authenticates the vault key) → save locally → pull
  every replica.

## Backups

Drive's hidden app folder is not an independently recoverable backup. A user
cannot see it, and revoking KeyRoam's access in Google deletes it. KeyRoam
therefore provides:

- **Encrypted export** (`.krbackup`). This is the full state, including history
  and tombstones, protected by a separately chosen backup password with fresh
  key material.
- **Restore by merge.** Importing a backup merges it, so it can only add or
  revive data and never rolls back newer changes.
- **Rotating local backups** of the vault file before every merge that changes it.
