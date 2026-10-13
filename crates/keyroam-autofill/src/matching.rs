//! Vault queries for autofill: which logins belong to a page, and whether a
//! submitted login is new, a password update or already saved.

use keyroam_core::{EntryData, EntryId, EntryRecord, Error, Result, Vault};

use crate::origin::{MatchKind, WebOrigin};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginMatch {
    pub id: EntryId,
    pub title: String,
    pub username: String,
    pub kind: MatchKind,
}

/// What saving a submitted login would do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SavePlan {
    /// No saved login for this site has that username.
    New,
    /// A login for this site with that username has a different password.
    Update { id: EntryId, title: String },
    /// Already saved; nothing to ask.
    Unchanged { id: EntryId },
}

/// The content shown for an entry: its newest non-deleted version. Deleted
/// entries are never offered (a deleted-vs-edited conflict still is).
fn live_data(rec: &EntryRecord) -> Option<&EntryData> {
    if rec.is_deleted() {
        return None;
    }
    rec.versions.iter().find_map(|v| v.data.as_ref())
}

/// The best way any of the entry's URLs matches the page.
pub fn entry_match(data: &EntryData, page: &WebOrigin) -> Option<MatchKind> {
    data.urls
        .iter()
        .filter_map(|u| WebOrigin::parse_saved(u))
        .filter_map(|saved| saved.matches_page(page))
        .min()
}

/// Logins for `page`: exact host matches first, then other hosts of the same
/// site, each sorted by title and username.
pub fn find_logins(vault: &Vault, page: &WebOrigin) -> Vec<LoginMatch> {
    let mut out: Vec<LoginMatch> = vault
        .entries()
        .filter_map(|rec| {
            let data = live_data(rec)?;
            Some(LoginMatch {
                id: rec.id,
                title: data.title.clone(),
                username: data.username.clone(),
                kind: entry_match(data, page)?,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
            .then_with(|| a.username.cmp(&b.username))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// The entry's data, only if the entry is live and belongs to `page`.
pub fn credential_for<'a>(vault: &'a Vault, id: EntryId, page: &WebOrigin) -> Option<&'a EntryData> {
    let data = live_data(vault.get(id)?)?;
    entry_match(data, page)?;
    Some(data)
}

/// Decide whether a login submitted on `page` should be offered for saving.
pub fn plan_save(vault: &Vault, page: &WebOrigin, username: &str, password: &str) -> SavePlan {
    let username = username.trim();
    let mut update = None;
    for m in find_logins(vault, page) {
        if m.username != username {
            continue;
        }
        let data = credential_for(vault, m.id, page).expect("found above");
        if data.password == password {
            return SavePlan::Unchanged { id: m.id };
        }
        // Prefer an exact-host entry (find_logins lists those first).
        update.get_or_insert(SavePlan::Update {
            id: m.id,
            title: m.title,
        });
    }
    update.unwrap_or(SavePlan::New)
}

/// Save a login submitted on `page`: update the password of `update_id`
/// (which must belong to the page), or add a new entry for the page.
/// Returns the entry id and whether an existing entry was updated.
pub fn apply_save(
    vault: &mut Vault,
    page: &WebOrigin,
    username: &str,
    password: &str,
    update_id: Option<EntryId>,
) -> Result<(EntryId, bool)> {
    if password.is_empty() {
        return Err(Error::InvalidInput("the password is empty".into()));
    }
    let username = username.trim();
    if let Some(id) = update_id {
        let mut data = credential_for(vault, id, page).ok_or(Error::EntryNotFound)?.clone();
        data.password = password.to_string();
        if data.username.is_empty() {
            data.username = username.to_string();
        }
        vault.update_entry(id, data)?;
        return Ok((id, true));
    }
    let id = vault.add_entry(EntryData {
        title: page.site().unwrap_or(page.host()).to_string(),
        username: username.to_string(),
        password: password.to_string(),
        urls: vec![page.to_string()],
        ..Default::default()
    })?;
    Ok((id, false))
}

#[cfg(test)]
mod tests {
    use keyroam_core::KdfCost;

    use super::*;

    const PW: &str = "synthetic-master-pw-1";

    fn page(s: &str) -> WebOrigin {
        WebOrigin::parse(s).unwrap()
    }

    fn add(v: &mut Vault, title: &str, user: &str, pw: &str, urls: &[&str]) -> EntryId {
        v.add_entry(EntryData {
            title: title.into(),
            username: user.into(),
            password: pw.into(),
            urls: urls.iter().map(|u| u.to_string()).collect(),
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn finds_only_logins_for_the_site() {
        let mut v = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        let exact = add(&mut v, "Example login", "alice", "s1", &["https://login.example.com/"]);
        let site = add(&mut v, "Example", "bob", "s2", &["example.com"]);
        let _other = add(&mut v, "Other", "carol", "s3", &["https://example.org"]);
        let _lookalike = add(&mut v, "Phish", "dave", "s4", &["https://login.example.com.evil.test"]);
        let _no_url = add(&mut v, "Notes only", "erin", "s5", &[]);
        let deleted = add(&mut v, "Old", "frank", "s6", &["https://login.example.com"]);
        v.delete_entry(deleted).unwrap();

        let found = find_logins(&v, &page("https://login.example.com/signin"));
        let ids: Vec<_> = found.iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![exact, site]);
        assert_eq!(found[0].kind, MatchKind::Exact);
        assert_eq!(found[1].kind, MatchKind::SameSite);

        assert!(credential_for(&v, exact, &page("https://login.example.com")).is_some());
        // An entry is never released to a page of another site.
        assert!(credential_for(&v, exact, &page("https://example.org")).is_none());
        assert!(credential_for(&v, deleted, &page("https://login.example.com")).is_none());
        // Nor to the http version of its site.
        assert!(find_logins(&v, &page("http://login.example.com")).is_empty());
    }

    #[test]
    fn save_plans() {
        let mut v = Vault::create(PW, KdfCost::MINIMUM).unwrap();
        let id = add(&mut v, "Example", "alice", "old-pw", &["https://example.com"]);
        let p = page("https://www.example.com/login");
        assert_eq!(plan_save(&v, &p, "alice", "old-pw"), SavePlan::Unchanged { id });
        assert_eq!(
            plan_save(&v, &p, " alice ", "new-pw"),
            SavePlan::Update {
                id,
                title: "Example".into()
            }
        );
        assert_eq!(plan_save(&v, &p, "bob", "x"), SavePlan::New);
        assert_eq!(
            plan_save(&v, &page("https://example.net"), "alice", "old-pw"),
            SavePlan::New
        );

        let (updated, was_update) = apply_save(&mut v, &p, "alice", "new-pw", Some(id)).unwrap();
        assert!(was_update);
        assert_eq!(updated, id);
        let rec = v.get(id).unwrap();
        assert_eq!(rec.current().data.as_ref().unwrap().password, "new-pw");
        assert_eq!(rec.history[0].data.as_ref().unwrap().password, "old-pw");

        let (new_id, was_update) = apply_save(&mut v, &p, "bob", "bob-pw", None).unwrap();
        assert!(!was_update);
        let data = v.get(new_id).unwrap().current().data.clone().unwrap();
        assert_eq!(data.title, "example.com");
        assert_eq!(data.urls, vec!["https://www.example.com".to_string()]);
        assert_eq!(plan_save(&v, &p, "bob", "bob-pw"), SavePlan::Unchanged { id: new_id });

        // An update can only target an entry of the page's site.
        assert!(apply_save(&mut v, &page("https://example.org"), "alice", "x", Some(id)).is_err());
        assert!(apply_save(&mut v, &p, "alice", "", None).is_err());
    }
}
