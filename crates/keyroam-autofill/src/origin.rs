//! Web origins and public-suffix-aware site matching.
//!
//! A credential is offered on a page only when the page belongs to the same
//! *site* as a URL saved in the entry. The site is the registrable domain
//! ("eTLD+1") from the Public Suffix List, including its private section, so
//! `alice.github.io` and `mallory.github.io` are different sites while
//! `login.example.co.uk` and `www.example.co.uk` are the same one. IP
//! addresses, single-label hosts such as `localhost`, and hosts that are
//! themselves public suffixes only ever match exactly.

use std::fmt;

use url::{Host, Url};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scheme {
    Http,
    Https,
}

impl Scheme {
    fn default_port(self) -> u16 {
        match self {
            Scheme::Http => 80,
            Scheme::Https => 443,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Scheme::Http => "http",
            Scheme::Https => "https",
        }
    }
}

/// The scheme, host and effective port of an http(s) URL.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WebOrigin {
    scheme: Scheme,
    /// Lower-case ASCII (IDNA/punycode) without a trailing dot. IPv6
    /// addresses are kept in brackets.
    host: String,
    port: u16,
    is_ip: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OriginError {
    Invalid,
    /// Not an http or https page (e.g. `file:`, `chrome:`, `about:`).
    UnsupportedScheme,
}

impl fmt::Display for OriginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OriginError::Invalid => f.write_str("not a valid web address"),
            OriginError::UnsupportedScheme => f.write_str("only http and https pages can be filled"),
        }
    }
}

impl std::error::Error for OriginError {}

/// How well a saved URL matches a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchKind {
    /// Same host and port.
    Exact,
    /// A different host of the same registrable domain.
    SameSite,
}

impl MatchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchKind::Exact => "exact",
            MatchKind::SameSite => "same_site",
        }
    }
}

impl WebOrigin {
    /// The origin of a page URL. Only http and https pages are accepted.
    pub fn parse(url: &str) -> Result<WebOrigin, OriginError> {
        let url = Url::parse(url.trim()).map_err(|_| OriginError::Invalid)?;
        Self::from_url(&url)
    }

    /// The origin of a URL saved in an entry. Users often save bare hosts
    /// ("example.com/login"); those are read as https.
    pub fn parse_saved(saved: &str) -> Option<WebOrigin> {
        let saved = saved.trim();
        if saved.is_empty() {
            return None;
        }
        match Url::parse(saved) {
            Ok(url) if url.has_host() => Self::from_url(&url).ok(),
            // "example.com:8443" parses as scheme "example.com"; "example.com" fails.
            _ => Url::parse(&format!("https://{saved}"))
                .ok()
                .and_then(|u| Self::from_url(&u).ok()),
        }
    }

    fn from_url(url: &Url) -> Result<WebOrigin, OriginError> {
        let scheme = match url.scheme() {
            "https" => Scheme::Https,
            "http" => Scheme::Http,
            _ => return Err(OriginError::UnsupportedScheme),
        };
        let (host, is_ip) = match url.host().ok_or(OriginError::Invalid)? {
            Host::Domain(d) => (d.trim_end_matches('.').to_ascii_lowercase(), false),
            Host::Ipv4(ip) => (ip.to_string(), true),
            Host::Ipv6(ip) => (format!("[{ip}]"), true),
        };
        if host.is_empty() {
            return Err(OriginError::Invalid);
        }
        let port = url.port().unwrap_or(scheme.default_port());
        Ok(WebOrigin {
            scheme,
            host,
            port,
            is_ip,
        })
    }

    pub fn scheme(&self) -> Scheme {
        self.scheme
    }
    pub fn host(&self) -> &str {
        &self.host
    }
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn is_secure(&self) -> bool {
        self.scheme == Scheme::Https
    }

    fn has_default_port(&self) -> bool {
        self.port == self.scheme.default_port()
    }

    /// The registrable domain, or `None` for IP addresses, single-label hosts
    /// and hosts that are themselves public suffixes.
    pub fn site(&self) -> Option<&str> {
        if self.is_ip {
            return None;
        }
        let domain = psl::domain(self.host.as_bytes())?;
        // Only ICANN or private-section suffixes count. An unknown TLD is
        // matched by the list's implicit "*" rule, which would make e.g.
        // "intranet.corp" a site; treat such hosts as exact-only instead.
        if !domain.suffix().is_known() {
            return None;
        }
        std::str::from_utf8(domain.as_bytes()).ok()
    }

    /// True when both origins belong to the same site (registrable domain),
    /// or are the same host where no registrable domain exists.
    pub fn same_site(&self, other: &WebOrigin) -> bool {
        match (self.site(), other.site()) {
            (Some(a), Some(b)) => a == b,
            _ => self.host == other.host,
        }
    }

    /// How a credential saved for `self` matches the page `page`, if at all.
    ///
    /// A URL saved as https is never offered on an http page of the same
    /// host (that would leak it to a network attacker); a URL saved as http
    /// is offered on the https version.
    pub fn matches_page(&self, page: &WebOrigin) -> Option<MatchKind> {
        if self.scheme == Scheme::Https && page.scheme == Scheme::Http {
            return None;
        }
        if self.host == page.host {
            let same_port = self.port == page.port || (self.has_default_port() && page.has_default_port());
            return same_port.then_some(MatchKind::Exact);
        }
        match (self.site(), page.site()) {
            (Some(a), Some(b)) if a == b => Some(MatchKind::SameSite),
            _ => None,
        }
    }
}

impl fmt::Display for WebOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}", self.scheme.as_str(), self.host)?;
        if !self.has_default_port() {
            write!(f, ":{}", self.port)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(s: &str) -> WebOrigin {
        WebOrigin::parse(s).unwrap()
    }
    fn saved(s: &str) -> WebOrigin {
        WebOrigin::parse_saved(s).unwrap()
    }

    #[test]
    fn parses_and_normalises() {
        let a = o("HTTPS://Login.Example.COM./path?q=1#frag");
        assert_eq!(a.host(), "login.example.com");
        assert_eq!(a.port(), 443);
        assert_eq!(a.to_string(), "https://login.example.com");
        assert_eq!(o("http://example.com:8080/").to_string(), "http://example.com:8080");
        assert_eq!(o("https://bücher.example/").host(), "xn--bcher-kva.example");
        assert_eq!(o("https://[::1]:8443/").to_string(), "https://[::1]:8443");
        assert_eq!(
            WebOrigin::parse("file:///etc/passwd"),
            Err(OriginError::UnsupportedScheme)
        );
        assert_eq!(
            WebOrigin::parse("chrome://settings"),
            Err(OriginError::UnsupportedScheme)
        );
        assert_eq!(WebOrigin::parse("not a url"), Err(OriginError::Invalid));
    }

    #[test]
    fn saved_urls_without_scheme_are_https() {
        assert_eq!(saved("example.com").to_string(), "https://example.com");
        assert_eq!(saved("example.com/login").to_string(), "https://example.com");
        assert_eq!(saved("example.com:8443").to_string(), "https://example.com:8443");
        assert_eq!(saved("http://example.com").to_string(), "http://example.com");
        assert!(WebOrigin::parse_saved("").is_none());
        assert!(WebOrigin::parse_saved("ftp://example.com").is_none());
        assert!(WebOrigin::parse_saved("android://com.example.app").is_none());
    }

    #[test]
    fn registrable_domains() {
        assert_eq!(o("https://a.b.example.co.uk").site(), Some("example.co.uk"));
        assert_eq!(o("https://example.com").site(), Some("example.com"));
        // Private-section suffixes separate tenants.
        assert_eq!(o("https://alice.github.io").site(), Some("alice.github.io"));
        // No registrable domain.
        assert_eq!(o("https://co.uk").site(), None);
        assert_eq!(o("https://github.io").site(), None);
        assert_eq!(o("http://localhost:3000").site(), None);
        assert_eq!(o("https://192.168.1.1").site(), None);
        assert_eq!(o("https://intranet.corp-unknown-tld").site(), None);
    }

    #[test]
    fn matching_rules() {
        let page = o("https://login.example.com/signin");
        assert_eq!(
            saved("https://login.example.com").matches_page(&page),
            Some(MatchKind::Exact)
        );
        assert_eq!(saved("example.com").matches_page(&page), Some(MatchKind::SameSite));
        assert_eq!(
            saved("https://www.example.com/x").matches_page(&page),
            Some(MatchKind::SameSite)
        );
        // Look-alikes and other registrable domains never match.
        assert_eq!(saved("https://example.com.evil.test").matches_page(&page), None);
        assert_eq!(saved("https://evil-example.com").matches_page(&page), None);
        assert_eq!(saved("https://example.org").matches_page(&page), None);
        // Different tenants of a shared-hosting suffix never match.
        assert_eq!(
            saved("https://alice.github.io").matches_page(&o("https://mallory.github.io")),
            None
        );
        // Public suffixes and IPs only match exactly.
        assert_eq!(saved("https://co.uk").matches_page(&o("https://example.co.uk")), None);
        assert_eq!(
            saved("https://10.0.0.1").matches_page(&o("https://10.0.0.1/")),
            Some(MatchKind::Exact)
        );
        assert_eq!(saved("https://10.0.0.1").matches_page(&o("https://10.0.0.2/")), None);
        // Ports must agree for the same host.
        assert_eq!(
            saved("http://localhost:3000").matches_page(&o("http://localhost:8080")),
            None
        );
        assert_eq!(
            saved("http://localhost:3000").matches_page(&o("http://localhost:3000/login")),
            Some(MatchKind::Exact)
        );
    }

    #[test]
    fn never_downgrade_to_http() {
        assert_eq!(
            saved("https://example.com").matches_page(&o("http://example.com")),
            None
        );
        assert_eq!(saved("example.com").matches_page(&o("http://www.example.com")), None);
        // Upgrading is fine, and default ports are equivalent across schemes.
        assert_eq!(
            saved("http://example.com").matches_page(&o("https://example.com")),
            Some(MatchKind::Exact)
        );
    }

    #[test]
    fn same_site_for_frames() {
        assert!(o("https://accounts.example.com").same_site(&o("https://www.example.com")));
        assert!(!o("https://example.com").same_site(&o("https://ads.example.net")));
        assert!(o("http://localhost:3000").same_site(&o("http://localhost:9000")));
        assert!(!o("https://alice.github.io").same_site(&o("https://bob.github.io")));
    }
}
