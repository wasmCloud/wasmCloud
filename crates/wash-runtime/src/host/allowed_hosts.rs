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
//! - Hostname comparison is ASCII-case-insensitive. An entry written in
//!   Unicode is IDNA-encoded when parsed, in every form, so it is stored and
//!   rendered as punycode.
//! - `Authority` and `SuffixWildcard` with no explicit port match any
//!   request port. With an explicit port they match exact.
//! - `SuffixWildcard` with no explicit scheme matches any scheme. With an
//!   explicit scheme it matches exact (case-insensitive).
//! - `Url` matches scheme + host + port exactly. Paths on policy entries
//!   aren't allowed (see above) and request paths/queries are not
//!   inspected by the matcher.
//!
//! # Raw sockets
//!
//! A `wasi:sockets` destination is an address with no name or scheme. An entry
//! permits it by naming the address ([`AllowedHost::permits_addr`]) or by
//! naming a host the guest resolved into it ([`AllowedHost::permits_name`]).
//! An entry naming a host also lets the guest resolve it
//! ([`AllowedHost::permits_lookup`]), so one entry is the whole grant.
//! Either way the port is [`AllowedHost::socket_port`]: explicit, else the
//! scheme's default, else any. A native plugin's endpoint is held to the same
//! port rule, whether it names an address or a host.
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

use core::net::{IpAddr, SocketAddr};
use std::borrow::Cow;
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

impl AllowedHost {
    /// Returns `true` if a raw socket connection to `addr` satisfies this entry.
    ///
    /// This is the same allowlist `wasi:http` matches against, evaluated for a
    /// destination that is an address rather than a URI — a guest opening a
    /// socket named no host, so only entries that can speak about an address
    /// can match it:
    ///
    /// - [`AllowedHost::Any`] matches, subject to the range policy applied
    ///   separately. `*` grants the internet, not the machine the host runs on.
    /// - [`AllowedHost::Authority`] and [`AllowedHost::Url`] match when the
    ///   entry's host is a literal IP equal to `addr`, and the port agrees.
    /// - [`AllowedHost::SuffixWildcard`] never matches: a suffix describes
    ///   names, and an address has none. An entry naming a host permits an
    ///   address only through [`Self::permits_name`], for a name the guest
    ///   resolved.
    ///
    /// The port is [`Self::socket_port`].
    #[must_use]
    pub fn permits_addr(&self, addr: SocketAddr) -> bool {
        let host = match self {
            AllowedHost::Any => return true,
            AllowedHost::Authority(authority) => authority.host(),
            AllowedHost::Url(url) => match url.host_str() {
                Some(host) => host,
                None => return false,
            },
            AllowedHost::SuffixWildcard { .. } => return false,
        };
        host_is_addr(host, addr.ip()) && self.socket_port_matches(addr.port())
    }

    /// Returns `true` if this entry names `name` and permits `port` on it.
    ///
    /// For a raw socket to an address the guest resolved `name` into — see
    /// [`crate::sockets::resolved_names`]. `name` is that module's normalized
    /// form: lowercase, IDNA-encoded, no terminal root dot — which is what
    /// parsing made of this entry's own host. A wildcard requires at least one
    /// label before its suffix, as it does for `wasi:http`.
    ///
    /// [`AllowedHost::Any`] answers `false`: it names nothing, and already
    /// permits every address through [`Self::permits_addr`].
    #[must_use]
    pub fn permits_name(&self, name: &str, port: u16) -> bool {
        let named = match self {
            AllowedHost::Any => false,
            AllowedHost::Authority(authority) => same_name(authority.host(), name),
            AllowedHost::Url(url) => url.host_str().is_some_and(|host| same_name(host, name)),
            AllowedHost::SuffixWildcard { suffix, .. } => {
                let suffix = suffix.strip_suffix('.').unwrap_or(suffix);
                name.len() > suffix.len()
                    && name
                        .get(name.len() - suffix.len()..)
                        .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
            }
        };
        named && self.socket_port_matches(port)
    }

    /// Returns `true` if this entry permits resolving `host`.
    ///
    /// Being allowed to connect to a name includes being allowed to look it
    /// up: a lookup reaches no further than the connection the entry already
    /// grants, so asking an operator to repeat the name in
    /// `allowedIpNameLookups` buys nothing. Port and scheme do not bear on a
    /// lookup. `host` is the parsed, punycoded form of the requested name.
    ///
    /// [`AllowedHost::Any`] answers `false`. It names no host, and it is what
    /// the wash config layer substitutes for an omitted `allowedHosts`; a
    /// default nobody wrote must not open every lookup. Resolving any name
    /// stays an explicit `allowedIpNameLookups: ["*"]`.
    #[must_use]
    pub fn permits_lookup(&self, host: &url::Host<String>) -> bool {
        let policy_host = match self {
            AllowedHost::Any => return false,
            AllowedHost::Authority(authority) => authority.host(),
            AllowedHost::Url(url) => match url.host_str() {
                Some(host) => host,
                None => return false,
            },
            AllowedHost::SuffixWildcard { .. } => "",
        };
        match host {
            url::Host::Ipv4(addr) => host_is_addr(policy_host, IpAddr::V4(*addr)),
            url::Host::Ipv6(addr) => host_is_addr(policy_host, IpAddr::V6(*addr)),
            url::Host::Domain(domain) => {
                let name = domain.strip_suffix('.').unwrap_or(domain);
                // Any port: `permits_name` with the entry's own, or an
                // arbitrary one where it names none.
                self.permits_name(name, self.socket_port().unwrap_or(0))
            }
        }
    }

    /// The one port a raw socket may use under this entry, or `None` for any.
    ///
    /// An explicit port is exact. An entry with a scheme and no port means that
    /// scheme's default port — `https://db.internal` is 443, not every port on
    /// the host. The scheme is otherwise not consulted: a raw socket has none,
    /// and nothing here can hold a guest to TLS. A scheme with no known default
    /// (`postgres://`, `nats://`) and no port leaves the port open.
    #[must_use]
    pub fn socket_port(&self) -> Option<u16> {
        match self {
            AllowedHost::Any => None,
            AllowedHost::Authority(authority) => authority.port_u16(),
            AllowedHost::Url(url) => url.port_or_known_default(),
            AllowedHost::SuffixWildcard { scheme, port, .. } => {
                port.or_else(|| scheme.as_ref().and_then(|s| default_port(s.as_str())))
            }
        }
    }

    fn socket_port_matches(&self, port: u16) -> bool {
        match self.socket_port() {
            Some(permitted) => permitted == port,
            None => true,
        }
    }
}

/// The default port of a scheme, for the schemes [`Url::port_or_known_default`]
/// knows. The one table for it: `wasi:http` routing reads it too.
pub(crate) fn default_port(scheme: &str) -> Option<u16> {
    // Compared in place: this runs per request and per connect.
    const PORTS: [(&str, u16); 5] = [
        ("http", 80),
        ("https", 443),
        ("ws", 80),
        ("wss", 443),
        ("ftp", 21),
    ];
    PORTS
        .iter()
        .find(|(known, _)| scheme.eq_ignore_ascii_case(known))
        .map(|(_, port)| *port)
}

/// Whether a policy entry's host text is the normalized `name`, ignoring case
/// and a terminal root dot on the entry.
fn same_name(policy_host: &str, name: &str) -> bool {
    policy_host
        .strip_suffix('.')
        .unwrap_or(policy_host)
        .eq_ignore_ascii_case(name)
}

/// Whether a policy entry's host text is a literal IP equal to `addr`.
///
/// Compares parsed addresses, so `::ffff:10.0.0.1` and `10.0.0.1` are the same
/// destination, and bracketed IPv6 authority text (`[::1]`) is unwrapped first.
fn host_is_addr(host: &str, addr: IpAddr) -> bool {
    let host = host.strip_prefix('[').unwrap_or(host);
    let host = host.strip_suffix(']').unwrap_or(host);
    host.parse::<IpAddr>()
        .is_ok_and(|policy| policy.to_canonical() == addr.to_canonical())
}

/// Returns `true` if a raw socket connection to `addr` satisfies any entry.
///
/// An empty `policy` denies every connection, mirroring
/// [`check_allowed_hosts`](crate::host::http::check_allowed_hosts) for
/// `wasi:http`.
#[must_use]
pub fn check_allowed_addr(policy: &[AllowedHost], addr: SocketAddr) -> bool {
    policy.iter().any(|entry| entry.permits_addr(addr))
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
                    suffix: format!(".{}", idna_host(suffix_no_dot).to_ascii_lowercase()),
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
            // `Url` IDNA-encodes a host only under a scheme it knows, so a
            // `postgres://` entry is encoded here first.
            let url = Url::parse(&format!("{scheme_part}://{}", idna_authority(rest)))
                .context("not a valid URL")?;
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
                suffix: format!(".{}", idna_host(suffix_no_dot).to_ascii_lowercase()),
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
        let authority =
            Authority::from_str(&idna_authority(trimmed)).context("invalid host[:port]")?;
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

/// `host` IDNA-encoded, so an entry written in Unicode compares equal to the
/// punycode a request or a lookup carries. ASCII is returned untouched.
fn idna_host(host: &str) -> Cow<'_, str> {
    if host.is_ascii() {
        return Cow::Borrowed(host);
    }
    match url::Host::parse(host) {
        Ok(url::Host::Domain(domain)) => Cow::Owned(domain),
        _ => Cow::Borrowed(host),
    }
}

/// [`idna_host`] applied to the host of a `host[:port][/]` string. A host that
/// is not ASCII is not a bracketed IPv6 literal, so it ends at the first `:`
/// or `/`.
fn idna_authority(text: &str) -> Cow<'_, str> {
    if text.is_ascii() {
        return Cow::Borrowed(text);
    }
    let end = text.find([':', '/']).unwrap_or(text.len());
    let (host, tail) = text.split_at(end);
    Cow::Owned(format!("{}{tail}", idna_host(host)))
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

    fn entry(s: &str) -> AllowedHost {
        s.parse().unwrap()
    }

    #[test]
    fn a_name_entry_permits_its_name_on_its_port() {
        assert!(entry("db.internal").permits_name("db.internal", 5432));
        assert!(entry("DB.Internal:5432").permits_name("db.internal", 5432));
        assert!(entry("db.internal.:5432").permits_name("db.internal", 5432));
        assert!(!entry("db.internal:5432").permits_name("db.internal", 6379));
        assert!(!entry("db.internal").permits_name("xdb.internal", 5432));
        assert!(!entry("10.0.0.5").permits_name("db.internal", 5432));
        assert!(!entry("*").permits_name("db.internal", 5432));
    }

    /// A lookup key is punycode, so every entry form has to be too.
    #[test]
    fn an_entry_written_in_unicode_matches_the_punycode_a_lookup_records() {
        let apex = "xn--bcher-kva.example";
        for text in [
            "bücher.example:5432",
            "BÜCHER.example:5432",
            "postgres://bücher.example:5432",
            "https://bücher.example:5432",
        ] {
            assert!(entry(text).permits_name(apex, 5432), "{text}");
        }
        for text in ["*.bücher.example:5432", "postgres://*.bücher.example:5432"] {
            let e = entry(text);
            assert!(e.permits_name(&format!("db.{apex}"), 5432), "{text}");
            assert!(!e.permits_name(apex, 5432), "{text}");
        }

        // The same entries serve `wasi:http`, where the request carries
        // punycode as well.
        let request: Uri = "https://db.xn--bcher-kva.example/".parse().unwrap();
        assert!(entry("*.bücher.example").matches(&request));
        assert_eq!(
            entry("*.bücher.example").to_string(),
            "*.xn--bcher-kva.example"
        );
    }

    #[test]
    fn a_wildcard_entry_needs_a_subdomain_on_a_label_boundary() {
        let wildcard = entry("*.svc.local:5432");
        assert!(wildcard.permits_name("db.svc.local", 5432));
        assert!(wildcard.permits_name("a.b.svc.local", 5432));
        assert!(!wildcard.permits_name("svc.local", 5432));
        assert!(!wildcard.permits_name("evilsvc.local", 5432));
        assert!(!wildcard.permits_name("db.svc.local", 80));
        assert!(entry("*.svc.local").permits_name("db.svc.local", 80));
    }

    /// A raw socket has no scheme, so a scheme-bearing entry with no port
    /// means that scheme's default port rather than every port on the host —
    /// for a literal address and a resolved name alike.
    #[test]
    fn a_scheme_with_no_port_means_its_default_port_for_sockets() {
        for (text, port) in [
            ("https://db.internal", 443),
            ("http://db.internal", 80),
            ("https://*.internal", 443),
        ] {
            let e = entry(text);
            assert_eq!(e.socket_port(), Some(port), "{text}");
            assert!(e.permits_name("db.internal", port), "{text}");
            assert!(!e.permits_name("db.internal", 5432), "{text}");
        }

        let literal = entry("https://10.0.0.5");
        assert!(literal.permits_addr("10.0.0.5:443".parse().unwrap()));
        assert!(!literal.permits_addr("10.0.0.5:5432".parse().unwrap()));

        // An explicit port wins, and a scheme with no known default leaves the
        // port open.
        assert!(entry("https://10.0.0.5:8443").permits_addr("10.0.0.5:8443".parse().unwrap()));
        assert_eq!(entry("postgres://db.internal").socket_port(), None);
        assert!(entry("postgres://10.0.0.5").permits_addr("10.0.0.5:5432".parse().unwrap()));
        assert!(entry("postgres://db.internal:5432").permits_name("db.internal", 5432));
        assert!(!entry("postgres://db.internal:5432").permits_name("db.internal", 5433));
    }

    /// Naming a host is the whole grant: it may be connected to, so it may be
    /// resolved. `*` names nothing and opens no lookup.
    #[test]
    fn an_entry_naming_a_host_permits_resolving_it() {
        let host = |s: &str| url::Host::parse(s).unwrap();

        for text in [
            "db.internal",
            "db.internal:5432",
            "postgres://db.internal:5432",
        ] {
            let e = entry(text);
            assert!(e.permits_lookup(&host("db.internal")), "{text}");
            assert!(e.permits_lookup(&host("DB.internal.")), "{text}");
            assert!(!e.permits_lookup(&host("other.internal")), "{text}");
        }

        let wildcard = entry("https://*.svc.local");
        assert!(wildcard.permits_lookup(&host("db.svc.local")));
        assert!(!wildcard.permits_lookup(&host("svc.local")));
        assert!(!wildcard.permits_lookup(&host("evilsvc.local")));

        assert!(entry("10.0.0.5:5432").permits_lookup(&host("10.0.0.5")));
        assert!(!entry("10.0.0.5:5432").permits_lookup(&host("10.0.0.6")));
        assert!(!entry("db.internal").permits_lookup(&host("10.0.0.5")));

        assert!(!entry("*").permits_lookup(&host("db.internal")));
    }
}
