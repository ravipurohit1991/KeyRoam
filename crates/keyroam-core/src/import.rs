//! Importing logins from the CSV exports of browsers and other password
//! managers: Chrome, Edge, Brave and other Chromium browsers, Firefox, Safari,
//! Bitwarden, LastPass, 1Password, KeePass and KeePassXC.
//!
//! Columns are recognised by their header names, so the column order does not
//! matter. Importing only adds entries: a login that is already in the vault
//! (same site, username and password), or repeated in the file, is skipped,
//! and existing entries are never changed.

use std::collections::HashSet;

use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::error::{Error, Result};
use crate::model::EntryData;
use crate::vault::Vault;

/// Larger files are refused; real exports are far smaller.
pub const MAX_CSV_BYTES: usize = 32 * 1024 * 1024;

/// Logins read from a CSV export.
pub struct CsvImport {
    /// The detected source, for display ("Chrome", "Firefox", … or "CSV").
    pub format: &'static str,
    pub logins: Vec<EntryData>,
    /// Rows without a username or password (or non-login Bitwarden items).
    pub skipped: usize,
}

impl Drop for CsvImport {
    fn drop(&mut self) {
        self.logins.iter_mut().for_each(Zeroize::zeroize);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportSummary {
    pub added: usize,
    /// Already in the vault, or repeated in the file.
    pub duplicates: usize,
}

/// Parse RFC 4180 CSV: quoted fields may hold commas, quotes ("") and line
/// breaks. A UTF-8 byte-order mark is ignored.
fn parse_rows(text: &str) -> Result<Vec<Vec<String>>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\r' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    if quoted {
        field.zeroize();
        return Err(Error::Format("the CSV file ends inside a quoted field".into()));
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows.retain(|r| !(r.len() == 1 && r[0].trim().is_empty()));
    Ok(rows)
}

fn normalize_header(h: &str) -> String {
    h.trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// Header aliases, most specific first.
const TITLE: &[&str] = &["name", "title", "itemname", "account"];
const URL: &[&str] = &[
    "url",
    "loginuri",
    "uri",
    "website",
    "websiteurl",
    "loginurl",
    "web",
    "address",
];
const USERNAME: &[&str] = &[
    "username",
    "loginusername",
    "user",
    "login",
    "userid",
    "loginname",
    "email",
    "emailaddress",
];
const PASSWORD: &[&str] = &["password", "loginpassword", "pass", "pwd"];
const NOTES: &[&str] = &["notes", "note", "extra", "comments", "comment"];
const TOTP: &[&str] = &["logintotp", "totp", "otpauth", "otp"];
const GROUP: &[&str] = &["folder", "grouping", "group", "tags", "category", "collections"];
const FAVORITE: &[&str] = &["favorite", "favourite", "fav"];
const TYPE: &[&str] = &["type"];

fn column(headers: &[String], aliases: &[&str]) -> Option<usize> {
    aliases.iter().find_map(|a| headers.iter().position(|h| h == a))
}

fn detect_format(headers: &[String]) -> &'static str {
    let has = |h: &str| headers.iter().any(|x| x == h);
    if has("httprealm") || has("formactionorigin") {
        "Firefox"
    } else if has("loginuri") {
        "Bitwarden"
    } else if has("grouping") {
        "LastPass"
    } else if has("archived") {
        "1Password"
    } else if has("group") && has("title") {
        "KeePass"
    } else if has("otpauth") {
        "Safari"
    } else if has("name") && has("url") && has("username") && has("password") {
        "Chrome"
    } else {
        "CSV"
    }
}

/// "https://www.Example.com:8443/login?x" → "example.com:8443".
fn site_key(url: &str) -> String {
    let u = url.trim().to_lowercase();
    let rest = u.split_once("://").map(|(_, r)| r).unwrap_or(&u);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once('@').map(|(_, h)| h).unwrap_or(host);
    host.strip_prefix("www.")
        .unwrap_or(host)
        .trim_end_matches('.')
        .to_string()
}

fn title_for(url: &str, username: &str) -> String {
    let site = site_key(url);
    let host = site.split(':').next().unwrap_or("");
    if !host.is_empty() {
        host.to_string()
    } else if !username.is_empty() {
        username.to_string()
    } else {
        "Imported login".into()
    }
}

/// Read the logins of a CSV export.
pub fn parse_csv(bytes: &[u8]) -> Result<CsvImport> {
    if bytes.len() > MAX_CSV_BYTES {
        return Err(Error::InvalidInput("the CSV file is too large".into()));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Format("the CSV file is not UTF-8 text".into()))?;
    let mut rows = parse_rows(text)?;
    let result = (|| {
        let Some((header_row, data)) = rows.split_first() else {
            return Err(Error::InvalidInput("the CSV file is empty".into()));
        };
        let headers: Vec<String> = header_row.iter().map(|h| normalize_header(h)).collect();
        let password_col = column(&headers, PASSWORD).ok_or_else(|| {
            Error::InvalidInput("no password column found; export your passwords as CSV from the browser".into())
        })?;
        let cols = (
            column(&headers, TITLE),
            column(&headers, URL),
            column(&headers, USERNAME),
            column(&headers, NOTES),
            column(&headers, TOTP),
            column(&headers, GROUP),
            column(&headers, FAVORITE),
            column(&headers, TYPE),
        );
        let (title_c, url_c, user_c, notes_c, totp_c, group_c, fav_c, type_c) = cols;
        let get = |row: &Vec<String>, c: Option<usize>| -> String {
            c.and_then(|i| row.get(i))
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        };

        let mut import = CsvImport {
            format: detect_format(&headers),
            logins: Vec::new(),
            skipped: 0,
        };
        for row in data {
            let kind = get(row, type_c).to_lowercase();
            if !kind.is_empty() && kind != "login" && type_c.is_some() && import.format == "Bitwarden" {
                import.skipped += 1;
                continue;
            }
            // Passwords keep their exact characters, including spaces.
            let password = row.get(password_col).cloned().unwrap_or_default();
            let username = get(row, user_c);
            if password.is_empty() && username.is_empty() {
                import.skipped += 1;
                continue;
            }
            let url = get(row, url_c);
            let mut title = get(row, title_c);
            if title.is_empty() {
                title = title_for(&url, &username);
            }
            let mut notes = get(row, notes_c);
            let totp = get(row, totp_c);
            if !totp.is_empty() {
                if !notes.is_empty() {
                    notes.push_str("\n\n");
                }
                notes.push_str("One-time password (TOTP) setup: ");
                notes.push_str(&totp);
            }
            let group = get(row, group_c);
            let tags = group
                .split([',', '/', '\\'])
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(String::from)
                .collect();
            let favorite = matches!(get(row, fav_c).to_lowercase().as_str(), "1" | "true" | "yes");
            import.logins.push(EntryData {
                title,
                username,
                password,
                urls: if url.is_empty() { Vec::new() } else { vec![url] },
                notes,
                tags,
                favorite,
            });
        }
        Ok(import)
    })();
    for row in rows.iter_mut() {
        row.iter_mut().for_each(Zeroize::zeroize);
    }
    result
}

/// Delete an imported plaintext export: overwrite its bytes with zeros, then
/// remove it. Best effort only: SSDs, backups and file-system snapshots can
/// still hold earlier copies.
pub fn remove_plaintext_file(path: &std::path::Path) -> Result<()> {
    use std::io::Write;
    let len = std::fs::metadata(path)?.len();
    {
        let mut f = std::fs::OpenOptions::new().write(true).open(path)?;
        let zeros = vec![0u8; 64 * 1024];
        let mut left = len;
        while left > 0 {
            let n = left.min(zeros.len() as u64) as usize;
            f.write_all(&zeros[..n])?;
            left -= n as u64;
        }
        f.sync_all()?;
    }
    std::fs::remove_file(path)?;
    Ok(())
}

impl Vault {
    /// Add imported logins, skipping any already in the vault (same site,
    /// username and password) or repeated among `logins`. Never changes an
    /// existing entry.
    pub fn import_logins(&mut self, logins: &[EntryData]) -> Result<ImportSummary> {
        // Compare digests so the set holds no plaintext passwords.
        let key = |d: &EntryData| -> [u8; 32] {
            let site = d.urls.first().map(|u| site_key(u)).unwrap_or_default();
            let mut h = Sha256::new();
            for part in [site.as_bytes(), d.username.as_bytes(), d.password.as_bytes()] {
                h.update((part.len() as u64).to_be_bytes());
                h.update(part);
            }
            h.finalize().into()
        };
        let mut seen: HashSet<[u8; 32]> = self
            .entries()
            .filter(|r| !r.is_deleted())
            .flat_map(|r| r.versions.iter().filter_map(|v| v.data.as_ref()))
            .map(key)
            .collect();
        let mut summary = ImportSummary::default();
        for data in logins {
            if !seen.insert(key(data)) {
                summary.duplicates += 1;
                continue;
            }
            self.add_entry(data.clone())?;
            summary.added += 1;
        }
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kdf::KdfCost;

    fn logins(csv: &str) -> CsvImport {
        parse_csv(csv.as_bytes()).unwrap()
    }

    #[test]
    fn chrome_edge_brave() {
        let csv = "name,url,username,password,note\r\n\
                   example.com,https://example.com/login,alice@example.test,synthetic-1,\r\n\
                   ,https://www.shop.test/,bob,\"pass, with \"\"quotes\"\"\",\"line 1\nline 2\"\r\n";
        let i = logins(csv);
        assert_eq!(i.format, "Chrome");
        assert_eq!(i.logins.len(), 2);
        assert_eq!(i.logins[0].title, "example.com");
        assert_eq!(i.logins[0].urls, vec!["https://example.com/login".to_string()]);
        assert_eq!(
            i.logins[1].title, "shop.test",
            "title from the URL when the name is empty"
        );
        assert_eq!(i.logins[1].password, "pass, with \"quotes\"");
        assert_eq!(i.logins[1].notes, "line 1\nline 2");
    }

    #[test]
    fn firefox() {
        let csv = "\u{feff}\"url\",\"username\",\"password\",\"httpRealm\",\"formActionOrigin\",\"guid\",\"timeCreated\",\"timeLastUsed\",\"timePasswordChanged\"\n\
                   \"https://accounts.example.test\",\"carol\",\"synthetic-ff\",,\"https://accounts.example.test\",\"{1}\",\"1\",\"2\",\"3\"\n";
        let i = logins(csv);
        assert_eq!(i.format, "Firefox");
        let d = &i.logins[0];
        assert_eq!(
            (d.title.as_str(), d.username.as_str(), d.password.as_str()),
            ("accounts.example.test", "carol", "synthetic-ff")
        );
    }

    #[test]
    fn safari_bitwarden_lastpass_keepass_1password() {
        let safari = logins(
            "Title,URL,Username,Password,Notes,OTPAuth\nMail,https://mail.test,dan,pw-s,,otpauth://totp/x?secret=ABC\n",
        );
        assert_eq!(safari.format, "Safari");
        assert!(safari.logins[0].notes.contains("otpauth://totp/x?secret=ABC"));

        let bw = logins(
            "folder,favorite,type,name,notes,fields,reprompt,login_uri,login_username,login_password,login_totp\n\
             Work,1,login,Intranet,,,0,https://intra.test,erin,pw-b,\n\
             ,,note,Secure note,text,,0,,,,\n",
        );
        assert_eq!(bw.format, "Bitwarden");
        assert_eq!(bw.logins.len(), 1);
        assert_eq!(bw.skipped, 1);
        let d = &bw.logins[0];
        assert_eq!(
            (d.title.as_str(), d.username.as_str(), d.password.as_str()),
            ("Intranet", "erin", "pw-b")
        );
        assert!(d.favorite);
        assert_eq!(d.tags, vec!["Work".to_string()]);

        let lp = logins(
            "url,username,password,totp,extra,name,grouping,fav\nhttps://lp.test,frank,pw-l,,memo,LP,Social,0\n",
        );
        assert_eq!(lp.format, "LastPass");
        assert_eq!(lp.logins[0].notes, "memo");

        let kp = logins("\"Group\",\"Title\",\"Username\",\"Password\",\"URL\",\"Notes\"\n\"Root/Web\",\"Forum\",\"gina\",\"pw-k\",\"forum.test\",\"\"\n");
        assert_eq!(kp.format, "KeePass");
        assert_eq!(kp.logins[0].tags, vec!["Root".to_string(), "Web".to_string()]);

        let op = logins("Title,Url,Username,Password,OTPAuth,Favorite,Archived,Tags,Notes\nBank,https://bank.test,hank,pw-1,,false,false,,\n");
        assert_eq!(op.format, "1Password");
        assert_eq!(op.logins[0].title, "Bank");
    }

    #[test]
    fn rejects_unusable_files() {
        assert!(parse_csv(b"").is_err());
        assert!(parse_csv(b"title,url,username\nx,y,z\n").is_err(), "no password column");
        assert!(parse_csv(b"name,password\n\"unterminated,x\n").is_err());
        assert!(parse_csv(&[0xff, 0xfe, 0x00]).is_err());
        let i = logins("url,username,password\nhttps://a.test,,\n\n");
        assert_eq!((i.logins.len(), i.skipped), (0, 1));
        // Passwords keep surrounding spaces.
        assert_eq!(
            logins("url,username,password\n,u, spaced \n").logins[0].password,
            " spaced "
        );
    }

    #[test]
    fn plaintext_export_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("passwords.csv");
        std::fs::write(
            &path,
            "name,url,username,password
x,y,z,synthetic
",
        )
        .unwrap();
        remove_plaintext_file(&path).unwrap();
        assert!(!path.exists());
        assert!(remove_plaintext_file(&path).is_err());
    }

    #[test]
    fn import_adds_only_new_logins() {
        let mut v = Vault::create("synthetic-master-pw-1", KdfCost::MINIMUM).unwrap();
        v.add_entry(EntryData {
            title: "Existing".into(),
            username: "alice".into(),
            password: "pw-1".into(),
            urls: vec!["https://www.example.com/".into()],
            ..Default::default()
        })
        .unwrap();
        let csv = "name,url,username,password,note\n\
                   a,https://example.com/login,alice,pw-1,\n\
                   b,https://example.com/login,alice,pw-2,\n\
                   b,https://example.com/other,alice,pw-2,\n\
                   c,https://other.test,alice,pw-1,\n";
        let i = logins(csv);
        let s = v.import_logins(&i.logins).unwrap();
        assert_eq!(
            s,
            ImportSummary {
                added: 2,
                duplicates: 2
            }
        );
        assert_eq!(v.entries().count(), 3);
        // Importing again changes nothing.
        assert_eq!(
            v.import_logins(&i.logins).unwrap(),
            ImportSummary {
                added: 0,
                duplicates: 4
            }
        );
    }
}
