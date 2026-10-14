//! Android autofill and Credential Manager: which app or site is asking,
//! which fields hold the login, and which logins it may see.
//!
//! The Android services (`app/android`) describe a request as the calling
//! package, the web domain reported by the view hierarchy (if any) and a list
//! of input fields. Everything that decides what is released lives here, so
//! it is shared with the desktop rules and covered by tests:
//!
//! * **Apps** are identified by package name. A login belongs to an app when
//!   one of its URLs is `androidapp://<package>` (KeyRoam's form) or
//!   `android://<hash>@<package>/` (the form used by Google Password
//!   Manager and Chrome exports).
//! * **Web pages** in a browser are identified by the web domain the browser
//!   reports. Any app can put a web domain into its views, so the domain is
//!   trusted only from the browsers in [`TRUSTED_BROWSERS`]; for every other
//!   app the request is treated as coming from that app. A page then matches
//!   by registrable domain exactly as on the desktop ([`crate::origin`]).

use keyroam_core::{EntryData, EntryId, EntryRecord, Error, Result, Vault};
use serde::Deserialize;

use crate::matching::{self, LoginMatch, SavePlan};
use crate::origin::{MatchKind, WebOrigin};
use crate::protocol::Secret;

/// URL scheme KeyRoam uses for logins saved from Android apps.
pub const APP_URL_PREFIX: &str = "androidapp://";

/// Browsers whose reported web domain is believed. Android's own autofill
/// compatibility list works the same way. A look-alike app can only use one
/// of these package names when the real browser is not installed.
pub const TRUSTED_BROWSERS: &[&str] = &[
    "com.android.chrome",
    "com.chrome.beta",
    "com.chrome.dev",
    "com.chrome.canary",
    "org.chromium.chrome",
    "com.google.android.apps.chrome",
    "org.mozilla.firefox",
    "org.mozilla.firefox_beta",
    "org.mozilla.fenix",
    "org.mozilla.fenix.nightly",
    "org.mozilla.focus",
    "org.mozilla.klar",
    "com.microsoft.emmx",
    "com.brave.browser",
    "com.brave.browser_beta",
    "com.sec.android.app.sbrowser",
    "com.sec.android.app.sbrowser.beta",
    "com.vivaldi.browser",
    "com.opera.browser",
    "com.opera.mini.native",
    "com.duckduckgo.mobile.android",
    "com.kiwibrowser.browser",
    "app.vanadium.browser",
    "com.ecosia.android",
];

/// Most logins offered in one fill response.
pub const MAX_DATASETS: usize = 20;

/// Who is asking for a login.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A web page shown by a trusted browser.
    Web(WebOrigin),
    /// A native app, by package name.
    App(String),
}

impl Target {
    /// Short text for the user: the site, or the package name.
    pub fn label(&self) -> String {
        match self {
            Target::Web(o) => o.site().unwrap_or(o.host()).to_string(),
            Target::App(p) => p.clone(),
        }
    }

    /// The URL stored in a new entry saved for this target.
    pub fn saved_url(&self) -> String {
        match self {
            Target::Web(o) => o.to_string(),
            Target::App(p) => format!("{APP_URL_PREFIX}{p}"),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Target::Web(_) => "web",
            Target::App(_) => "app",
        }
    }
}

/// Is `s` a syntactically valid Android package name?
pub fn is_package_name(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    s.len() <= 255
        && parts.len() >= 2
        && parts.iter().all(|p| {
            let mut chars = p.chars();
            chars.next().is_some_and(|c| c.is_ascii_alphabetic())
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// Decide who is asking. `web_domain` and `web_scheme` come from the view
/// structure and are used only when `package` is a trusted browser.
pub fn resolve_target(package: &str, web_domain: Option<&str>, web_scheme: Option<&str>) -> Result<Target> {
    if !is_package_name(package) {
        return Err(Error::InvalidInput("not an Android package name".into()));
    }
    let domain = web_domain.map(str::trim).filter(|d| !d.is_empty());
    match domain {
        Some(d) if TRUSTED_BROWSERS.contains(&package) => {
            let scheme = match web_scheme.map(str::trim) {
                Some("http") => "http",
                _ => "https",
            };
            WebOrigin::parse(&format!("{scheme}://{d}"))
                .map(Target::Web)
                .map_err(|e| Error::InvalidInput(format!("unusable web domain: {e}")))
        }
        _ => Ok(Target::App(package.to_string())),
    }
}

/// The package a saved URL names, for `androidapp://pkg` and
/// `android://<hash>@pkg/` URLs.
pub fn saved_package(url: &str) -> Option<&str> {
    let url = url.trim();
    let rest = if let Some(r) = url.strip_prefix(APP_URL_PREFIX) {
        r
    } else {
        let r = url.strip_prefix("android://")?;
        r.split_once('@').map(|(_, p)| p)?
    };
    let pkg = rest.split(['/', '?', '#']).next()?;
    is_package_name(pkg).then_some(pkg)
}

fn live_data(rec: &EntryRecord) -> Option<&EntryData> {
    if rec.is_deleted() {
        return None;
    }
    rec.versions.iter().find_map(|v| v.data.as_ref())
}

fn app_match(data: &EntryData, package: &str) -> bool {
    data.urls.iter().any(|u| saved_package(u) == Some(package))
}

/// Logins for `target`, best matches first.
pub fn find_logins(vault: &Vault, target: &Target) -> Vec<LoginMatch> {
    match target {
        Target::Web(page) => matching::find_logins(vault, page),
        Target::App(package) => {
            let mut out: Vec<LoginMatch> = vault
                .entries()
                .filter_map(|rec| {
                    let data = live_data(rec)?;
                    app_match(data, package).then(|| LoginMatch {
                        id: rec.id,
                        title: data.title.clone(),
                        username: data.username.clone(),
                        kind: MatchKind::Exact,
                    })
                })
                .collect();
            out.sort_by(|a, b| {
                a.title
                    .to_lowercase()
                    .cmp(&b.title.to_lowercase())
                    .then_with(|| a.username.cmp(&b.username))
                    .then_with(|| a.id.cmp(&b.id))
            });
            out
        }
    }
}

/// The entry's data, only if it is live and belongs to `target`.
pub fn credential_for<'a>(vault: &'a Vault, id: EntryId, target: &Target) -> Option<&'a EntryData> {
    match target {
        Target::Web(page) => matching::credential_for(vault, id, page),
        Target::App(package) => live_data(vault.get(id)?).filter(|d| app_match(d, package)),
    }
}

/// Whether a submitted login is new, an update or already saved.
pub fn plan_save(vault: &Vault, target: &Target, username: &str, password: &str) -> SavePlan {
    if let Target::Web(page) = target {
        return matching::plan_save(vault, page, username, password);
    }
    let username = username.trim();
    let mut update = None;
    for m in find_logins(vault, target) {
        if m.username != username {
            continue;
        }
        let data = credential_for(vault, m.id, target).expect("found above");
        if data.password == password {
            return SavePlan::Unchanged { id: m.id };
        }
        update.get_or_insert(SavePlan::Update {
            id: m.id,
            title: m.title,
        });
    }
    update.unwrap_or(SavePlan::New)
}

/// Save a login for `target`: update `update_id` (which must belong to the
/// target) or add a new entry titled `title` (or the target's label).
pub fn apply_save(
    vault: &mut Vault,
    target: &Target,
    username: &str,
    password: &str,
    update_id: Option<EntryId>,
    title: Option<&str>,
) -> Result<(EntryId, bool)> {
    if password.is_empty() {
        return Err(Error::InvalidInput("the password is empty".into()));
    }
    let username = username.trim();
    if let Some(id) = update_id {
        let mut data = credential_for(vault, id, target).ok_or(Error::EntryNotFound)?.clone();
        data.password = password.to_string();
        if data.username.is_empty() {
            data.username = username.to_string();
        }
        vault.update_entry(id, data)?;
        return Ok((id, true));
    }
    let title = title.map(str::trim).filter(|t| !t.is_empty()).map(String::from);
    let id = vault.add_entry(EntryData {
        title: title.unwrap_or_else(|| target.label()),
        username: username.to_string(),
        password: password.to_string(),
        urls: vec![target.saved_url()],
        ..Default::default()
    })?;
    Ok((id, false))
}

// ----- fields -----

/// One input field of the screen, as described by the Android service.
#[derive(Debug, Default, Deserialize)]
pub struct Field {
    /// `View.getAutofillHints()`.
    #[serde(default)]
    pub hints: Vec<String>,
    /// `View.getInputType()` bits.
    #[serde(default)]
    pub input_type: u32,
    /// HTML `type` attribute (web content).
    #[serde(default)]
    pub html_type: String,
    /// HTML `name` and `id` attributes, and `autocomplete`.
    #[serde(default)]
    pub html_name: String,
    #[serde(default)]
    pub html_autocomplete: String,
    /// Resource id entry name (`R.id.<entry>`).
    #[serde(default)]
    pub id_entry: String,
    /// Hint text shown in the empty field.
    #[serde(default)]
    pub hint: String,
    /// Current text, sent only with save requests.
    #[serde(default)]
    pub value: Option<Secret>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Username,
    Password,
    NewPassword,
    Other,
}

// android.text.InputType
const TYPE_MASK_CLASS: u32 = 0x0f;
const TYPE_MASK_VARIATION: u32 = 0xff0;
const TYPE_CLASS_TEXT: u32 = 0x01;
const TYPE_CLASS_NUMBER: u32 = 0x02;
const TEXT_VARIATION_EMAIL_ADDRESS: u32 = 0x20;
const TEXT_VARIATION_PASSWORD: u32 = 0x80;
const TEXT_VARIATION_VISIBLE_PASSWORD: u32 = 0x90;
const TEXT_VARIATION_WEB_EMAIL_ADDRESS: u32 = 0xd0;
const TEXT_VARIATION_WEB_PASSWORD: u32 = 0xe0;
const NUMBER_VARIATION_PASSWORD: u32 = 0x10;

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn hint_kind(hint: &str) -> Option<FieldKind> {
    match norm(hint).as_str() {
        "newpassword" => Some(FieldKind::NewPassword),
        "password" | "currentpassword" => Some(FieldKind::Password),
        "username" | "emailaddress" | "email" | "newusername" | "phone" | "phonenumber" => Some(FieldKind::Username),
        "onetimecode" | "smsotpcode" | "otp" | "off" | "no" => Some(FieldKind::Other),
        _ => None,
    }
}

fn keyword_kind(text: &str) -> Option<FieldKind> {
    let t = norm(text);
    if t.is_empty() {
        return None;
    }
    let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
    if has(&["search", "otp", "onetime", "verification", "captcha", "promo", "coupon"]) {
        return Some(FieldKind::Other);
    }
    if has(&["newpass", "confirmpass", "repeatpass", "retypepass"]) {
        return Some(FieldKind::NewPassword);
    }
    if has(&[
        "password",
        "passwort",
        "passwd",
        "pwd",
        "contrasena",
        "motdepasse",
        "pincode",
    ]) {
        return Some(FieldKind::Password);
    }
    if has(&[
        "user",
        "login",
        "email",
        "mail",
        "account",
        "phone",
        "mobile",
        "identifier",
        "benutzer",
    ]) {
        return Some(FieldKind::Username);
    }
    None
}

/// What kind of login field `f` is.
pub fn field_kind(f: &Field) -> FieldKind {
    // Explicit hints from the app or page win.
    let explicit = f
        .hints
        .iter()
        .map(String::as_str)
        .chain(f.html_autocomplete.split_whitespace())
        .filter_map(hint_kind)
        .next();
    if let Some(k) = explicit {
        return k;
    }
    match f.html_type.trim().to_ascii_lowercase().as_str() {
        "password" => return FieldKind::Password,
        "email" => return FieldKind::Username,
        "hidden" | "submit" | "button" | "checkbox" | "radio" | "search" => return FieldKind::Other,
        _ => {}
    }
    let class = f.input_type & TYPE_MASK_CLASS;
    let variation = f.input_type & TYPE_MASK_VARIATION;
    if class == TYPE_CLASS_TEXT {
        match variation {
            TEXT_VARIATION_PASSWORD | TEXT_VARIATION_VISIBLE_PASSWORD | TEXT_VARIATION_WEB_PASSWORD => {
                return keyword_kind(&format!("{} {}", f.id_entry, f.html_name))
                    .filter(|k| *k == FieldKind::NewPassword)
                    .unwrap_or(FieldKind::Password)
            }
            TEXT_VARIATION_EMAIL_ADDRESS | TEXT_VARIATION_WEB_EMAIL_ADDRESS => return FieldKind::Username,
            _ => {}
        }
    }
    if class == TYPE_CLASS_NUMBER && variation == NUMBER_VARIATION_PASSWORD {
        return FieldKind::Password;
    }
    [f.id_entry.as_str(), f.html_name.as_str(), f.hint.as_str()]
        .into_iter()
        .find_map(keyword_kind)
        .unwrap_or(FieldKind::Other)
}

fn is_plain_text(f: &Field) -> bool {
    let html = f.html_type.trim().to_ascii_lowercase();
    if !html.is_empty() {
        return matches!(html.as_str(), "text" | "email" | "tel");
    }
    f.input_type & TYPE_MASK_CLASS == TYPE_CLASS_TEXT
}

/// Which fields to fill (indexes into the list the service sent).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LoginFields {
    pub username: Option<usize>,
    pub password: Option<usize>,
    /// On sign-up and change-password screens: the new password.
    pub new_password: Option<usize>,
}

impl LoginFields {
    pub fn is_empty(&self) -> bool {
        self.username.is_none() && self.password.is_none() && self.new_password.is_none()
    }
}

/// Find the username and password fields of a screen.
pub fn classify(fields: &[Field]) -> LoginFields {
    let kinds: Vec<FieldKind> = fields.iter().map(field_kind).collect();
    let first = |k: FieldKind| kinds.iter().position(|x| *x == k);
    let password = first(FieldKind::Password);
    let new_password = kinds.iter().rposition(|x| *x == FieldKind::NewPassword);
    let anchor = password.or(new_password);
    let usernames: Vec<usize> = (0..kinds.len()).filter(|&i| kinds[i] == FieldKind::Username).collect();
    let username = match anchor {
        Some(p) => usernames
            .iter()
            .copied()
            .rev()
            .find(|&i| i < p)
            .or_else(|| usernames.first().copied())
            // No marked username: the nearest plain text field before the password.
            .or_else(|| {
                (0..p)
                    .rev()
                    .find(|&i| kinds[i] == FieldKind::Other && is_plain_text(&fields[i]))
            }),
        // A username-only first step (e.g. "Next" before the password page).
        None => usernames.first().copied(),
    };
    LoginFields {
        username,
        password,
        new_password,
    }
}

/// The username and password the user typed, for a save request. On a
/// change-password screen the new password is the one to keep.
pub fn submitted(fields: &[Field]) -> Option<(String, String)> {
    let found = classify(fields);
    let value = |i: Option<usize>| {
        i.and_then(|i| fields[i].value.as_ref())
            .map(|v| v.expose().to_string())
            .filter(|v| !v.is_empty())
    };
    let password = value(found.new_password).or_else(|| value(found.password))?;
    let username = value(found.username).unwrap_or_default();
    Some((username.trim().to_string(), password))
}

#[cfg(test)]
mod tests {
    use keyroam_core::KdfCost;

    use super::*;

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

    fn field(input_type: u32) -> Field {
        Field {
            input_type,
            ..Default::default()
        }
    }

    fn with_value(mut f: Field, v: &str) -> Field {
        f.value = Some(Secret::new(v));
        f
    }

    #[test]
    fn package_names() {
        assert!(is_package_name("com.example.app"));
        assert!(is_package_name("org.mozilla.firefox_beta"));
        for bad in [
            "",
            "example",
            "com..app",
            "1com.app",
            "com.app/../x",
            "com.ex ample",
            "com.app.",
        ] {
            assert!(!is_package_name(bad), "{bad}");
        }
    }

    #[test]
    fn web_domains_are_trusted_only_from_browsers() {
        let t = resolve_target("com.android.chrome", Some("login.example.co.uk"), Some("https")).unwrap();
        assert_eq!(t.label(), "example.co.uk");
        assert_eq!(t.kind(), "web");
        let http = resolve_target("org.mozilla.firefox", Some("router.local"), Some("http")).unwrap();
        assert_eq!(http.saved_url(), "http://router.local");
        // Missing scheme: assume https.
        let t = resolve_target("com.brave.browser", Some("example.com"), None).unwrap();
        assert_eq!(t.saved_url(), "https://example.com");

        // Any other app claiming a web domain is treated as itself.
        let t = resolve_target("com.evil.phish", Some("bank.example"), Some("https")).unwrap();
        assert_eq!(t, Target::App("com.evil.phish".into()));
        // A browser without a web domain (its own UI) is an app.
        let t = resolve_target("com.android.chrome", None, None).unwrap();
        assert_eq!(t, Target::App("com.android.chrome".into()));
        assert!(resolve_target("not a package", None, None).is_err());
        assert!(resolve_target("com.android.chrome", Some("exa mple.com"), None).is_err());
    }

    #[test]
    fn saved_app_urls() {
        assert_eq!(saved_package("androidapp://com.example.app"), Some("com.example.app"));
        assert_eq!(
            saved_package(" androidapp://com.example.app/ "),
            Some("com.example.app")
        );
        assert_eq!(
            saved_package("android://Ab-Cd_eF1234==@com.example.app/"),
            Some("com.example.app")
        );
        assert_eq!(saved_package("https://example.com"), None);
        assert_eq!(saved_package("android://com.example.app"), None);
        assert_eq!(saved_package("androidapp://not-a-package"), None);
    }

    #[test]
    fn apps_see_only_their_own_logins() {
        let mut v = vault();
        let app = add(
            &mut v,
            "Example app",
            "alice",
            "synthetic-1",
            &["androidapp://com.example.app"],
        );
        let gpm = add(
            &mut v,
            "From GPM",
            "bob",
            "synthetic-2",
            &["android://hash@com.example.app/"],
        );
        add(&mut v, "Web", "carol", "synthetic-3", &["https://example.com"]);
        add(
            &mut v,
            "Other app",
            "dave",
            "synthetic-4",
            &["androidapp://com.other.app"],
        );

        let target = Target::App("com.example.app".into());
        let found: Vec<EntryId> = find_logins(&v, &target).into_iter().map(|m| m.id).collect();
        assert_eq!(found, [app, gpm]);
        assert!(credential_for(&v, app, &target).is_some());
        // An app cannot read a website's login, nor another app's.
        let web = find_logins(&v, &Target::Web(WebOrigin::parse("https://example.com").unwrap()));
        assert_eq!(web.len(), 1);
        assert!(credential_for(&v, web[0].id, &target).is_none());
        assert!(find_logins(&v, &Target::App("com.evil.phish".into())).is_empty());
    }

    #[test]
    fn browsers_match_by_site() {
        let mut v = vault();
        add(&mut v, "Example", "alice", "synthetic-1", &["https://www.example.com"]);
        add(
            &mut v,
            "Example app",
            "bob",
            "synthetic-2",
            &["androidapp://com.example.app"],
        );
        let t = resolve_target("com.android.chrome", Some("login.example.com"), None).unwrap();
        let found = find_logins(&v, &t);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].username, "alice");
        // https logins are never offered on an http page.
        let t = resolve_target("com.android.chrome", Some("www.example.com"), Some("http")).unwrap();
        assert!(find_logins(&v, &t).is_empty());
    }

    #[test]
    fn saving_for_apps() {
        let mut v = vault();
        let target = Target::App("com.example.app".into());
        assert_eq!(plan_save(&v, &target, "alice", "synthetic-1"), SavePlan::New);
        let (id, updated) = apply_save(&mut v, &target, " alice ", "synthetic-1", None, Some("Example")).unwrap();
        assert!(!updated);
        let data = v.get(id).unwrap().current().data.clone().unwrap();
        assert_eq!(data.title, "Example");
        assert_eq!(data.username, "alice");
        assert_eq!(data.urls, ["androidapp://com.example.app"]);

        assert_eq!(
            plan_save(&v, &target, "alice", "synthetic-1"),
            SavePlan::Unchanged { id }
        );
        let plan = plan_save(&v, &target, "alice", "synthetic-2");
        assert_eq!(
            plan,
            SavePlan::Update {
                id,
                title: "Example".into()
            }
        );
        apply_save(&mut v, &target, "alice", "synthetic-2", Some(id), None).unwrap();
        assert_eq!(
            v.get(id).unwrap().current().data.as_ref().unwrap().password,
            "synthetic-2"
        );

        // Another app cannot update it, and an untitled save uses the package.
        let other = Target::App("com.other.app".into());
        assert!(apply_save(&mut v, &other, "alice", "x-synthetic", Some(id), None).is_err());
        let (id2, _) = apply_save(&mut v, &other, "bob", "synthetic-3", None, None).unwrap();
        assert_eq!(
            v.get(id2).unwrap().current().data.as_ref().unwrap().title,
            "com.other.app"
        );
        assert!(apply_save(&mut v, &other, "bob", "", None, None).is_err());
    }

    #[test]
    fn field_kinds() {
        let hinted = |h: &str| Field {
            hints: vec![h.into()],
            ..Default::default()
        };
        assert_eq!(field_kind(&hinted("username")), FieldKind::Username);
        assert_eq!(field_kind(&hinted("emailAddress")), FieldKind::Username);
        assert_eq!(field_kind(&hinted("password")), FieldKind::Password);
        assert_eq!(field_kind(&hinted("newPassword")), FieldKind::NewPassword);
        assert_eq!(field_kind(&hinted("smsOTPCode")), FieldKind::Other);

        assert_eq!(field_kind(&field(0x81)), FieldKind::Password); // text|password
        assert_eq!(field_kind(&field(0xe1)), FieldKind::Password); // text|web password
        assert_eq!(field_kind(&field(0x21)), FieldKind::Username); // text|email
        assert_eq!(field_kind(&field(0x12)), FieldKind::Password); // number|password (PIN)
        assert_eq!(field_kind(&field(0x01)), FieldKind::Other); // plain text
        assert_eq!(field_kind(&field(0x02)), FieldKind::Other); // plain number

        let web = |t: &str, name: &str, ac: &str| Field {
            html_type: t.into(),
            html_name: name.into(),
            html_autocomplete: ac.into(),
            ..Default::default()
        };
        assert_eq!(field_kind(&web("password", "pw", "")), FieldKind::Password);
        assert_eq!(
            field_kind(&web("password", "pw", "new-password")),
            FieldKind::NewPassword
        );
        assert_eq!(field_kind(&web("text", "q", "username")), FieldKind::Username);
        assert_eq!(field_kind(&web("text", "login_email", "")), FieldKind::Username);
        assert_eq!(field_kind(&web("hidden", "user", "")), FieldKind::Other);
        assert_eq!(field_kind(&web("text", "search", "")), FieldKind::Other);

        let ids = |id: &str| Field {
            input_type: 0x01,
            id_entry: id.into(),
            ..Default::default()
        };
        assert_eq!(field_kind(&ids("et_username")), FieldKind::Username);
        assert_eq!(field_kind(&ids("otp_code")), FieldKind::Other);
        let pw_new = Field {
            input_type: 0x81,
            id_entry: "confirm_password".into(),
            ..Default::default()
        };
        assert_eq!(field_kind(&pw_new), FieldKind::NewPassword);
    }

    #[test]
    fn classify_login_screens() {
        // Plain app login: unmarked text field, then a password field.
        let f = [field(0x01), field(0x01), field(0x81)];
        assert_eq!(
            classify(&f),
            LoginFields {
                username: Some(1),
                password: Some(2),
                new_password: None
            }
        );
        // Email field after an unrelated field.
        let f = [field(0x01), field(0x21), field(0x81)];
        assert_eq!(classify(&f).username, Some(1));
        // Username-only first step.
        let f = [field(0x21)];
        assert_eq!(
            classify(&f),
            LoginFields {
                username: Some(0),
                ..Default::default()
            }
        );
        // Nothing to fill.
        assert!(classify(&[field(0x01), field(0x02)]).is_empty());
        assert!(classify(&[]).is_empty());
    }

    #[test]
    fn submitted_logins() {
        let f = [
            with_value(field(0x21), " alice@example.test "),
            with_value(field(0x81), "synthetic-1"),
        ];
        assert_eq!(submitted(&f), Some(("alice@example.test".into(), "synthetic-1".into())));
        // Change password: current, new; the new one is saved.
        let new_pw = |v: &str| {
            with_value(
                Field {
                    hints: vec!["newPassword".into()],
                    ..Default::default()
                },
                v,
            )
        };
        let f = [
            with_value(field(0x21), "alice"),
            with_value(field(0x81), "synthetic-old"),
            new_pw("synthetic-new"),
        ];
        assert_eq!(submitted(&f).unwrap().1, "synthetic-new");
        // No password typed: nothing to save.
        assert_eq!(submitted(&[with_value(field(0x21), "alice"), field(0x81)]), None);
        assert_eq!(submitted(&[with_value(field(0x81), "synthetic-1")]).unwrap().0, "");
    }
}
