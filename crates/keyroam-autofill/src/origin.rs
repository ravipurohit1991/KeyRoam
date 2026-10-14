//! Origin matching for autofill.
//!
//! A page is identified by its *origin* (scheme, host, port) and its *site*:
//! the registrable domain from the Public Suffix List ("eTLD+1"). A saved login
//! is offered on a page when one of its URLs belongs to the same site:
//!
//! * `login.example.co.uk` and `www.example.co.uk` share the site
//!   `example.co.uk`, so they match each other.
//! * `alice.github.io` and `bob.github.io` are different sites, because
//!   `github.io` is itself a public suffix.
//! * Hosts without a registrable domain (IP addresses, `localhost`, or a
//!   public suffix itself) only match exactly.
//!
//! An entry URL with an explicit `https://` scheme is never offered on a plain
//! `http://` page (a downgrade). An entry URL with an explicit non-default port
//! only matches that port. Only `http` and `https` pages can be filled at all.

use std::fmt;

use url::{Host, Url};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scheme {
    Http,
    Https,
}

impl Scheme {
    fn as_str(self) -> &'static str {
        match self {
            Scheme::Http => "http",
            Scheme::Https => "https",
        }
    }
}

/// Why a URL cannot be used for autofill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OriginError {
    /// Not a URL at all.
    Invalid,
    /// A URL whose scheme is not `http` or `https` (`file:`, `chrome:`, `about:`…).
    UnsupportedScheme(String),
    /// A URL without a host.
    NoHost,
}

impl fmt::Display for OriginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OriginError::Invalid => write!(f, "not a valid URL"),
            OriginError::UnsupportedScheme(s) => write!(f, "KeyRoam does not fill on {s}: pages"),
            OriginError::NoHost => write!(f, "the URL has no host"),
        }
    }
}

impl std::error::Error for OriginError {}

/// The origin and site of a web page (or frame).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebOrigin {
    pub scheme: Scheme,
    /// Lower-case ASCII host (internationalised names in punycode), without a
    /// trailing dot. IPv6 addresses are bracketed.
    pub host: String,
    /// Effective port (the scheme default when the URL has none).
    pub port: u16,
    /// Registrable domain, or the host itself when it has none.
    pub site: String,
}

impl WebOrigin {
    pub fn parse(url: &str) -> Result<WebOrigin, OriginError> {
        let url = Url::parse(url.trim()).map_err(|_| OriginError::Invalid)?;
        Self::from_url(&url)
    }

    fn from_url(url: &Url) -> Result<WebOrigin, OriginError> {
        let scheme = match url.scheme() {
            "https" => Scheme::Https,
            "http" => Scheme::Http,
            other => return Err(OriginError::UnsupportedScheme(other.to_string())),
        };
        let (host, site) = match url.host().ok_or(OriginError::NoHost)? {
            Host::Domain(d) => {
                let host = d.trim_end_matches('.').to_ascii_lowercase();
                if host.is_empty() {
                    return Err(OriginError::NoHost);
                }
                let site = registrable_domain(&host).unwrap_or_else(|| host.clone());
                (host, site)
            }
            Host::Ipv4(ip) => (ip.to_string(), ip.to_string()),
            Host::Ipv6(ip) => (format!("[{ip}]"), format!("[{ip}]")),
        };
        Ok(WebOrigin {
            scheme,
            port: url.port_or_known_default().ok_or(OriginError::NoHost)?,
            host,
            site,
        })
    }

    pub fn is_secure(&self) -> bool {
        self.scheme == Scheme::Https
    }

    /// Same scheme, host and port.
    pub fn same_origin(&self, other: &WebOrigin) -> bool {
        self.scheme == other.scheme && self.host == other.host && self.port == other.port
    }

    fn default_port(&self) -> u16 {
        match self.scheme {
            Scheme::Http => 80,
            Scheme::Https => 443,
        }
    }

    /// `https://host[:port]`, the form stored in new entries.
    pub fn serialize(&self) -> String {
        if self.port == self.default_port() {
            format!("{}://{}", self.scheme.as_str(), self.host)
        } else {
            format!("{}://{}:{}", self.scheme.as_str(), self.host, self.port)
        }
    }
}

impl fmt::Display for WebOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.serialize())
    }
}

/// The registrable domain ("eTLD+1") of a lower-case ASCII host, if any.
fn registrable_domain(host: &str) -> Option<String> {
    // A single label (`localhost`, `intranet`) has no registrable domain, even
    // though the list's implicit `*` rule treats any unknown TLD as a suffix.
    if !host.contains('.') {
        return None;
    }
    psl::domain_str(host).map(str::to_string)
}

/// How well a saved URL matches a page. Better matches sort first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchKind {
    /// Same host.
    Exact,
    /// Same registrable domain, different host.
    Site,
}

impl MatchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchKind::Exact => "exact",
            MatchKind::Site => "site",
        }
    }
}

/// Match one URL saved in an entry against a page origin.
///
/// Saved URLs may omit the scheme (`example.com/login`); such URLs match both
/// `http` and `https` pages. Unparseable or non-web URLs never match.
pub fn match_saved_url(page: &WebOrigin, saved: &str) -> Option<MatchKind> {
    let saved = saved.trim();
    if saved.is_empty() {
        return None;
    }
    let explicit_scheme = saved.contains("://");
    let url = if explicit_scheme {
        Url::parse(saved).ok()?
    } else {
        Url::parse(&format!("https://{saved}")).ok()?
    };
    let entry = WebOrigin::from_url(&url).ok()?;
    if entry.site != page.site {
        return None;
    }
    // Never offer credentials saved for https on an http page.
    if explicit_scheme && entry.is_secure() && !page.is_secure() {
        return None;
    }
    // A saved non-default port pins the entry to that port.
    if url.port().is_some() && entry.port != page.port {
        return None;
    }
    Some(if entry.host == page.host {
        MatchKind::Exact
    } else {
        MatchKind::Site
    })
}

/// The best match among an entry's saved URLs.
pub fn best_match<'a>(page: &WebOrigin, saved: impl IntoIterator<Item = &'a String>) -> Option<MatchKind> {
    saved.into_iter().filter_map(|u| match_saved_url(page, u)).min()
}

/// Where a fill would happen: the frame that holds the form, inside the
/// top-level page. URLs come from the browser, never from page content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FillContext {
    pub frame: WebOrigin,
    pub top: WebOrigin,
}

impl FillContext {
    pub fn new(frame_url: &str, top_url: &str) -> Result<FillContext, OriginError> {
        Ok(FillContext {
            frame: WebOrigin::parse(frame_url)?,
            top: WebOrigin::parse(top_url)?,
        })
    }

    /// Both the frame and the page are served over HTTPS.
    pub fn secure(&self) -> bool {
        self.frame.is_secure() && self.top.is_secure()
    }

    /// The form is in a frame whose origin differs from the page's.
    pub fn cross_origin(&self) -> bool {
        !self.frame.same_origin(&self.top)
    }

    /// Filling here needs the user's explicit, per-fill consent.
    pub fn needs_consent(&self) -> bool {
        !self.secure() || self.cross_origin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(url: &str) -> WebOrigin {
        WebOrigin::parse(url).unwrap()
    }

    fn m(page_url: &str, saved: &str) -> Option<MatchKind> {
        match_saved_url(&page(page_url), saved)
    }

    #[test]
    fn parses_origin_and_site() {
        let o = page("https://Login.Example.CO.UK.:8443/path?q=1#frag");
        assert_eq!(o.scheme, Scheme::Https);
        assert_eq!(o.host, "login.example.co.uk");
        assert_eq!(o.port, 8443);
        assert_eq!(o.site, "example.co.uk");
        assert_eq!(o.serialize(), "https://login.example.co.uk:8443");
        assert_eq!(page("http://example.com/").serialize(), "http://example.com");
        assert_eq!(page("https://example.com:443/").serialize(), "https://example.com");
    }

    #[test]
    fn rejects_non_web_urls() {
        for url in [
            "file:///etc/passwd",
            "chrome://settings",
            "about:blank",
            "javascript:alert(1)",
        ] {
            assert!(
                matches!(WebOrigin::parse(url), Err(OriginError::UnsupportedScheme(_))),
                "{url}"
            );
        }
        assert_eq!(WebOrigin::parse("not a url"), Err(OriginError::Invalid));
        assert_eq!(WebOrigin::parse(""), Err(OriginError::Invalid));
    }

    #[test]
    fn same_site_matches_across_subdomains() {
        assert_eq!(
            m("https://example.com/login", "https://example.com"),
            Some(MatchKind::Exact)
        );
        assert_eq!(
            m("https://www.example.com", "https://login.example.com/x"),
            Some(MatchKind::Site)
        );
        assert_eq!(m("https://a.b.example.co.uk", "example.co.uk"), Some(MatchKind::Site));
        assert_eq!(m("https://example.com", "https://example.org"), None);
        // A look-alike that merely ends with the same characters.
        assert_eq!(m("https://notexample.com", "https://example.com"), None);
        assert_eq!(m("https://example.com.evil.test", "https://example.com"), None);
    }

    #[test]
    fn public_suffixes_separate_sites() {
        // github.io is on the private section of the Public Suffix List.
        assert_eq!(m("https://alice.github.io", "https://bob.github.io"), None);
        assert_eq!(
            m("https://alice.github.io/app", "https://alice.github.io"),
            Some(MatchKind::Exact)
        );
        // co.uk is an ICANN suffix: different registrants.
        assert_eq!(m("https://shop-a.co.uk", "https://shop-b.co.uk"), None);
        // A public suffix used as a host only matches itself.
        assert_eq!(page("https://co.uk").site, "co.uk");
        assert_eq!(m("https://co.uk", "https://example.co.uk"), None);
    }

    #[test]
    fn hosts_without_registrable_domain_match_exactly() {
        assert_eq!(
            m("http://localhost:3000", "http://localhost:3000"),
            Some(MatchKind::Exact)
        );
        assert_eq!(m("http://localhost:3000", "localhost"), Some(MatchKind::Exact));
        assert_eq!(m("http://intranet/", "http://other/"), None);
        assert_eq!(
            m("https://192.168.1.10/", "https://192.168.1.10/admin"),
            Some(MatchKind::Exact)
        );
        assert_eq!(m("https://192.168.1.10/", "https://192.168.1.11/"), None);
        assert_eq!(m("https://[::1]:8443/", "https://[::1]:8443"), Some(MatchKind::Exact));
    }

    #[test]
    fn scheme_rules() {
        // https credentials are never offered on an http page.
        assert_eq!(m("http://example.com", "https://example.com"), None);
        // http credentials may be used on the https version of the site.
        assert_eq!(m("https://example.com", "http://example.com"), Some(MatchKind::Exact));
        // Without a scheme, both work.
        assert_eq!(m("http://example.com", "example.com"), Some(MatchKind::Exact));
        assert_eq!(m("https://example.com", "example.com/login"), Some(MatchKind::Exact));
        // Saved non-web URLs never match.
        assert_eq!(m("https://example.com", "ftp://example.com"), None);
        assert_eq!(m("https://example.com", "android://hash@com.example/"), None);
    }

    #[test]
    fn port_rules() {
        assert_eq!(
            m("https://example.com:8443", "https://example.com:8443"),
            Some(MatchKind::Exact)
        );
        assert_eq!(m("https://example.com", "https://example.com:8443"), None);
        // A saved URL without a port matches any port on the site.
        assert_eq!(
            m("https://example.com:8443", "https://example.com"),
            Some(MatchKind::Exact)
        );
    }

    #[test]
    fn internationalised_names() {
        // IDNA: the Unicode and punycode spellings are the same host.
        assert_eq!(
            m("https://bücher.example", "https://xn--bcher-kva.example"),
            Some(MatchKind::Exact)
        );
        assert_eq!(page("https://BÜCHER.example").host, "xn--bcher-kva.example");
    }

    #[test]
    fn junk_saved_urls_never_match() {
        for saved in ["", "   ", "://", "https://", "http://[bad", "not a url at all"] {
            assert_eq!(m("https://example.com", saved), None, "{saved:?}");
        }
    }

    #[test]
    fn best_match_prefers_exact() {
        let urls = vec![
            "https://other.org".to_string(),
            "https://www.example.com".into(),
            "example.com".into(),
        ];
        assert_eq!(best_match(&page("https://example.com"), &urls), Some(MatchKind::Exact));
        assert_eq!(
            best_match(&page("https://api.example.com"), &urls),
            Some(MatchKind::Site)
        );
        assert_eq!(best_match(&page("https://example.net"), &urls), None);
    }

    #[test]
    fn fill_context_consent_rules() {
        let c = FillContext::new("https://example.com/login", "https://example.com/").unwrap();
        assert!(c.secure() && !c.cross_origin() && !c.needs_consent());

        let c = FillContext::new("http://example.com/login", "http://example.com/").unwrap();
        assert!(!c.secure() && c.needs_consent());

        // An https frame in an http page is still not secure.
        let c = FillContext::new("https://example.com/login", "http://example.com/").unwrap();
        assert!(!c.secure() && c.needs_consent());

        // Even a same-site frame on another host is cross-origin.
        let c = FillContext::new("https://accounts.example.com/", "https://www.example.com/").unwrap();
        assert!(c.cross_origin() && c.needs_consent());

        let c = FillContext::new("https://evil.test/", "https://example.com/").unwrap();
        assert!(c.cross_origin() && c.needs_consent());

        assert!(FillContext::new("about:blank", "https://example.com").is_err());
    }
}
