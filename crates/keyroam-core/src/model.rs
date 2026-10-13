//! Vault data model and conflict-free merge.
//!
//! Each entry is a *multi-value register*: it holds the set of concurrent
//! versions ("siblings") that no other known version supersedes. Causality is
//! tracked with per-entry version vectors keyed by device id.
//!
//! * A local edit creates a version whose clock dominates every current
//!   sibling, so it supersedes them (they move to `history`).
//! * Merging two states unions siblings and keeps only the maximal ones. Two
//!   edits made concurrently on different devices both survive the merge and
//!   the entry is flagged as conflicting until the user resolves it. Nothing is
//!   silently overwritten.
//! * The merge is commutative, associative and idempotent (a state-based CRDT),
//!   so every device converges regardless of sync order, and replaying stale
//!   data can never remove newer data.
//! * A permanent purge records the entry's clock as a *purge mark*. Merges
//!   join the marks and erase every version at or before them, together with
//!   the entry's history, so a purge reaches every device and stale replicas
//!   cannot bring the data back. An edit made concurrently on a device that
//!   had not seen the purged state is not covered by the mark and survives.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroize;

use crate::error::{Error, Result};

/// Current schema of [`VaultState`]. Readers refuse newer schemas instead of
/// risking data loss by dropping unknown fields.
pub const STATE_SCHEMA: u32 = 1;
/// Superseded versions retained per entry (password history / undo).
pub const HISTORY_LIMIT: usize = 20;

pub type DeviceId = Uuid;
pub type EntryId = Uuid;

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VersionVector(pub BTreeMap<DeviceId, u64>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Causality {
    Equal,
    /// `self` happened before `other` (other supersedes self).
    Before,
    /// `self` happened after `other` (self supersedes other).
    After,
    Concurrent,
}

impl VersionVector {
    pub fn get(&self, device: &DeviceId) -> u64 {
        self.0.get(device).copied().unwrap_or(0)
    }

    pub fn compare(&self, other: &VersionVector) -> Causality {
        let (mut less, mut greater) = (false, false);
        for d in self.0.keys().chain(other.0.keys()) {
            match self.get(d).cmp(&other.get(d)) {
                Ordering::Less => less = true,
                Ordering::Greater => greater = true,
                Ordering::Equal => {}
            }
        }
        match (less, greater) {
            (false, false) => Causality::Equal,
            (true, false) => Causality::Before,
            (false, true) => Causality::After,
            (true, true) => Causality::Concurrent,
        }
    }

    pub fn join(&self, other: &VersionVector) -> VersionVector {
        let mut out = self.0.clone();
        for (d, &c) in &other.0 {
            let e = out.entry(*d).or_insert(0);
            *e = (*e).max(c);
        }
        VersionVector(out)
    }

    pub fn increment(&mut self, device: DeviceId) {
        *self.0.entry(device).or_insert(0) += 1;
    }
}

/// The user-visible content of a login entry.
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Zeroize)]
pub struct EntryData {
    pub title: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub favorite: bool,
}

impl std::fmt::Debug for EntryData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EntryData")
            .field("title", &self.title)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .field("urls", &self.urls)
            .field("notes", &"<redacted>")
            .field("tags", &self.tags)
            .field("favorite", &self.favorite)
            .finish()
    }
}

impl EntryData {
    pub fn validate(&self) -> Result<()> {
        if self.title.trim().is_empty() {
            return Err(Error::InvalidInput("title must not be empty".into()));
        }
        Ok(())
    }
}

/// One immutable version of an entry.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Version {
    pub clock: VersionVector,
    /// Wall-clock time (ms since Unix epoch) on the authoring device. Used only
    /// for display and deterministic ordering, never for causality.
    pub modified_at: i64,
    pub modified_by: DeviceId,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub data: Option<EntryData>,
}

impl Version {
    /// Total, deterministic display order: newest first, then stable tie-breaks.
    fn display_cmp(&self, other: &Version) -> Ordering {
        other
            .modified_at
            .cmp(&self.modified_at)
            .then_with(|| self.modified_by.cmp(&other.modified_by))
            // Same author and timestamp: the causally later (larger) clock first.
            .then_with(|| other.clock.cmp(&self.clock))
            .then_with(|| self.deleted.cmp(&other.deleted))
            .then_with(|| self.data.cmp(&other.data))
    }

    /// The observable outcome of this version, ignoring causality metadata.
    fn outcome(&self) -> (bool, Option<&EntryData>) {
        (self.deleted, self.data.as_ref())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryRecord {
    pub id: EntryId,
    /// Concurrent versions not superseded by any other known version, in
    /// display order. Never empty.
    pub versions: Vec<Version>,
    /// Superseded versions, newest first, at most [`HISTORY_LIMIT`].
    #[serde(default)]
    pub history: Vec<Version>,
}

impl EntryRecord {
    /// The version shown by default (newest sibling).
    pub fn current(&self) -> &Version {
        &self.versions[0]
    }

    /// True when every concurrent version is a deletion.
    pub fn is_deleted(&self) -> bool {
        self.versions.iter().all(|v| v.deleted)
    }

    /// True when concurrent versions disagree and the user must choose.
    pub fn has_conflict(&self) -> bool {
        let first = self.versions[0].outcome();
        self.versions.iter().any(|v| v.outcome() != first)
    }

    /// The clock that dominates all current siblings.
    pub(crate) fn joined_clock(&self) -> VersionVector {
        self.versions
            .iter()
            .fold(VersionVector::default(), |acc, v| acc.join(&v.clock))
    }

    /// Create a local version superseding all current siblings.
    pub(crate) fn supersede(&mut self, device: DeviceId, now: i64, data: Option<EntryData>) {
        let mut clock = self.joined_clock();
        clock.increment(device);
        let deleted = data.is_none();
        let old = std::mem::take(&mut self.versions);
        self.history.extend(old.into_iter().filter(|v| !v.deleted));
        self.history = normalize_history(std::mem::take(&mut self.history));
        // Keep wall-clock order monotone per entry even if the device clock is behind.
        let floor = self.history.first().map(|v| v.modified_at).unwrap_or(i64::MIN);
        self.versions = vec![Version {
            clock,
            modified_at: now.max(floor),
            modified_by: device,
            deleted,
            data,
        }];
    }

    pub(crate) fn new_local(id: EntryId, device: DeviceId, now: i64, data: EntryData) -> EntryRecord {
        let mut clock = VersionVector::default();
        clock.increment(device);
        EntryRecord {
            id,
            versions: vec![Version {
                clock,
                modified_at: now,
                modified_by: device,
                deleted: false,
                data: Some(data),
            }],
            history: Vec::new(),
        }
    }

    /// CRDT merge of two records with the same id.
    pub fn merge(&self, other: &EntryRecord) -> EntryRecord {
        debug_assert_eq!(self.id, other.id);
        let versions = normalize_siblings(self.versions.iter().chain(&other.versions).cloned().collect());
        let history = normalize_history(self.history.iter().chain(&other.history).cloned().collect());
        EntryRecord {
            id: self.id,
            versions,
            history,
        }
    }

    /// Best-effort wipe of the record's secret content held in memory.
    fn wipe(&mut self) {
        for v in self.versions.iter_mut().chain(self.history.iter_mut()) {
            if let Some(d) = v.data.as_mut() {
                d.zeroize();
            }
        }
    }

    /// Apply a purge mark: drop the versions it covers and all history.
    /// Returns `None` when nothing survives.
    fn purged(&self, mark: &VersionVector) -> Option<EntryRecord> {
        let versions: Vec<Version> = self
            .versions
            .iter()
            .filter(|v| !matches!(v.clock.compare(mark), Causality::Before | Causality::Equal))
            .cloned()
            .collect();
        (!versions.is_empty()).then(|| EntryRecord {
            id: self.id,
            versions,
            history: Vec::new(),
        })
    }

    fn validate(&self) -> Result<()> {
        if self.versions.is_empty() {
            return Err(Error::Format("entry without versions".into()));
        }
        for v in &self.versions {
            if v.deleted != v.data.is_none() {
                return Err(Error::Format("inconsistent deletion marker".into()));
            }
        }
        Ok(())
    }
}

/// Keep only versions not strictly superseded by another, deduplicated, in
/// display order. This is the maximal-antichain merge, which is commutative,
/// associative and idempotent.
fn normalize_siblings(mut all: Vec<Version>) -> Vec<Version> {
    all.sort_by(Version::display_cmp);
    all.dedup();
    let keep: Vec<bool> = all
        .iter()
        .map(|v| !all.iter().any(|o| o.clock.compare(&v.clock) == Causality::After))
        .collect();
    all.into_iter().zip(keep).filter_map(|(v, k)| k.then_some(v)).collect()
}

fn normalize_history(mut all: Vec<Version>) -> Vec<Version> {
    all.sort_by(Version::display_cmp);
    all.dedup();
    all.truncate(HISTORY_LIMIT);
    all
}

/// The complete decrypted contents of a vault.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultState {
    pub schema: u32,
    #[serde(default)]
    pub entries: BTreeMap<EntryId, EntryRecord>,
    /// Purge marks: for each permanently purged entry, the clock up to which
    /// its versions are erased everywhere.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub purged: BTreeMap<EntryId, VersionVector>,
}

impl Default for VaultState {
    fn default() -> Self {
        VaultState {
            schema: STATE_SCHEMA,
            entries: BTreeMap::new(),
            purged: BTreeMap::new(),
        }
    }
}

impl VaultState {
    /// Validate a state decoded from storage or the network.
    pub fn validate(&self) -> Result<()> {
        if self.schema > STATE_SCHEMA || self.schema == 0 {
            return Err(Error::UnsupportedVersion {
                what: "vault data",
                found: self.schema,
                supported: STATE_SCHEMA,
            });
        }
        for (id, rec) in &self.entries {
            if *id != rec.id {
                return Err(Error::Format("entry id mismatch".into()));
            }
            rec.validate()?;
        }
        Ok(())
    }

    /// Merge `other` into `self`. Returns true if `self` changed.
    pub fn merge_from(&mut self, other: &VaultState) -> bool {
        let mut changed = false;
        let mut new_marks = Vec::new();
        for (id, mark) in &other.purged {
            let joined = match self.purged.get(id) {
                Some(ours) => ours.join(mark),
                None => mark.clone(),
            };
            if self.purged.get(id) != Some(&joined) {
                self.purged.insert(*id, joined);
                new_marks.push(*id);
                changed = true;
            }
        }
        for (id, theirs) in &other.entries {
            let merged = match self.entries.get(id) {
                Some(ours) => ours.merge(theirs),
                None => theirs.merge(theirs),
            };
            let merged = match self.purged.get(id) {
                Some(mark) => merged.purged(mark),
                None => Some(merged),
            };
            if self.entries.get(id) != merged.as_ref() {
                changed = true;
                match merged {
                    Some(m) => {
                        self.entries.insert(*id, m);
                    }
                    None => {
                        if let Some(mut gone) = self.entries.remove(id) {
                            gone.wipe();
                        }
                    }
                }
            }
        }
        for id in new_marks {
            self.enforce_purge(id);
        }
        changed
    }

    /// Record a purge of `id` up to `clock` and apply it.
    pub fn add_purge(&mut self, id: EntryId, clock: VersionVector) {
        let joined = match self.purged.get(&id) {
            Some(ours) => ours.join(&clock),
            None => clock,
        };
        self.purged.insert(id, joined);
        self.enforce_purge(id);
    }

    /// Re-apply the purge mark of `id`, if any (also after a local edit of a
    /// surviving entry, which must not grow its history). Returns true if the
    /// state changed.
    pub fn enforce_purge(&mut self, id: EntryId) -> bool {
        let (Some(mark), Some(rec)) = (self.purged.get(&id), self.entries.get(&id)) else {
            return false;
        };
        match rec.purged(mark) {
            None => {
                self.entries.remove(&id).expect("present").wipe();
                true
            }
            Some(kept) if &kept != rec => {
                self.entries.insert(id, kept);
                true
            }
            Some(_) => false,
        }
    }

    pub fn merged(&self, other: &VaultState) -> VaultState {
        let mut out = self.clone();
        out.merge_from(other);
        out
    }

    /// Best-effort wipe of all secret content held in memory.
    pub fn wipe(&mut self) {
        for rec in self.entries.values_mut() {
            rec.wipe();
        }
    }

    pub fn conflict_count(&self) -> usize {
        self.entries.values().filter(|r| r.has_conflict()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn data(title: &str, pw: &str) -> EntryData {
        EntryData {
            title: title.into(),
            password: pw.into(),
            ..Default::default()
        }
    }

    #[test]
    fn version_vector_compare() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut x = VersionVector::default();
        let mut y = VersionVector::default();
        assert_eq!(x.compare(&y), Causality::Equal);
        x.increment(a);
        assert_eq!(x.compare(&y), Causality::After);
        assert_eq!(y.compare(&x), Causality::Before);
        y.increment(b);
        assert_eq!(x.compare(&y), Causality::Concurrent);
        let j = x.join(&y);
        assert_eq!(j.compare(&x), Causality::After);
        assert_eq!(j.compare(&y), Causality::After);
    }

    #[test]
    fn sequential_edit_supersedes() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let id = Uuid::new_v4();
        let base = EntryRecord::new_local(id, a, 1, data("t", "p1"));
        let mut on_b = base.clone();
        on_b.supersede(b, 2, Some(data("t", "p2")));
        let merged = base.merge(&on_b);
        assert!(!merged.has_conflict());
        assert_eq!(merged.current().data.as_ref().unwrap().password, "p2");
        assert_eq!(merged.history.len(), 1);
        assert_eq!(merged.history[0].data.as_ref().unwrap().password, "p1");
    }

    #[test]
    fn concurrent_edits_are_both_preserved() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let id = Uuid::new_v4();
        let base = EntryRecord::new_local(id, a, 1, data("t", "p1"));
        let mut on_a = base.clone();
        on_a.supersede(a, 5, Some(data("t", "from-a")));
        let mut on_b = base.clone();
        on_b.supersede(b, 6, Some(data("t", "from-b")));

        let m1 = on_a.merge(&on_b);
        let m2 = on_b.merge(&on_a);
        assert_eq!(m1, m2);
        assert!(m1.has_conflict());
        let pws: Vec<_> = m1
            .versions
            .iter()
            .map(|v| v.data.as_ref().unwrap().password.clone())
            .collect();
        assert_eq!(pws, vec!["from-b", "from-a"]);

        // Resolving supersedes both siblings; the losing value stays in history.
        let mut resolved = m1.clone();
        resolved.supersede(a, 7, Some(data("t", "from-a")));
        assert!(!resolved.has_conflict());
        assert!(resolved
            .history
            .iter()
            .any(|v| v.data.as_ref().unwrap().password == "from-b"));
        // Merging stale conflicting copies cannot resurrect the conflict.
        assert_eq!(resolved.merge(&on_b).merge(&on_a), resolved);
    }

    #[test]
    fn edit_vs_delete_is_a_conflict() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let base = EntryRecord::new_local(Uuid::new_v4(), a, 1, data("t", "p1"));
        let mut del = base.clone();
        del.supersede(a, 2, None);
        let mut edit = base.clone();
        edit.supersede(b, 3, Some(data("t", "p2")));
        let m = del.merge(&edit);
        assert!(m.has_conflict());
        assert!(!m.is_deleted());
    }

    #[test]
    fn identical_concurrent_edits_are_not_a_conflict() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let base = EntryRecord::new_local(Uuid::new_v4(), a, 1, data("t", "p1"));
        let mut x = base.clone();
        x.supersede(a, 2, Some(data("t", "same")));
        let mut y = base.clone();
        y.supersede(b, 3, Some(data("t", "same")));
        let m = x.merge(&y);
        assert_eq!(m.versions.len(), 2);
        assert!(!m.has_conflict());
    }

    #[test]
    fn purge_reaches_every_copy_and_is_not_resurrected() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let id = Uuid::new_v4();
        let mut on_a = VaultState::default();
        on_a.entries
            .insert(id, EntryRecord::new_local(id, a, 1, data("t", "secret-1")));
        on_a.entries
            .get_mut(&id)
            .unwrap()
            .supersede(a, 2, Some(data("t", "secret-2")));
        on_a.entries.get_mut(&id).unwrap().supersede(a, 3, None);
        let stale_b = on_a.clone();

        let clock = on_a.entries[&id].joined_clock();
        on_a.add_purge(id, clock);
        assert!(on_a.entries.is_empty());
        // A device that still has the deleted entry (and its history) learns
        // of the purge, and its stale copy cannot bring it back.
        let mut on_b = stale_b.clone();
        assert!(on_b.merge_from(&on_a));
        assert!(on_b.entries.is_empty());
        assert!(!on_a.merge_from(&stale_b));
        assert!(on_a.entries.is_empty());
        let _ = b;
    }

    #[test]
    fn edit_concurrent_with_purge_survives_without_old_history() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let id = Uuid::new_v4();
        let mut base = VaultState::default();
        base.entries
            .insert(id, EntryRecord::new_local(id, a, 1, data("t", "old-secret")));
        base.entries.get_mut(&id).unwrap().supersede(a, 2, None);

        let mut on_a = base.clone();
        let clock = on_a.entries[&id].joined_clock();
        on_a.add_purge(id, clock);
        // Meanwhile b, which had not seen the purge, restores and edits it.
        let mut on_b = base.clone();
        on_b.entries
            .get_mut(&id)
            .unwrap()
            .supersede(b, 3, Some(data("t", "new-secret")));

        let merged = on_a.merged(&on_b);
        assert_eq!(merged, on_b.merged(&on_a));
        let rec = &merged.entries[&id];
        assert_eq!(rec.versions.len(), 1);
        assert_eq!(rec.current().data.as_ref().unwrap().password, "new-secret");
        assert!(rec.history.is_empty(), "the purged past is gone");
        assert!(!rec.has_conflict());
    }

    #[test]
    fn state_without_purges_serialises_as_before() {
        let s = VaultState::default();
        assert_eq!(serde_json::to_string(&s).unwrap(), r#"{"schema":1,"entries":{}}"#);
        let old: VaultState = serde_json::from_str(r#"{"schema":1,"entries":{}}"#).unwrap();
        assert!(old.purged.is_empty());
    }

    #[test]
    fn history_is_capped() {
        let a = Uuid::new_v4();
        let mut r = EntryRecord::new_local(Uuid::new_v4(), a, 0, data("t", "p0"));
        for i in 1..50 {
            r.supersede(a, i, Some(data("t", &format!("p{i}"))));
        }
        assert_eq!(r.history.len(), HISTORY_LIMIT);
        assert_eq!(r.history[0].data.as_ref().unwrap().password, "p48");
    }

    #[test]
    fn modified_at_is_monotone_despite_clock_skew() {
        let a = Uuid::new_v4();
        let mut r = EntryRecord::new_local(Uuid::new_v4(), a, 1000, data("t", "p0"));
        r.supersede(a, 10, Some(data("t", "p1")));
        assert!(r.current().modified_at >= 1000);
        assert_eq!(r.current().data.as_ref().unwrap().password, "p1");
    }

    #[test]
    fn validate_rejects_future_schema_and_bad_records() {
        let s = VaultState {
            schema: STATE_SCHEMA + 1,
            ..Default::default()
        };
        assert!(matches!(s.validate(), Err(Error::UnsupportedVersion { .. })));

        let mut s = VaultState::default();
        let rec = EntryRecord::new_local(Uuid::new_v4(), Uuid::new_v4(), 1, data("t", "p"));
        s.entries.insert(Uuid::new_v4(), rec);
        assert!(s.validate().is_err());
    }

    #[test]
    fn debug_redacts_secrets() {
        let d = data("Example", "synthetic-pw-should-not-print");
        assert!(!format!("{d:?}").contains("synthetic-pw"));
    }

    // ---- Property tests: random operations on three devices ----

    #[derive(Clone, Debug)]
    enum Op {
        Add(usize, u8),
        Edit(usize, u8, u8),
        Delete(usize, u8),
        Purge(usize, u8),
        Sync(usize, usize),
    }

    fn op_strategy() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0..3usize, 0..4u8).prop_map(|(d, e)| Op::Add(d, e)),
            (0..3usize, 0..4u8, any::<u8>()).prop_map(|(d, e, p)| Op::Edit(d, e, p)),
            (0..3usize, 0..4u8).prop_map(|(d, e)| Op::Delete(d, e)),
            (0..3usize, 0..4u8).prop_map(|(d, e)| Op::Purge(d, e)),
            (0..3usize, 0..3usize).prop_map(|(a, b)| Op::Sync(a, b)),
        ]
    }

    /// Simulate devices; returns final states plus every version ever written.
    fn simulate(ops: &[Op]) -> (Vec<VaultState>, Vec<(EntryId, Version)>) {
        let devices: Vec<DeviceId> = (0..3).map(|i| Uuid::from_u128(i + 1)).collect();
        let ids: Vec<EntryId> = (0..4).map(|i| Uuid::from_u128(100 + i)).collect();
        let mut states = vec![VaultState::default(); 3];
        let mut written = Vec::new();
        for (t, op) in ops.iter().enumerate() {
            let now = t as i64;
            let (d, e, new) = match *op {
                Op::Add(d, e) => {
                    // Real ids are random and never reused after a purge.
                    let id = ids[e as usize];
                    if !states[d].entries.contains_key(&id) && !states[d].purged.contains_key(&id) {
                        let rec = EntryRecord::new_local(ids[e as usize], devices[d], now, data("t", "init"));
                        written.push((rec.id, rec.versions[0].clone()));
                        states[d].entries.insert(rec.id, rec);
                    }
                    continue;
                }
                Op::Edit(d, e, p) => (d, e, Some(data("t", &format!("pw{p}")))),
                Op::Delete(d, e) => (d, e, None),
                Op::Purge(d, e) => {
                    let id = ids[e as usize];
                    if let Some(rec) = states[d].entries.get(&id) {
                        let clock = rec.joined_clock();
                        states[d].add_purge(id, clock);
                    }
                    continue;
                }
                Op::Sync(a, b) => {
                    let sb = states[b].clone();
                    states[a].merge_from(&sb);
                    continue;
                }
            };
            if let Some(rec) = states[d].entries.get_mut(&ids[e as usize]) {
                rec.supersede(devices[d], now, new);
                written.push((rec.id, rec.versions[0].clone()));
                states[d].enforce_purge(ids[e as usize]);
            }
        }
        (states, written)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        #[test]
        fn merge_is_commutative_associative_idempotent(ops in proptest::collection::vec(op_strategy(), 0..40)) {
            let (s, _) = simulate(&ops);
            prop_assert_eq!(s[0].merged(&s[1]), s[1].merged(&s[0]));
            prop_assert_eq!(s[0].merged(&s[1]).merged(&s[2]), s[0].merged(&s[1].merged(&s[2])));
            prop_assert_eq!(s[0].merged(&s[0]), s[0].clone());
            let m = s[0].merged(&s[1]);
            prop_assert_eq!(m.merged(&m), m.clone());
            prop_assert_eq!(m.merged(&s[0]), m.clone());
            // "changed" is exact: sync uploads only when something changed.
            for (a, b) in [(&s[0], &s[1]), (&m, &s[2]), (&m, &s[0])] {
                let mut x = a.clone();
                let changed = x.merge_from(b);
                prop_assert_eq!(changed, &x != a);
            }
        }

        #[test]
        fn no_update_is_lost(ops in proptest::collection::vec(op_strategy(), 0..40)) {
            let (s, written) = simulate(&ops);
            let all = s[0].merged(&s[1]).merged(&s[2]);
            // Every version ever written is either still a sibling, or strictly
            // superseded by a sibling (i.e. some device saw it before editing).
            for (id, v) in written {
                // Purged on purpose: covered by a purge mark.
                if all.purged.get(&id).is_some_and(|m| matches!(v.clock.compare(m), Causality::Before | Causality::Equal)) {
                    prop_assert!(all.entries.get(&id).is_none_or(|r| !r.versions.contains(&v) && r.history.is_empty()));
                    continue;
                }
                let rec = all.entries.get(&id).expect("entry lost");
                let kept = rec.versions.contains(&v)
                    || rec.versions.iter().any(|s| s.clock.compare(&v.clock) == Causality::After);
                prop_assert!(kept, "version lost: {:?}", v);
            }
        }

        #[test]
        fn devices_converge(ops in proptest::collection::vec(op_strategy(), 0..40)) {
            let (mut s, _) = simulate(&ops);
            // Gossip until everyone has seen everyone.
            for _ in 0..2 {
                for a in 0..3 { for b in 0..3 { let sb = s[b].clone(); s[a].merge_from(&sb); } }
            }
            prop_assert_eq!(&s[0], &s[1]);
            prop_assert_eq!(&s[1], &s[2]);
        }
    }
}
