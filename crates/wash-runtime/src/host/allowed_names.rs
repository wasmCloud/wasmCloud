//! Name allowlist for `wasi:sockets/ip-name-lookup` resolution.
//!
//! A workload declares which names its components may resolve. Each entry
//! parses from a plain string on the wire (proto, wash YAML, CRD) into an
//! [`AllowedName`], and [`AllowedName::matches`] evaluates a requested name
//! against it at resolve time.
//!
//! # Accepted forms
//!
//! | Form              | Variant                         |
//! | ----------------- | ------------------------------- |
//! | `*`               | [`AllowedName::Any`]            |
//! | `*.example.com`   | [`AllowedName::SuffixWildcard`] |
//! | `example.com`     | [`AllowedName::Exact`]          |
//! | `127.0.0.1`, `::1`| [`AllowedName::Ip`]             |
//!
//! The wildcard must be `*.<rest>`. A bare `*foo` is rejected: `*com`
//! matching every `.com` is never the intent.
//!
//! A name is not a URL. Entries carrying a scheme, port, path, query, or
//! fragment are rejected at parse time, because none of those participate
//! in resolving a name to addresses.
//!
//! # Empty list denies every lookup
//!
//! An empty policy rejects all resolution with
//! `permanent-resolver-failure`. Resolution stays opt-in: nothing
//! substitutes an allow-all policy for a workload that declared none. To
//! resolve any name, declare `["*"]`.
//!
//! # Why names and not just a switch
//!
//! Resolution reaches the network before any connection is attempted, so a
//! guest permitted to resolve anything can encode data in the labels it
//! looks up and have a resolver carry it off the host. Restricting which
//! names may be resolved closes that channel. Which addresses a component
//! may then connect to is a separate policy, see
//! [`crate::host::allowed_hosts`].
//!
//! # Matching semantics
//!
//! - Comparison is ASCII-case-insensitive, against the punycode form
//!   the socket layer produces when it parses the requested name.
//! - [`AllowedName::SuffixWildcard`] requires a non-empty prefix, so
//!   `example.com` does not satisfy `*.example.com`.
//! - [`AllowedName::Ip`] compares parsed addresses, so `::1` and
//!   `0:0:0:0:0:0:0:1` are the same entry.
//! - Only [`AllowedName::Any`] and [`AllowedName::Ip`] match a literal
//!   address. A suffix never matches one.
//!
//! # Examples
//!
//! ```
//! use wash_runtime::host::allowed_names::AllowedName;
//!
//! let policy: AllowedName = "*.example.com".parse().unwrap();
//! assert!(policy.matches(&url::Host::parse("api.example.com").unwrap()));
//! assert!(!policy.matches(&url::Host::parse("evil.com").unwrap()));
//! ```

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

use anyhow::anyhow;
use serde::{Deserialize, Serialize, de, ser};

/// A parsed entry from the `allowIpNameLookup` allowlist.
///
/// See the [module-level docs](self) for accepted string forms and
/// matching semantics. Parsed via [`FromStr`]; rendered back to its wire
/// representation via [`Display`](fmt::Display); the [`Serialize`] /
/// [`Deserialize`] impls round-trip through that same string form, so
/// YAML / JSON callers see plain strings.
///
/// # Errors
///
/// Parsing via [`FromStr`] returns an error when the input:
///
/// - is empty (after trimming),
/// - is a wildcard not of the form `*.<rest>`,
/// - carries a scheme, port, path, query, or fragment,
/// - is not a syntactically valid domain name or IP address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowedName {
    /// `*`, matching every name and address.
    Any,
    /// `example.com`, matching that one name.
    ///
    /// Stored lowercased.
    Exact(String),
    /// `*.example.com`, matching subdomains of the suffix.
    ///
    /// `suffix` stores the canonical lowercased suffix *including* the
    /// leading dot, e.g. `".example.com"`. Matching requires at least one
    /// character before it, so `example.com` does not satisfy
    /// `*.example.com`.
    SuffixWildcard { suffix: String },
    /// `127.0.0.1` or `::1`, matching that one address.
    Ip(IpAddr),
}

impl AllowedName {
    /// Returns `true` if `host` satisfies this allowlist entry.
    ///
    /// `host` is the parsed, punycoded form of the requested name.
    #[must_use]
    pub fn matches(&self, host: &url::Host<String>) -> bool {
        match (self, host) {
            (AllowedName::Any, _) => true,
            (AllowedName::Ip(allowed), url::Host::Ipv4(addr)) => *allowed == IpAddr::V4(*addr),
            (AllowedName::Ip(allowed), url::Host::Ipv6(addr)) => *allowed == IpAddr::V6(*addr),
            (AllowedName::Ip(_), url::Host::Domain(_)) => false,
            (AllowedName::Exact(allowed), url::Host::Domain(domain)) => {
                strip_root_dot(domain).eq_ignore_ascii_case(allowed)
            }
            (AllowedName::SuffixWildcard { suffix }, url::Host::Domain(domain)) => {
                let domain = strip_root_dot(domain);
                domain.len() > suffix.len()
                    && domain[domain.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
            }
            (AllowedName::Exact(_) | AllowedName::SuffixWildcard { .. }, _) => false,
        }
    }
}

/// Returns `true` if `host` satisfies any entry in `policy`.
///
/// An empty `policy` denies every name.
#[must_use]
pub fn check_allowed_names(policy: &[AllowedName], host: &url::Host<String>) -> bool {
    policy.iter().any(|entry| entry.matches(host))
}

impl FromStr for AllowedName {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("allowed-name entry is empty"));
        }

        if trimmed == "*" {
            return Ok(AllowedName::Any);
        }

        for (pattern, what) in [
            ("://", "a scheme"),
            ("/", "a path"),
            ("?", "a query string"),
            ("#", "a fragment"),
        ] {
            if trimmed.contains(pattern) {
                return Err(anyhow!(
                    "must not include {what}; a name lookup resolves a host name, not a URL"
                ));
            }
        }

        if let Some(suffix) = trimmed.strip_prefix("*.") {
            validate_domain(suffix)?;
            return Ok(AllowedName::SuffixWildcard {
                suffix: format!(".{}", suffix.to_ascii_lowercase()),
            });
        }
        if trimmed.contains('*') {
            return Err(anyhow!(
                "wildcard must be of the form '*.suffix' with a leading dot"
            ));
        }

        // Bare IPv6 (`::1`) parses here; anything else holding a colon is a
        // port, which resolution has no use for.
        if let Ok(addr) = trimmed.parse::<IpAddr>() {
            return Ok(AllowedName::Ip(addr));
        }
        if trimmed.contains(':') {
            return Err(anyhow!(
                "must not include a port; a name lookup resolves a host name, not an endpoint"
            ));
        }

        validate_domain(trimmed)?;
        Ok(AllowedName::Exact(trimmed.to_ascii_lowercase()))
    }
}

impl fmt::Display for AllowedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AllowedName::Any => f.write_str("*"),
            AllowedName::Exact(name) => f.write_str(name),
            // `suffix` already carries the leading dot.
            AllowedName::SuffixWildcard { suffix } => write!(f, "*{suffix}"),
            AllowedName::Ip(addr) => write!(f, "{addr}"),
        }
    }
}

impl Serialize for AllowedName {
    fn serialize<S: ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for AllowedName {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        s.parse().map_err(de::Error::custom)
    }
}

/// Drops a single trailing root dot so the fully-qualified `example.com.`
/// compares equal to `example.com`.
fn strip_root_dot(domain: &str) -> &str {
    domain.strip_suffix('.').unwrap_or(domain)
}

/// Rejects anything that is not a syntactically valid domain name.
///
/// Labels must be non-empty, at most 63 bytes, alphanumeric or hyphen, and
/// must not lead or trail with a hyphen. The whole name is capped at 253
/// bytes.
fn validate_domain(domain: &str) -> anyhow::Result<()> {
    let domain = strip_root_dot(domain);
    if domain.is_empty() {
        return Err(anyhow!("host name is empty"));
    }
    if domain.len() > 253 {
        return Err(anyhow!("host name is longer than 253 bytes"));
    }
    for label in domain.split('.') {
        if label.is_empty() {
            return Err(anyhow!("host name '{domain}' has an empty label"));
        }
        if label.len() > 63 {
            return Err(anyhow!(
                "host name '{domain}' has a label longer than 63 bytes"
            ));
        }
        if !label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(anyhow!(
                "host name '{domain}' has a label with characters outside a-z, 0-9, and '-'"
            ));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(anyhow!(
                "host name '{domain}' has a label leading or trailing with '-'"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(s: &str) -> url::Host<String> {
        crate::sockets::util::parse_host(s).expect("test gave an unparseable host")
    }

    fn parse(s: &str) -> AllowedName {
        s.parse().expect("test gave an invalid allowed-name entry")
    }

    #[test]
    fn star_matches_names_and_addresses() {
        let any = parse("*");
        assert!(any.matches(&host("example.com")));
        assert!(any.matches(&host("anything.at.all")));
        assert!(any.matches(&host("127.0.0.1")));
        assert!(any.matches(&host("::1")));
    }

    #[test]
    fn exact_matches_only_that_name() {
        let policy = parse("example.com");
        assert!(policy.matches(&host("example.com")));
        // Case and the fully-qualified root dot are both normalized away.
        assert!(policy.matches(&host("EXAMPLE.COM")));
        assert!(policy.matches(&host("example.com.")));

        assert!(!policy.matches(&host("api.example.com")));
        assert!(!policy.matches(&host("example.org")));
        assert!(!policy.matches(&host("notexample.com")));
    }

    #[test]
    fn suffix_wildcard_requires_a_subdomain() {
        let policy = parse("*.example.com");
        assert!(policy.matches(&host("api.example.com")));
        assert!(policy.matches(&host("deep.nested.example.com")));

        // The bare suffix is not a subdomain of itself.
        assert!(!policy.matches(&host("example.com")));
        // A suffix match is per-label, not a substring of the name.
        assert!(!policy.matches(&host("evilexample.com")));
        assert!(!policy.matches(&host("example.com.evil.net")));
    }

    #[test]
    fn suffix_wildcard_never_matches_an_address() {
        // `1.2.3.4` textually ends with `.4`, but an address is not a
        // subdomain of anything.
        let policy = parse("*.4");
        assert!(!policy.matches(&host("1.2.3.4")));
    }

    #[test]
    fn ip_entries_compare_parsed_addresses() {
        let v4 = parse("127.0.0.1");
        assert!(v4.matches(&host("127.0.0.1")));
        assert!(!v4.matches(&host("127.0.0.2")));
        assert!(!v4.matches(&host("example.com")));

        // Both spellings of loopback are the same address.
        let v6 = parse("::1");
        assert!(v6.matches(&host("::1")));
        assert!(v6.matches(&host("0:0:0:0:0:0:0:1")));
    }

    #[test]
    fn unicode_names_match_their_punycode_entry() {
        // `parse_host` punycodes before matching, so an entry written in
        // punycode catches the unicode spelling of the same name.
        let policy = parse("xn--n3h.example.com");
        assert!(policy.matches(&host("☃.example.com")));
    }

    #[test]
    fn empty_policy_denies_everything() {
        assert!(!check_allowed_names(&[], &host("example.com")));
        assert!(!check_allowed_names(&[], &host("127.0.0.1")));
    }

    #[test]
    fn check_matches_any_entry() {
        let policy = [parse("example.com"), parse("*.internal")];
        assert!(check_allowed_names(&policy, &host("example.com")));
        assert!(check_allowed_names(&policy, &host("db.internal")));
        assert!(!check_allowed_names(&policy, &host("evil.com")));
    }

    #[test]
    fn bare_star_wildcard_is_rejected() {
        // `*com` would match every `.com`, which is never the intent.
        let err = "*com".parse::<AllowedName>().unwrap_err();
        assert!(format!("{err:#}").contains("leading dot"), "{err:#}");
    }

    #[test]
    fn url_shaped_entries_are_rejected() {
        for entry in [
            "https://example.com",
            "example.com/v1",
            "example.com?q=1",
            "example.com#frag",
        ] {
            assert!(
                entry.parse::<AllowedName>().is_err(),
                "{entry} should be rejected"
            );
        }
    }

    #[test]
    fn port_entries_are_rejected() {
        let err = "example.com:8080".parse::<AllowedName>().unwrap_err();
        assert!(format!("{err:#}").contains("port"), "{err:#}");
    }

    #[test]
    fn malformed_names_are_rejected() {
        for entry in ["", "   ", "example..com", "-lead.com", "trail-.com"] {
            assert!(
                entry.parse::<AllowedName>().is_err(),
                "{entry:?} should be rejected"
            );
        }
    }

    #[test]
    fn entries_round_trip_through_their_string_form() {
        for entry in ["*", "example.com", "*.example.com", "127.0.0.1", "::1"] {
            let parsed = parse(entry);
            assert_eq!(parsed.to_string(), entry);
            assert_eq!(parsed.to_string().parse::<AllowedName>().unwrap(), parsed);
        }
    }
}
