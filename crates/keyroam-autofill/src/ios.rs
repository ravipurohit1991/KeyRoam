//! iOS AutoFill: which logins a Credential Provider request may see, and the
//! credential identities KeyRoam gives iOS for QuickType suggestions.
//!
//! iOS describes a request as a list of service identifiers. Each is either a
//! domain or a URL:
//!
//! * Safari and web views report the page's URL (or its domain).
//! * Apps are reported by the domains of their `webcredentials` associated
//!   domains, which Apple verifies against the site's
//!   `apple-app-site-association` file. So, unlike on Android, an app's
//!   identifier is a web domain it has proven it controls.
//!
//! Both are therefore matched like a web page on the desktop
//! ([`crate::origin`]): by registrable domain, never offering an https login
//! to an http page. A bare domain is read as https.

use std::collections::BTreeMap;

use keyroam_core::{EntryData, EntryId, EntryRecord, Error, Result, Vault};
use serde::{Deserialize, Serialize};

use crate::matching::{self, LoginMatch};
use crate::origin::WebOrigin;

/// Most logins listed for one request.
pub const MAX_LOGINS: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceKind {
    /// `ASCredentialServiceIdentifier.IdentifierType.domain`
    Domain,
    /// `ASCredentialServiceIdentifier.IdentifierType.URL`
    Url,
}

/// One `ASCredentialServiceIdentifier`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ServiceIdentifier {
    pub identifier: String,
    #[serde(rename = "type")]
    pub kind: ServiceKind,
}

impl ServiceIdentifier {
    pub fn domain(d: &str) -> Self {
        ServiceIdentifier {
            identifier: d.to_string(),
            kind: ServiceKind::Domain,
        }
    }

    pub fn url(u: &str) -> Self {
        ServiceIdentifier {
            identifier: u.to_string(),
            kind: ServiceKind::Url,
        }
    }
}

/// The page a service identifier names.
pub fn service_origin(s: &ServiceIdentifier) -> Result<WebOrigin> {
    let id = s.identifier.trim();
    let url = match s.kind {
        ServiceKind::Url => id.to_string(),
        ServiceKind::Domain => {
            if id.is_empty() || id.contains(['/', '?', '#', '@', ' ']) {
                return Err(Error::InvalidInput("not a domain".into()));
            }
            format!("https://{id}")
        }
    };
    WebOrigin::parse(&url).map_err(|e| Error::InvalidInput(format!("unusable service identifier: {e}")))
}

/// The usable origins of a request. Unusable identifiers are skipped; a
/// request with none is an error.
pub fn service_origins(services: &[ServiceIdentifier]) -> Result<Vec<WebOrigin>> {
    let origins: Vec<WebOrigin> = services.iter().filter_map(|s| service_origin(s).ok()).collect();
    if origins.is_empty() {
        return Err(Error::InvalidInput("no usable service identifier".into()));
    }
    Ok(origins)
}

/// Short text for the user: the site of the first identifier.
pub fn label(origins: &[WebOrigin]) -> String {
    origins
        .first()
        .map(|o| o.site().unwrap_or(o.host()).to_string())
        .unwrap_or_default()
}

/// Logins for any of `origins`, best matches first, each listed once.
pub fn find_logins(vault: &Vault, origins: &[WebOrigin]) -> Vec<LoginMatch> {
    let mut best: BTreeMap<EntryId, LoginMatch> = BTreeMap::new();
    for page in origins {
        for m in matching::find_logins(vault, page) {
            match best.get(&m.id) {
                Some(old) if old.kind <= m.kind => {}
                _ => {
                    best.insert(m.id, m);
                }
            }
        }
    }
    let mut out: Vec<LoginMatch> = best.into_values().collect();
    out.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
            .then_with(|| a.username.cmp(&b.username))
            .then_with(|| a.id.cmp(&b.id))
    });
    out.truncate(MAX_LOGINS);
    out
}

/// The entry's data, only if it is live and belongs to one of `origins`.
pub fn credential_for<'a>(vault: &'a Vault, id: EntryId, origins: &[WebOrigin]) -> Option<&'a EntryData> {
    origins
        .iter()
        .find_map(|page| matching::credential_for(vault, id, page))
}

/// A credential identity for `ASCredentialIdentityStore`: no secret, only
/// what iOS needs to suggest the login in the QuickType bar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Identity {
    /// The entry id (`recordIdentifier`).
    pub id: String,
    pub user: String,
    /// The host of a saved URL (a domain service identifier).
    pub domain: String,
}

fn live_data(rec: &EntryRecord) -> Option<&EntryData> {
    if rec.is_deleted() {
        return None;
    }
    rec.versions.iter().find_map(|v| v.data.as_ref())
}

/// One identity per live login with a password and per web host it is saved
/// for. iOS matches a domain identity against the page's domain and its
/// subdomains; KeyRoam re-checks the site when the login is requested.
pub fn identities(vault: &Vault) -> Vec<Identity> {
    let mut out = Vec::new();
    for rec in vault.entries() {
        let Some(data) = live_data(rec) else { continue };
        if data.password.is_empty() {
            continue;
        }
        let mut hosts: Vec<String> = data
            .urls
            .iter()
            .filter_map(|u| WebOrigin::parse_saved(u))
            .map(|o| o.host().to_string())
            .collect();
        hosts.sort();
        hosts.dedup();
        for domain in hosts {
            out.push(Identity {
                id: rec.id.to_string(),
                user: data.username.clone(),
                domain,
            });
        }
    }
    out.sort_by(|a, b| {
        a.domain
            .cmp(&b.domain)
            .then_with(|| a.user.cmp(&b.user))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

#[cfg(test)]
mod tests {
    use keyroam_core::KdfCost;

    use super::*;
    use crate::origin::MatchKind;

    const PW: &str = "synthetic-master-pw-1";

    fn vault() -> Vault {
        Vault::create(PW, KdfCost::MINIMUM).unwrap()
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
    fn service_identifiers() {
        let o = service_origin(&ServiceIdentifier::domain("accounts.example.com")).unwrap();
        assert_eq!(o.to_string(), "https://accounts.example.com");
        let o = service_origin(&ServiceIdentifier::url("http://intranet.example.org/login?x=1")).unwrap();
        assert_eq!(o.to_string(), "http://intranet.example.org");
        assert!(!o.is_secure());
        for bad in [
            ServiceIdentifier::domain(""),
            ServiceIdentifier::domain("example.com/path"),
            ServiceIdentifier::domain("user@example.com"),
            ServiceIdentifier::url("ftp://example.com"),
            ServiceIdentifier::url("not a url"),
        ] {
            assert!(service_origin(&bad).is_err(), "{bad:?}");
        }
        // Unusable identifiers are skipped as long as one is usable.
        let origins = service_origins(&[
            ServiceIdentifier::domain("bad/x"),
            ServiceIdentifier::domain("example.com"),
        ])
        .unwrap();
        assert_eq!(origins.len(), 1);
        assert_eq!(label(&origins), "example.com");
        assert!(service_origins(&[ServiceIdentifier::domain("")]).is_err());
        assert!(service_origins(&[]).is_err());
    }

    #[test]
    fn identifiers_deserialize_from_the_extension() {
        let s: Vec<ServiceIdentifier> = serde_json::from_str(
            r#"[{"identifier":"example.com","type":"domain"},{"identifier":"https://a.test/","type":"url"}]"#,
        )
        .unwrap();
        assert_eq!(
            s,
            vec![
                ServiceIdentifier::domain("example.com"),
                ServiceIdentifier::url("https://a.test/")
            ]
        );
    }

    #[test]
    fn logins_match_by_site_and_are_listed_once() {
        let mut v = vault();
        let exact = add(&mut v, "Example", "alice", "pw-a", &["https://accounts.example.com"]);
        let site = add(&mut v, "Example main", "bob", "pw-b", &["example.com"]);
        let both = add(
            &mut v,
            "Both",
            "carol",
            "pw-c",
            &["https://accounts.example.com", "https://example.com"],
        );
        add(&mut v, "Other", "dave", "pw-d", &["https://example.org"]);
        add(&mut v, "Look-alike", "eve", "pw-e", &["https://example.com.evil.test"]);
        let insecure = add(&mut v, "Old", "frank", "pw-f", &["http://accounts.example.com"]);
        add(&mut v, "Look-alike sub", "gina", "pw-g", &["https://example.co"]);

        // An app with two associated domains, both of the same site.
        let origins = service_origins(&[
            ServiceIdentifier::domain("accounts.example.com"),
            ServiceIdentifier::domain("example.com"),
        ])
        .unwrap();
        let found = find_logins(&v, &origins);
        let ids: Vec<EntryId> = found.iter().map(|m| m.id).collect();
        assert_eq!(ids.len(), 4, "{found:?}");
        assert!(ids.contains(&exact) && ids.contains(&site) && ids.contains(&both) && ids.contains(&insecure));
        // Each login keeps its best match; exact matches come first.
        assert!(found.iter().all(|m| m.kind == MatchKind::Exact));

        // A subdomain page sees the site's logins as same-site matches.
        let sub = service_origins(&[ServiceIdentifier::url("https://shop.example.com/cart")]).unwrap();
        let found = find_logins(&v, &sub);
        assert_eq!(found.len(), 4);
        assert!(found.iter().all(|m| m.kind != MatchKind::Exact));

        // An http page never sees https logins.
        let http = service_origins(&[ServiceIdentifier::url("http://accounts.example.com/")]).unwrap();
        let found: Vec<EntryId> = find_logins(&v, &http).iter().map(|m| m.id).collect();
        assert_eq!(found, vec![insecure]);

        assert_eq!(credential_for(&v, exact, &origins).unwrap().password, "pw-a");
        let other = service_origins(&[ServiceIdentifier::domain("example.org")]).unwrap();
        assert!(
            credential_for(&v, exact, &other).is_none(),
            "never released to another site"
        );

        v.delete_entry(exact).unwrap();
        assert!(
            credential_for(&v, exact, &origins).is_none(),
            "deleted logins are not offered"
        );
    }

    #[test]
    fn identities_name_hosts_without_secrets() {
        let mut v = vault();
        let a = add(
            &mut v,
            "Example",
            "alice",
            "pw-a",
            &[
                "https://accounts.example.com/login",
                "accounts.example.com",
                "https://example.org",
            ],
        );
        add(&mut v, "App only", "bob", "pw-b", &["androidapp://com.example.app"]);
        add(&mut v, "No password", "carol", "", &["https://example.net"]);
        let gone = add(&mut v, "Deleted", "dave", "pw-d", &["https://example.edu"]);
        v.delete_entry(gone).unwrap();

        let ids = identities(&v);
        assert_eq!(
            ids,
            vec![
                Identity {
                    id: a.to_string(),
                    user: "alice".into(),
                    domain: "accounts.example.com".into()
                },
                Identity {
                    id: a.to_string(),
                    user: "alice".into(),
                    domain: "example.org".into()
                },
            ]
        );
        let json = serde_json::to_string(&ids).unwrap();
        assert!(!json.contains("pw-"), "identities never carry passwords");
    }
}
