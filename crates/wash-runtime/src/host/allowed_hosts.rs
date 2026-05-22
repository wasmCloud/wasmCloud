//! Outbound host allowlist for HTTP/TCP egress from workloads.
//!
//! Each entry parses into one of four shapes and is checked at request time
//! by [`AllowedHost::matches`]. Wire format is a plain string. Both the
//! wash YAML config and the Kubernetes `WorkloadDeployment` CRD carry
//! `allowedHosts: [String]`; parsing into [`AllowedHost`] happens once,
//! either at deserialize-time (wash) or at proto → in-memory conversion
//! (operator path), so request-path matching is allocation-free.
//!
//! # Accepted forms
//!
//! | Form                                 | Variant                         |
//! | ------------------------------------ | ------------------------------- |
//! | `*`                                  | [`AllowedHost::Any`]            |
//! | `*.example.com[:port]`               | [`AllowedHost::SuffixWildcard`] |
//! | `scheme://*.example.com[:port][/]`   | [`AllowedHost::SuffixWildcard`] |
//! | `scheme://host[:port][/]`            | [`AllowedHost::Url`]            |
//! | `host[:port]`                        | [`AllowedHost::Authority`]      |
//!
//! The wildcard must always be `*.<rest>` (leading-dot subdomain match).
//! A bare `*foo` is rejected — `*com` matching every `.com` was never the
//! intent and is a footgun.
//!
//! This is a host policy, not a URL policy. Entries are rejected at parse
//! time if they include any path beyond a bare trailing `/`, a query
//! string, or a fragment.
//!
//! # Empty list = deny all
//!
//! At the runtime check ([`crate::host::http::check_allowed_hosts`]) an
//! empty list of [`AllowedHost`] entries denies every outgoing request
//! (fail-closed). Callers that want unrestricted egress must pass an
//! explicit `[AllowedHost::Any]`. The wash config layer substitutes
//! `[Any]` when `allowedHosts` is omitted from YAML, so `wash dev`
//! workloads land at the runtime with a populated policy.
//!
//! # Matching semantics
//!
//! - Hostname comparison is ASCII-case-insensitive.
//! - `Authority` and `SuffixWildcard` with no explicit port match any
//!   request port. With an explicit port they match exact.
//! - `SuffixWildcard` with no explicit scheme matches any scheme. With an
//!   explicit scheme it matches exact (case-insensitive).
//! - `Url` matches scheme + host + port exactly. Paths on policy entries
//!   aren't allowed (see above) and request paths/queries are not
//!   inspected by the matcher.
//!
//! # Examples
//!
//! ```
//! use wash_runtime::host::allowed_hosts::AllowedHost;
//!
//! let policy: AllowedHost = "*.example.com".parse().unwrap();
//! let req: http::Uri = "http://api.example.com/v1/users".parse().unwrap();
//! assert!(policy.matches(&req));
//!
//! let denied: http::Uri = "http://evil.com".parse().unwrap();
//! assert!(!policy.matches(&denied));
//! ```

use std::fmt;
use std::str::FromStr;

use anyhow::{Context, anyhow};
use http::Uri;
use http::uri::{Authority, Scheme};
use serde::{Deserialize, Serialize, de, ser};
use url::Url;

/// A parsed entry from the `allowedHosts` allowlist.
///
/// See the [module-level docs](self) for accepted string forms and
/// matching semantics. Parsed via [`FromStr`]; rendered back to its wire
/// representation via [`Display`](fmt::Display); the [`Serialize`] /
/// [`Deserialize`] impls round-trip through that same string form, so
/// YAML / JSON callers see plain strings. Use [`AllowedHost::matches`]
/// to evaluate a request URI against an entry.
///
/// # Errors
///
/// Parsing via [`FromStr`] returns an error when the input:
///
/// - is empty (after trimming),
/// - is a wildcard not of the form `*.<rest>` (e.g. bare `*foo` is rejected),
/// - has an invalid URL scheme, host, or port,
/// - is a URL form with a path beyond bare `/`, a query string, or a
///   fragment — this is a hosts policy, not a URL policy.
///
/// # Examples
///
/// ```
/// use wash_runtime::host::allowed_hosts::AllowedHost;
///
/// // The five accepted forms all parse:
/// let _: AllowedHost = "*".parse().unwrap();
/// let _: AllowedHost = "example.com".parse().unwrap();
/// let _: AllowedHost = "example.com:8443".parse().unwrap();
/// let _: AllowedHost = "https://api.example.com".parse().unwrap();
/// let _: AllowedHost = "*.example.com".parse().unwrap();
///
/// // Paths on URL entries are rejected — this is a hosts policy.
/// assert!("https://api.example.com/v1".parse::<AllowedHost>().is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AllowedHost {
    /// `*` — match every outbound host.
    Any,
    /// `host[:port]` — exact host match, optional port pin.
    Authority(Authority),
    /// `scheme://host[:port][/]` — exact scheme + host (+ optional port)
    /// match. A bare trailing `/` is accepted for ergonomics (matches what
    /// `url::Url` normalizes to); any other path, query, or fragment is
    /// rejected at parse time because this is a host policy, not a URL
    /// policy.
    Url(Url),
    /// `[scheme://]*.suffix[:port]` — subdomain wildcard.
    ///
    /// `suffix` stores the canonical lowercased suffix *including* the
    /// leading dot, e.g. `".example.com"`. Matching requires the request
    /// host to end with `suffix` AND have at least one character before
    /// it, so `example.com` does NOT satisfy `*.example.com`.
    SuffixWildcard {
        suffix: String,
        scheme: Option<Scheme>,
        port: Option<u16>,
    },
}

impl AllowedHost {
    /// Returns `true` if `request` satisfies this allowlist entry.
    ///
    /// Inspects the URI's host, scheme, and explicit port. Path, query,
    /// and fragment are not consulted — this is a host policy, not a URL
    /// policy. A URI without a host (e.g. a relative `/path-only`) never
    /// matches; the caller is expected to have already validated request
    /// shape.
    ///
    /// # Examples
    ///
    /// ```
    /// use wash_runtime::host::allowed_hosts::AllowedHost;
    ///
    /// let policy: AllowedHost = "https://api.example.com".parse().unwrap();
    /// let allowed: http::Uri = "https://api.example.com/v1".parse().unwrap();
    /// let wrong_scheme: http::Uri = "http://api.example.com".parse().unwrap();
    ///
    /// assert!(policy.matches(&allowed));     // scheme + host match; path ignored
    /// assert!(!policy.matches(&wrong_scheme));
    /// ```
    pub fn matches(&self, request: &Uri) -> bool {
        let Some(request_host) = request.host() else {
            return false;
        };
        let request_scheme = request.scheme_str();
        let request_port = request.port_u16();

        match self {
            AllowedHost::Any => true,

            AllowedHost::Authority(authority) => {
                if !authority.host().eq_ignore_ascii_case(request_host) {
                    return false;
                }
                // Unspecified port on the policy matches any request port.
                match authority.port_u16() {
                    Some(p) => Some(p) == request_port,
                    None => true,
                }
            }

            AllowedHost::Url(url) => {
                let Some(policy_host) = url.host_str() else {
                    return false;
                };
                if !policy_host.eq_ignore_ascii_case(request_host) {
                    return false;
                }
                if let Some(req_scheme) = request_scheme
                    && !url.scheme().eq_ignore_ascii_case(req_scheme)
                {
                    return false;
                }
                match url.port() {
                    Some(p) => Some(p) == request_port,
                    None => true,
                }
            }

            AllowedHost::SuffixWildcard {
                suffix,
                scheme,
                port,
            } => {
                // Require `host` to end with `.suffix-without-dot` AND have
                // at least one char before the dot. `suffix` already has
                // the leading dot baked in.
                let host_lower = request_host.to_ascii_lowercase();
                let Some(prefix) = host_lower.strip_suffix(suffix.as_str()) else {
                    return false;
                };
                if prefix.is_empty() {
                    return false;
                }
                if let (Some(pol_scheme), Some(req_scheme)) = (scheme.as_ref(), request_scheme)
                    && !pol_scheme.as_str().eq_ignore_ascii_case(req_scheme)
                {
                    return false;
                }
                match port {
                    Some(p) => Some(*p) == request_port,
                    None => true,
                }
            }
        }
    }
}

impl FromStr for AllowedHost {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("allowed-host entry is empty"));
        }

        // 1. `*` (no scheme, no port).
        if trimmed == "*" {
            return Ok(AllowedHost::Any);
        }

        // 2. `scheme://…`. Sub-cases: wildcard (`scheme://*.foo`) vs. exact
        //    (`scheme://host[:port]`). Match the wildcard form by hand since
        //    `url::Url` won't parse `*.foo` as a host. Paths beyond bare `/`
        //    are rejected for both sub-cases — this is a host policy, not a
        //    URL policy, and silently stripping a path would teach the wrong
        //    mental model (`https://api/v1` does NOT restrict to `/v1`).
        if let Some((scheme_part, rest)) = trimmed.split_once("://") {
            let scheme = Scheme::from_str(scheme_part)
                .with_context(|| format!("invalid scheme '{scheme_part}'"))?;

            if let Some(wildcard_rest) = rest.strip_prefix("*.") {
                // `scheme://*.foo.com[:port][/]`
                let (host_port, path) = wildcard_rest
                    .split_once('/')
                    .map_or((wildcard_rest, ""), |(h, p)| (h, p));
                reject_non_root_path(path)?;
                let (suffix_no_dot, port) = split_host_port(host_port)
                    .with_context(|| format!("invalid wildcard host '{wildcard_rest}'"))?;
                return Ok(AllowedHost::SuffixWildcard {
                    suffix: format!(".{}", suffix_no_dot.to_ascii_lowercase()),
                    scheme: Some(scheme),
                    port,
                });
            }

            // Plain URL form. Let `url::Url` do the heavy lifting, then
            // reject anything beyond scheme + host + port + bare `/`.
            // Error messages here don't repeat the entry text — callers
            // (e.g. `TryFrom<v2::LocalResources>` in washlet) already wrap
            // each error with `'<entry>':`, so duplicating it produces
            // unreadable nested quoting.
            let url = Url::parse(trimmed).context("not a valid URL")?;
            if url.host_str().is_none() {
                return Err(anyhow!("URL has no host"));
            }
            if url.path() != "" && url.path() != "/" {
                return Err(anyhow!("must not include a path; got '{}'", url.path()));
            }
            if url.query().is_some() {
                return Err(anyhow!("must not include a query string"));
            }
            if url.fragment().is_some() {
                return Err(anyhow!("must not include a fragment"));
            }
            return Ok(AllowedHost::Url(url));
        }

        // 3. Scheme-less wildcard: `*.foo.com[:port]`.
        if let Some(wildcard_rest) = trimmed.strip_prefix("*.") {
            let (suffix_no_dot, port) = split_host_port(wildcard_rest)
                .with_context(|| format!("invalid wildcard host '{wildcard_rest}'"))?;
            return Ok(AllowedHost::SuffixWildcard {
                suffix: format!(".{}", suffix_no_dot.to_ascii_lowercase()),
                scheme: None,
                port,
            });
        }

        // 4. Reject ambiguous wildcards that don't follow `*.<rest>`. A
        //    bare `*foo` would historically match `barfoo`, which is a
        //    foot-gun (`*com` matches every .com). Make it a parse error.
        if trimmed.starts_with('*') {
            return Err(anyhow!(
                "wildcard must be of the form `*.<rest>` (leading dot required)"
            ));
        }

        // 5. Bare authority — `host[:port]`. Let `http::Authority` validate
        //    the syntax; additionally reject `host:port` where `port` isn't
        //    a valid u16. `Authority::port_u16()` returns `None` both when
        //    no port is present and when the port string fails to parse, so
        //    detect the "port suffix is present" case explicitly. IPv6 hosts
        //    are bracket-wrapped so their host portion contains `:` itself;
        //    the port (if any) follows `]:`, not the first `:`.
        let authority = Authority::from_str(trimmed).context("invalid host[:port]")?;
        let s = authority.as_str();
        let has_port_suffix = if s.starts_with('[') {
            s.contains("]:")
        } else {
            s.contains(':')
        };
        if has_port_suffix && authority.port_u16().is_none() {
            return Err(anyhow!("invalid port"));
        }
        Ok(AllowedHost::Authority(authority))
    }
}

impl fmt::Display for AllowedHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AllowedHost::Any => f.write_str("*"),
            AllowedHost::Authority(authority) => write!(f, "{authority}"),
            AllowedHost::Url(url) => write!(f, "{url}"),
            AllowedHost::SuffixWildcard {
                suffix,
                scheme,
                port,
            } => {
                // `suffix` already has the leading dot; render the three
                // optional pieces directly to the formatter to avoid
                // intermediate `String` allocations.
                if let Some(scheme) = scheme {
                    write!(f, "{scheme}://")?;
                }
                write!(f, "*{suffix}")?;
                if let Some(port) = port {
                    write!(f, ":{port}")?;
                }
                Ok(())
            }
        }
    }
}

impl Serialize for AllowedHost {
    fn serialize<S: ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for AllowedHost {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        s.parse().map_err(de::Error::custom)
    }
}

/// Rejects any path component beyond the empty string.
///
/// Used for the wildcard arms where we hand-parse
/// `scheme://*.foo[:port][/]` and need to refuse `scheme://*.foo/v1`
/// while still accepting a bare trailing slash (`scheme://*.foo/`).
fn reject_non_root_path(path: &str) -> anyhow::Result<()> {
    if !path.is_empty() {
        return Err(anyhow!("must not include a path; got '/{path}'"));
    }
    Ok(())
}

/// Parses `host` or `host:port` into `(host_no_port, Option<port>)`.
///
/// Rejects empty host and out-of-range / non-numeric ports.
fn split_host_port(s: &str) -> anyhow::Result<(&str, Option<u16>)> {
    match s.rsplit_once(':') {
        Some((host, port_s)) => {
            if host.is_empty() {
                return Err(anyhow!("empty host before ':'"));
            }
            let port: u16 = port_s
                .parse()
                .with_context(|| format!("invalid port '{port_s}'"))?;
            Ok((host, Some(port)))
        }
        None => {
            if s.is_empty() {
                return Err(anyhow!("empty host"));
            }
            Ok((s, None))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> AllowedHost {
        s.parse()
            .unwrap_or_else(|e| panic!("parse '{s}' failed: {e:#}"))
    }

    // ---- parsing ----

    #[test]
    fn parses_any() {
        assert_eq!(parse("*"), AllowedHost::Any);
    }

    #[test]
    fn parses_authority() {
        match parse("example.com") {
            AllowedHost::Authority(a) => {
                assert_eq!(a.host(), "example.com");
                assert_eq!(a.port_u16(), None);
            }
            other => panic!("expected Authority, got {other:?}"),
        }
    }

    #[test]
    fn parses_authority_with_port() {
        match parse("example.com:8080") {
            AllowedHost::Authority(a) => {
                assert_eq!(a.host(), "example.com");
                assert_eq!(a.port_u16(), Some(8080));
            }
            other => panic!("expected Authority, got {other:?}"),
        }
    }

    #[test]
    fn parses_url() {
        // Bare scheme + host + port (no path) — the canonical URL form for
        // a hosts policy. The path-rejection tests below cover the
        // anything-beyond-`/` failure modes.
        match parse("https://api.example.com:8443") {
            AllowedHost::Url(u) => {
                assert_eq!(u.scheme(), "https");
                assert_eq!(u.host_str(), Some("api.example.com"));
                assert_eq!(u.port(), Some(8443));
            }
            other => panic!("expected Url, got {other:?}"),
        }
    }

    #[test]
    fn parses_suffix_wildcard_no_scheme() {
        match parse("*.example.com") {
            AllowedHost::SuffixWildcard {
                suffix,
                scheme,
                port,
            } => {
                assert_eq!(suffix, ".example.com");
                assert!(scheme.is_none());
                assert!(port.is_none());
            }
            other => panic!("expected SuffixWildcard, got {other:?}"),
        }
    }

    #[test]
    fn parses_suffix_wildcard_lowercases_suffix() {
        match parse("*.Example.COM") {
            AllowedHost::SuffixWildcard { suffix, .. } => assert_eq!(suffix, ".example.com"),
            other => panic!("expected SuffixWildcard, got {other:?}"),
        }
    }

    #[test]
    fn parses_suffix_wildcard_with_scheme_and_port() {
        match parse("https://*.example.com:8443") {
            AllowedHost::SuffixWildcard {
                suffix,
                scheme,
                port,
            } => {
                assert_eq!(suffix, ".example.com");
                assert_eq!(scheme.as_ref().map(Scheme::as_str), Some("https"));
                assert_eq!(port, Some(8443));
            }
            other => panic!("expected SuffixWildcard, got {other:?}"),
        }
    }

    #[test]
    fn rejects_bare_star_prefix() {
        let err = "*example.com".parse::<AllowedHost>().unwrap_err();
        assert!(
            format!("{err:#}").contains("leading dot required"),
            "{err:#}"
        );
    }

    #[test]
    fn rejects_empty_string() {
        let err = "".parse::<AllowedHost>().unwrap_err();
        assert!(format!("{err:#}").contains("empty"));
    }

    #[test]
    fn rejects_invalid_port() {
        let err = "example.com:notaport".parse::<AllowedHost>().unwrap_err();
        // http::Authority reports its own error; just check parsing failed.
        assert!(format!("{err:#}").contains("invalid"));
    }

    // ---- path/query/fragment rejection on URL form ----
    //
    // `allowed_hosts` is a hosts policy. Silently dropping `/v1` would teach
    // users that `https://api/v1` restricts to that path, when it doesn't.
    // Bare trailing `/` is fine because that's what `url::Url` normalizes to.

    #[test]
    fn url_accepts_bare_trailing_slash() {
        let h: AllowedHost = "https://api.example.com/".parse().unwrap();
        assert!(matches!(h, AllowedHost::Url(_)));
    }

    #[test]
    fn url_accepts_no_path() {
        // url::Url::parse normalizes this to add the trailing /, but the
        // input form without it should also parse cleanly.
        let h: AllowedHost = "https://api.example.com".parse().unwrap();
        assert!(matches!(h, AllowedHost::Url(_)));
    }

    #[test]
    fn url_rejects_non_root_path() {
        let err = "https://api.example.com/v1"
            .parse::<AllowedHost>()
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("must not include a path"), "{msg}");
        assert!(msg.contains("/v1"), "{msg}");
    }

    #[test]
    fn url_rejects_query() {
        let err = "https://api.example.com/?q=1"
            .parse::<AllowedHost>()
            .unwrap_err();
        assert!(format!("{err:#}").contains("query"));
    }

    #[test]
    fn url_rejects_fragment() {
        let err = "https://api.example.com/#frag"
            .parse::<AllowedHost>()
            .unwrap_err();
        assert!(format!("{err:#}").contains("fragment"));
    }

    #[test]
    fn wildcard_scheme_accepts_bare_trailing_slash() {
        let h: AllowedHost = "https://*.example.com/".parse().unwrap();
        assert!(matches!(h, AllowedHost::SuffixWildcard { .. }));
    }

    #[test]
    fn wildcard_scheme_rejects_non_root_path() {
        let err = "https://*.example.com/v1"
            .parse::<AllowedHost>()
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("must not include a path"), "{msg}");
        assert!(msg.contains("/v1"), "{msg}");
    }

    // ---- display round-trip ----

    #[test]
    fn display_round_trips() {
        for s in [
            "*",
            "example.com",
            "example.com:8080",
            "https://api.example.com/",
            "*.example.com",
            "https://*.example.com:8443",
        ] {
            let parsed: AllowedHost = s.parse().unwrap();
            let re_parsed: AllowedHost = parsed.to_string().parse().unwrap();
            assert_eq!(parsed, re_parsed, "round-trip failed for {s}");
        }
    }

    // ---- matching ----

    fn uri(s: &str) -> Uri {
        s.parse()
            .unwrap_or_else(|e| panic!("URI '{s}' invalid: {e}"))
    }

    #[test]
    fn any_matches_everything() {
        let h = AllowedHost::Any;
        assert!(h.matches(&uri("http://foo.example.com:8080/x")));
        assert!(h.matches(&uri("https://bar")));
    }

    #[test]
    fn any_does_not_match_request_with_no_host() {
        // Relative URIs (path-only) have no host; nothing can match them.
        let h = AllowedHost::Any;
        assert!(!h.matches(&uri("/relative/path")));
    }

    #[test]
    fn authority_matches_exact_case_insensitive() {
        let h: AllowedHost = "Example.COM".parse().unwrap();
        assert!(h.matches(&uri("https://example.com:443")));
        assert!(h.matches(&uri("http://EXAMPLE.com")));
        assert!(!h.matches(&uri("http://api.example.com")));
    }

    #[test]
    fn authority_no_port_matches_any_request_port() {
        let h: AllowedHost = "example.com".parse().unwrap();
        assert!(h.matches(&uri("http://example.com")));
        assert!(h.matches(&uri("http://example.com:80")));
        assert!(h.matches(&uri("https://example.com:8443")));
    }

    #[test]
    fn authority_with_port_pins_port() {
        let h: AllowedHost = "example.com:8443".parse().unwrap();
        assert!(h.matches(&uri("https://example.com:8443")));
        assert!(!h.matches(&uri("https://example.com:443")));
        assert!(!h.matches(&uri("https://example.com")));
    }

    #[test]
    fn url_matches_scheme_host() {
        let h: AllowedHost = "https://api.example.com".parse().unwrap();
        assert!(h.matches(&uri("https://api.example.com")));
        assert!(!h.matches(&uri("http://api.example.com")));
        assert!(!h.matches(&uri("https://other.example.com")));
    }

    #[test]
    fn url_ignores_request_path_and_query() {
        // Locking the documented "path/query not consulted at match time"
        // semantic — request path/query shouldn't affect whether the policy
        // allows the request, only host/scheme/port do.
        let h: AllowedHost = "https://api.example.com".parse().unwrap();
        assert!(h.matches(&uri("https://api.example.com/v1/users?id=5")));
        assert!(h.matches(&uri("https://api.example.com/admin")));
    }

    #[test]
    fn suffix_wildcard_matches_subdomain_not_bare() {
        let h: AllowedHost = "*.example.com".parse().unwrap();
        assert!(h.matches(&uri("http://api.example.com")));
        assert!(h.matches(&uri("http://a.b.example.com")));
        assert!(!h.matches(&uri("http://example.com")));
        assert!(!h.matches(&uri("http://evil.com")));
    }

    #[test]
    fn suffix_wildcard_is_case_insensitive() {
        let h: AllowedHost = "*.Example.COM".parse().unwrap();
        assert!(h.matches(&uri("http://Sub.EXAMPLE.com")));
    }

    #[test]
    fn suffix_wildcard_scheme_and_port_pin() {
        let h: AllowedHost = "https://*.example.com:8443".parse().unwrap();
        assert!(h.matches(&uri("https://api.example.com:8443")));
        assert!(!h.matches(&uri("http://api.example.com:8443")));
        assert!(!h.matches(&uri("https://api.example.com:443")));
    }

    #[test]
    fn suffix_wildcard_with_port_pin_rejects_request_without_port() {
        // Policy explicitly pins port 8443; a request omitting the port
        // entirely (most defaults) does NOT match. This is intentional —
        // matches "unspecified port on the request" to "explicit port on
        // the policy" would be ambiguous and weaken the pin.
        let h: AllowedHost = "*.example.com:8443".parse().unwrap();
        assert!(h.matches(&uri("http://api.example.com:8443")));
        assert!(!h.matches(&uri("http://api.example.com")));
    }

    #[test]
    fn ipv6_authority_matches() {
        // Authority::from_str accepts bracketed IPv6 with port; the matcher
        // should compare host strings as the http crate exposes them (no
        // brackets in Uri::host()).
        let h: AllowedHost = "[::1]:8080".parse().unwrap();
        assert!(h.matches(&uri("http://[::1]:8080")));
        assert!(!h.matches(&uri("http://[::1]:9090")));
    }

    #[test]
    fn ipv6_authority_without_port_parses_and_matches_any_port() {
        // IPv6 hosts contain colons inside the brackets, which previously
        // tripped the naive `as_str().contains(':')` port-suffix detector
        // and made `[::1]` (no port) fail to parse. Lock the correct
        // behavior: bracketed IPv6 with no port is a valid Authority and
        // matches any request port.
        let h: AllowedHost = "[::1]".parse().unwrap();
        assert!(matches!(h, AllowedHost::Authority(_)));
        assert!(h.matches(&uri("http://[::1]")));
        assert!(h.matches(&uri("http://[::1]:8080")));
        assert!(h.matches(&uri("https://[::1]:443")));
    }

    #[test]
    fn ipv6_authority_with_invalid_port_is_rejected() {
        // Lock the other side: a port suffix that doesn't parse as u16
        // must still error for bracketed IPv6, not be silently accepted
        // as "no port".
        let err = "[::1]:notaport".parse::<AllowedHost>().unwrap_err();
        assert!(format!("{err:#}").contains("invalid"), "{err:#}");
    }

    #[test]
    fn localhost_authority_matches() {
        // Single-label host — most common dev policy. Tests that the
        // RFC-1123 single-label form survives the K8s-regex / parser
        // round-trip even though the regex's per-label rule is the loosest
        // place this could regress.
        let h: AllowedHost = "localhost:8080".parse().unwrap();
        assert!(h.matches(&uri("http://localhost:8080")));
        assert!(!h.matches(&uri("http://localhost:9090")));
    }

    // ---- serde ----

    #[test]
    fn deserialize_from_json_list() {
        let json = r#"["*", "example.com", "example.com:8443", "https://api.example.com", "*.example.com"]"#;
        let parsed: Vec<AllowedHost> = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.len(), 5);
        assert!(matches!(parsed[0], AllowedHost::Any));
        assert!(matches!(parsed[4], AllowedHost::SuffixWildcard { .. }));
    }

    #[test]
    fn deserialize_rejects_invalid_entry() {
        let json = r#"["*com"]"#;
        let err = serde_json::from_str::<Vec<AllowedHost>>(json).unwrap_err();
        assert!(format!("{err}").contains("leading dot"), "{err}");
    }

    #[test]
    fn serialize_round_trips_string() {
        let h: AllowedHost = "https://*.example.com:8443".parse().unwrap();
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, "\"https://*.example.com:8443\"");
    }
}
