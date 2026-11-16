//! Secrets the host writes into a workload's outgoing HTTP requests.
//!
//! The guest holds a placeholder, never the secret. It receives the
//! placeholder where it would have received the secret (an environment
//! variable or config value) and sends it with an unmodified HTTP client. After
//! the request clears `allowed_hosts`, the ingress swaps each placeholder for
//! its secret, but only in the headers or query parameters, for the
//! destinations, and on the routes that secret names. A placeholder anywhere
//! else refuses the request, and so does any placeholder the caller holds no
//! secret for.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hyper::Method;
use hyper::header::{self, HeaderName, HeaderValue};
use hyper::http::uri::Scheme;
use zeroize::Zeroizing;

use crate::host::allowed_hosts::AllowedHost;

/// Every placeholder ends in this marker and at least [`MIN_RANDOM`] random
/// alphanumerics, so the host can recognise one it holds no secret for.
pub const PLACEHOLDER_MARKER: &str = "wash_ph_";

/// Random characters after the marker: about 119 bits, so no two placeholders
/// and no real value ever collide.
pub const MIN_RANDOM: usize = 20;

/// The guest that issued an outgoing request, as the host attests it.
#[derive(Clone, Copy, Debug)]
pub struct EgressCaller<'a> {
    pub workload_id: &'a str,
    /// `None` when the request reached the host without a component behind
    /// it.
    ///
    /// Taken from the store's active component, which concurrent linked calls
    /// can leave pointing at the wrong component (see
    /// `AccessorActiveCtxGuard`). `workload_id` has no such caveat.
    pub component_id: Option<&'a str>,
}

/// Why a placeholder was not substituted. Every variant refuses the request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialError {
    Unavailable,
    NotTls,
    /// The destination is not one the secret is for.
    Destination,
    /// The method or path is not one the secret is for.
    Route,
    /// The placeholder is somewhere the secret is not for.
    Misplaced,
    /// A placeholder the caller holds no secret for.
    Unknown,
    InvalidValue,
}

impl std::fmt::Display for CredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "secret is not available",
            Self::NotTls => "secret refused for a request that is not https",
            Self::Destination => "secret is not for this destination",
            Self::Route => "secret is not for this method or path",
            Self::Misplaced => "placeholder is somewhere the host does not substitute",
            Self::Unknown => "placeholder is not one this workload holds",
            Self::InvalidValue => "secret is not a valid value where it is placed",
        })
    }
}

impl std::error::Error for CredentialError {}

/// Where a secret's value comes from.
///
/// Called on the request path, so it must answer from what it already holds:
/// a source refreshes its value elsewhere and returns
/// [`CredentialError::Unavailable`] once what it holds has expired.
pub trait EgressCredentialSource: Send + Sync + 'static {
    fn current(&self) -> Result<Zeroizing<Vec<u8>>, CredentialError>;
}

/// A value served only until its lease runs out or it is revoked.
///
/// Whoever holds the authority renews it. If that authority becomes
/// unreachable, the value stops being served when the last lease ends, which
/// bounds how long a revoked secret can outlive its revocation.
#[derive(Default)]
pub struct LeasedSource {
    held: Mutex<Option<(Zeroizing<Vec<u8>>, Instant)>>,
}

impl LeasedSource {
    pub fn renew(&self, value: Zeroizing<Vec<u8>>, valid_for: Duration) {
        let until = Instant::now() + valid_for;
        *self.held.lock().unwrap_or_else(|e| e.into_inner()) = Some((value, until));
    }

    pub fn revoke(&self) {
        self.held.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

impl EgressCredentialSource for LeasedSource {
    fn current(&self) -> Result<Zeroizing<Vec<u8>>, CredentialError> {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        match &*held {
            Some((value, until)) if Instant::now() < *until => Ok(value.clone()),
            _ => {
                held.take();
                Err(CredentialError::Unavailable)
            }
        }
    }
}

/// A method and path prefix a secret may be sent on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretRoute {
    method: Option<Method>,
    path_prefix: Arc<str>,
}

impl SecretRoute {
    /// `method` of `None` allows any method. `path_prefix` matches whole
    /// segments: `/v1/repos` allows `/v1/repos/x` but not `/v1/reposx`.
    pub fn new(method: Option<Method>, path_prefix: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            path_prefix.starts_with('/') && !is_ambiguous(path_prefix),
            "a route's path prefix starts with `/` and has no dot segments or escapes"
        );
        Ok(Self {
            method,
            path_prefix: path_prefix.into(),
        })
    }

    fn allows(&self, method: &Method, path: &str) -> bool {
        self.method.as_ref().is_none_or(|m| m == method) && is_under(path, &self.path_prefix)
    }
}

/// A secret a workload may send by placeholder.
#[derive(Clone)]
pub struct EgressSecret {
    placeholder: Arc<str>,
    headers: Arc<[HeaderName]>,
    query: Arc<[Arc<str>]>,
    destinations: Arc<[AllowedHost]>,
    routes: Arc<[SecretRoute]>,
    source: Arc<dyn EgressCredentialSource>,
}

/// Headers that describe the message or connection rather than the caller.
const RESERVED: [HeaderName; 7] = [
    header::HOST,
    header::CONTENT_LENGTH,
    header::TRANSFER_ENCODING,
    header::CONNECTION,
    header::TE,
    header::UPGRADE,
    header::CONTENT_TYPE,
];

const ALPHANUMERIC: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// A new placeholder: `prefix`, the marker, then random alphanumerics, padded
/// to `len` characters when that is longer than the minimum.
///
/// The prefix lets a placeholder pass an SDK that checks a key's shape before
/// sending it, such as `sk-` or `ghp_`.
pub fn mint_placeholder(prefix: &str, len: usize) -> anyhow::Result<String> {
    anyhow::ensure!(
        prefix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            && !prefix.contains(PLACEHOLDER_MARKER),
        "a placeholder prefix uses only [A-Za-z0-9_-] and does not contain `{PLACEHOLDER_MARKER}`"
    );
    let fixed = prefix.len() + PLACEHOLDER_MARKER.len();
    let target = fixed + len.saturating_sub(fixed).max(MIN_RANDOM);
    let mut placeholder = format!("{prefix}{PLACEHOLDER_MARKER}");
    while placeholder.len() < target {
        let random = rand::random::<[u8; 32]>();
        let chars = random
            .iter()
            .filter_map(|byte| ALPHANUMERIC.get(usize::from(byte % 62)))
            .take(target - placeholder.len());
        placeholder.extend(chars.map(|c| char::from(*c)));
    }
    Ok(placeholder)
}

impl EgressSecret {
    /// Substituted only in `authorization` until [`Self::in_headers`] or
    /// [`Self::in_query`] says otherwise, and for no destination until
    /// [`Self::for_destinations`].
    ///
    /// The placeholder is `[prefix]wash_ph_` and at least [`MIN_RANDOM`]
    /// alphanumerics (see [`mint_placeholder`]).
    pub fn new(
        placeholder: impl Into<Arc<str>>,
        source: Arc<dyn EgressCredentialSource>,
    ) -> anyhow::Result<Self> {
        let placeholder = placeholder.into();
        let shaped = placeholder
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            && placeholder
                .split_once(PLACEHOLDER_MARKER)
                .is_some_and(|(prefix, random)| {
                    !prefix.contains(PLACEHOLDER_MARKER)
                        && random.len() >= MIN_RANDOM
                        && random.bytes().all(|b| b.is_ascii_alphanumeric())
                });
        anyhow::ensure!(
            shaped,
            "a placeholder is an optional [A-Za-z0-9_-] prefix, `{PLACEHOLDER_MARKER}`, then at \
             least {MIN_RANDOM} alphanumerics"
        );
        Ok(Self {
            placeholder,
            headers: Arc::from([header::AUTHORIZATION]),
            query: Arc::from([]),
            destinations: Arc::from([]),
            routes: Arc::from([]),
            source,
        })
    }

    pub fn in_headers(
        mut self,
        headers: impl IntoIterator<Item = HeaderName>,
    ) -> anyhow::Result<Self> {
        let headers: Arc<[HeaderName]> = headers.into_iter().collect();
        if let Some(name) = headers.iter().find(|h| RESERVED.contains(h)) {
            anyhow::bail!("`{name}` cannot carry a secret");
        }
        self.headers = headers;
        Ok(self)
    }

    /// Query parameters whose whole value may be the placeholder, for APIs
    /// that take a key in the URL.
    pub fn in_query<S: AsRef<str>>(mut self, names: impl IntoIterator<Item = S>) -> Self {
        self.query = names.into_iter().map(|n| Arc::from(n.as_ref())).collect();
        self
    }

    /// Exact https hosts only. A wildcard would let the guest send the secret
    /// to any name under it, including one it controls.
    pub fn for_destinations(
        mut self,
        destinations: impl IntoIterator<Item = AllowedHost>,
    ) -> anyhow::Result<Self> {
        let destinations: Arc<[AllowedHost]> = destinations.into_iter().collect();
        anyhow::ensure!(!destinations.is_empty(), "a secret needs a destination");
        for destination in destinations.iter() {
            match destination {
                AllowedHost::Authority(_) => {}
                AllowedHost::Url(url) if url.scheme() == "https" => {}
                AllowedHost::Url(_) => anyhow::bail!("a secret's destination must be https"),
                _ => anyhow::bail!("a secret's destination must be an exact host, not a wildcard"),
            }
        }
        self.destinations = destinations;
        Ok(self)
    }

    /// Methods and paths the secret may be sent on. None means any.
    pub fn for_routes(mut self, routes: impl IntoIterator<Item = SecretRoute>) -> Self {
        self.routes = routes.into_iter().collect();
        self
    }

    pub fn placeholder(&self) -> &str {
        &self.placeholder
    }

    fn found_in(&self, haystack: &[u8]) -> bool {
        contains(haystack, self.placeholder.as_bytes())
    }

    fn check<B>(
        &self,
        placement: Placement<'_>,
        request: &hyper::Request<B>,
    ) -> Result<(), CredentialError> {
        let declared = match placement {
            Placement::Header(name) => self.headers.contains(name),
            Placement::Query(name) => self.query.iter().any(|q| **q == *name),
        };
        if !declared {
            return Err(CredentialError::Misplaced);
        }
        let uri = request.uri();
        if uri.scheme() != Some(&Scheme::HTTPS) {
            return Err(CredentialError::NotTls);
        }
        if !self.destinations.iter().any(|d| d.matches(uri)) {
            return Err(CredentialError::Destination);
        }
        if !self.routes.is_empty()
            && (is_ambiguous(uri.path())
                || !self
                    .routes
                    .iter()
                    .any(|r| r.allows(request.method(), uri.path())))
        {
            return Err(CredentialError::Route);
        }
        Ok(())
    }

    fn refuse(&self, error: CredentialError) -> Refused {
        Refused {
            placeholder: Some(Arc::clone(&self.placeholder)),
            error,
        }
    }
}

impl std::fmt::Debug for EgressSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EgressSecret")
            .field("placeholder", &self.placeholder)
            .field("headers", &self.headers)
            .field("query", &self.query)
            .field("destinations", &self.destinations)
            .field("routes", &self.routes)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy)]
enum Placement<'a> {
    Header(&'a HeaderName),
    Query(&'a str),
}

/// Set on a request once the host has written a secret into it.
///
/// An [`crate::host::http::OutgoingHandler`] must send such a request only
/// over TLS verified against the host's own trust roots, ignoring any
/// per-caller TLS override.
#[derive(Clone, Copy, Debug)]
pub struct CarriesSecret;

/// One use or refusal of a placeholder, for audit.
#[derive(Debug)]
pub struct SecretUse<'a> {
    pub caller: EgressCaller<'a>,
    /// `None` when the refused placeholder is not one the caller holds.
    pub placeholder: Option<&'a str>,
    pub method: &'a Method,
    pub authority: &'a str,
    /// Without the query, which may hold the secret.
    pub path: &'a str,
    pub outcome: Result<(), &'a CredentialError>,
}

/// A refused substitution and the placeholder it was for, when known.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Refused {
    pub placeholder: Option<Arc<str>>,
    pub error: CredentialError,
}

impl From<CredentialError> for Refused {
    fn from(error: CredentialError) -> Self {
        Self {
            placeholder: None,
            error,
        }
    }
}

/// Swap every placeholder in `request` for its secret.
///
/// Returns the placeholders substituted. Placeholders are also found inside
/// `Basic` credentials, which carry them base64-encoded.
pub(crate) fn substitute<B>(
    request: &mut hyper::Request<B>,
    secrets: &[EgressSecret],
) -> Result<Vec<Arc<str>>, Refused> {
    let marker = PLACEHOLDER_MARKER.as_bytes();
    let uri = request.uri();
    let in_uri = [
        uri.authority().map_or("", |a| a.as_str()),
        uri.path_and_query().map_or("", |p| p.as_str()),
    ]
    .iter()
    .any(|part| contains(part.as_bytes(), marker));
    let in_headers = request.headers().iter().any(|(name, value)| {
        contains(value.as_bytes(), marker)
            || basic_credentials(name, value).is_some_and(|b| contains(&b, marker))
    });
    if !in_uri && !in_headers {
        return Ok(Vec::new());
    }

    let mut used = Vec::new();
    let uri = if in_uri {
        substitute_uri(request, secrets, &mut used)?
    } else {
        None
    };
    let rewrites = substitute_headers(request, secrets, &mut used)?;

    if let Some(uri) = uri {
        *request.uri_mut() = uri;
    }
    let mut rewrites = rewrites.into_iter().peekable();
    for (index, (_, value)) in request.headers_mut().iter_mut().enumerate() {
        if let Some((_, rewritten)) = rewrites.next_if(|(i, _)| *i == index) {
            *value = rewritten;
        }
    }
    if !used.is_empty() {
        request.extensions_mut().insert(CarriesSecret);
    }
    Ok(used)
}

/// The URI with declared query values substituted, or `None` if it holds a
/// placeholder-looking marker but nothing to substitute.
fn substitute_uri<B>(
    request: &hyper::Request<B>,
    secrets: &[EgressSecret],
    used: &mut Vec<Arc<str>>,
) -> Result<Option<hyper::Uri>, Refused> {
    let uri = request.uri();
    let authority = uri.authority().map_or("", |a| a.as_str());
    for part in [authority, uri.path()] {
        refuse_any_placeholder(part.as_bytes(), secrets)?;
    }
    let Some(query) = uri.query() else {
        return Ok(None);
    };
    let mut changed = false;
    let mut pieces = Vec::new();
    for piece in query.split('&') {
        if !contains(piece.as_bytes(), PLACEHOLDER_MARKER.as_bytes()) {
            pieces.push(Zeroizing::new(piece.to_string()));
            continue;
        }
        let (raw_name, _) = piece.split_once('=').unwrap_or((piece, ""));
        let Some((name, value)) = url::form_urlencoded::parse(piece.as_bytes()).next() else {
            pieces.push(Zeroizing::new(piece.to_string()));
            continue;
        };
        refuse_any_placeholder(name.as_bytes(), secrets)?;
        let Some(secret) = secrets.iter().find(|s| *s.placeholder == *value) else {
            refuse_any_placeholder(value.as_bytes(), secrets)?;
            pieces.push(Zeroizing::new(piece.to_string()));
            continue;
        };
        secret
            .check(Placement::Query(&name), request)
            .map_err(|e| secret.refuse(e))?;
        let value = secret.source.current().map_err(|e| secret.refuse(e))?;
        let value = std::str::from_utf8(&value)
            .map_err(|_| secret.refuse(CredentialError::InvalidValue))?;
        let encoded: String = url::form_urlencoded::byte_serialize(value.as_bytes()).collect();
        pieces.push(Zeroizing::new(format!("{raw_name}={encoded}")));
        note(used, &secret.placeholder);
        changed = true;
    }
    if !changed {
        return Ok(None);
    }
    let path_and_query = Zeroizing::new(format!("{}?{}", uri.path(), join(&pieces)));
    let mut builder = hyper::Uri::builder().path_and_query(path_and_query.as_str());
    if let Some(scheme) = uri.scheme() {
        builder = builder.scheme(scheme.clone());
    }
    if let Some(authority) = uri.authority() {
        builder = builder.authority(authority.clone());
    }
    builder
        .build()
        .map(Some)
        .map_err(|_| CredentialError::InvalidValue.into())
}

fn substitute_headers<B>(
    request: &hyper::Request<B>,
    secrets: &[EgressSecret],
    used: &mut Vec<Arc<str>>,
) -> Result<Vec<(usize, HeaderValue)>, Refused> {
    let mut rewrites = Vec::new();
    let mut carried: Vec<&HeaderName> = Vec::new();
    for (index, (name, value)) in request.headers().iter().enumerate() {
        let basic = basic_credentials(name, value);
        let text: &[u8] = basic.as_deref().map_or(value.as_bytes(), |b| b);
        if !contains(text, PLACEHOLDER_MARKER.as_bytes()) {
            continue;
        }
        let found: Vec<&EgressSecret> = secrets.iter().filter(|s| s.found_in(text)).collect();
        let mut rest = Zeroizing::new(text.to_vec());
        for secret in &found {
            rest = Zeroizing::new(replace(&rest, secret.placeholder.as_bytes(), b""));
        }
        if has_placeholder(&rest) {
            return Err(CredentialError::Unknown.into());
        }
        let Some(first) = found.first() else {
            continue;
        };
        if carried.contains(&name) {
            return Err(first.refuse(CredentialError::Misplaced));
        }
        carried.push(name);
        let mut replaced = Zeroizing::new(text.to_vec());
        for secret in found {
            secret
                .check(Placement::Header(name), request)
                .map_err(|e| secret.refuse(e))?;
            let value = secret.source.current().map_err(|e| secret.refuse(e))?;
            replaced = Zeroizing::new(replace(&replaced, secret.placeholder.as_bytes(), &value));
            note(used, &secret.placeholder);
        }
        let bytes = if basic.is_some() {
            Zeroizing::new(format!("Basic {}", STANDARD.encode(&*replaced)).into_bytes())
        } else {
            replaced
        };
        let mut value =
            HeaderValue::from_bytes(&bytes).map_err(|_| CredentialError::InvalidValue)?;
        value.set_sensitive(true);
        rewrites.push((index, value));
    }
    Ok(rewrites)
}

/// Refuse if `text` holds any placeholder: misplaced if the caller holds it,
/// unknown if not.
fn refuse_any_placeholder(text: &[u8], secrets: &[EgressSecret]) -> Result<(), Refused> {
    if let Some(secret) = secrets.iter().find(|s| s.found_in(text)) {
        return Err(secret.refuse(CredentialError::Misplaced));
    }
    if has_placeholder(text) {
        return Err(CredentialError::Unknown.into());
    }
    Ok(())
}

/// Whether `text` holds the marker followed by enough random characters to be
/// a placeholder.
fn has_placeholder(text: &[u8]) -> bool {
    let marker = PLACEHOLDER_MARKER.as_bytes();
    let mut rest = text;
    while let Some(at) = rest.windows(marker.len()).position(|w| w == marker) {
        rest = rest.get(at + marker.len()..).unwrap_or_default();
        let random = rest
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric())
            .count();
        if random >= MIN_RANDOM {
            return true;
        }
    }
    false
}

fn note(used: &mut Vec<Arc<str>>, placeholder: &Arc<str>) {
    if !used.contains(placeholder) {
        used.push(Arc::clone(placeholder));
    }
}

fn join(pieces: &[Zeroizing<String>]) -> String {
    let mut out = String::new();
    for (i, piece) in pieces.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        out.push_str(piece);
    }
    out
}

/// Whether `path` has a form a server might resolve differently than a prefix
/// match reads it.
fn is_ambiguous(path: &str) -> bool {
    path.contains('%')
        || path.contains(';')
        || path.contains('\\')
        || path.contains("//")
        || path
            .split('/')
            .any(|segment| segment == "." || segment == "..")
}

fn is_under(path: &str, prefix: &str) -> bool {
    prefix == "/"
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| prefix.ends_with('/') || rest.starts_with('/'))
}

fn basic_credentials(name: &HeaderName, value: &HeaderValue) -> Option<Zeroizing<Vec<u8>>> {
    if name != header::AUTHORIZATION && name != header::PROXY_AUTHORIZATION {
        return None;
    }
    let bytes = value.as_bytes();
    let (scheme, encoded) = bytes.split_at_checked(6)?;
    if !scheme.eq_ignore_ascii_case(b"basic ") {
        return None;
    }
    STANDARD
        .decode(encoded.trim_ascii())
        .ok()
        .map(Zeroizing::new)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn replace(haystack: &[u8], needle: &[u8], with: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(haystack.len() + with.len());
    let mut rest = haystack;
    while let Some(at) = rest.windows(needle.len()).position(|w| w == needle) {
        let Some((before, after)) = rest.split_at_checked(at) else {
            break;
        };
        out.extend_from_slice(before);
        out.extend_from_slice(with);
        rest = after.get(needle.len()..).unwrap_or_default();
    }
    out.extend_from_slice(rest);
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const PLACEHOLDER: &str = "wash_ph_7Qm2xYk9Lr4Tz8Vb1Nc6Hd3F";
    const OTHER: &str = "wash_ph_Zx81Qa4Ws7Ed2Rf5Tg9Yh3Uj";

    pub(crate) struct Fixed(pub Option<&'static [u8]>);

    impl EgressCredentialSource for Fixed {
        fn current(&self) -> Result<Zeroizing<Vec<u8>>, CredentialError> {
            self.0
                .map(|v| Zeroizing::new(v.to_vec()))
                .ok_or(CredentialError::Unavailable)
        }
    }

    /// A secret for `api.example.com`, sent in `authorization`.
    pub(crate) fn api_secret(value: Option<&'static [u8]>) -> EgressSecret {
        EgressSecret::new(PLACEHOLDER, Arc::new(Fixed(value)))
            .unwrap()
            .for_destinations(["api.example.com".parse().unwrap()])
            .unwrap()
    }

    fn request(uri: &str, headers: &[(&'static str, String)]) -> hyper::Request<()> {
        request_with(Method::GET, uri, headers)
    }

    fn request_with(
        method: Method,
        uri: &str,
        headers: &[(&'static str, String)],
    ) -> hyper::Request<()> {
        let mut builder = hyper::Request::builder().method(method).uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, value);
        }
        builder.body(()).unwrap()
    }

    fn bearer() -> (&'static str, String) {
        ("authorization", format!("Bearer {PLACEHOLDER}"))
    }

    fn run(
        req: &mut hyper::Request<()>,
        secrets: &[EgressSecret],
    ) -> Result<usize, CredentialError> {
        substitute(req, secrets)
            .map(|u| u.len())
            .map_err(|r| r.error)
    }

    #[test]
    fn the_placeholder_becomes_the_secret_in_place() {
        let mut req = request(
            "https://api.example.com/v1",
            &[bearer(), ("x-trace", "keep".into())],
        );

        let used = substitute(&mut req, &[api_secret(Some(b"host-secret"))]).unwrap();

        assert_eq!(used, [Arc::<str>::from(PLACEHOLDER)]);
        let value = &req.headers()[header::AUTHORIZATION];
        assert_eq!(value, "Bearer host-secret");
        assert!(value.is_sensitive());
        assert_eq!(req.headers()["x-trace"], "keep");
        assert!(req.extensions().get::<CarriesSecret>().is_some());
    }

    #[test]
    fn basic_credentials_are_decoded_and_encoded_again() {
        let encoded = STANDARD.encode(format!("svc:{PLACEHOLDER}"));
        let mut req = request(
            "https://api.example.com/",
            &[("authorization", format!("Basic {encoded}"))],
        );

        assert_eq!(run(&mut req, &[api_secret(Some(b"pa55"))]), Ok(1));

        let expected = format!("Basic {}", STANDARD.encode("svc:pa55"));
        assert_eq!(req.headers()[header::AUTHORIZATION], expected.as_str());
    }

    #[test]
    fn a_request_without_a_placeholder_is_untouched() {
        let mut req = request(
            "https://api.example.com/",
            &[("authorization", "Bearer own".into())],
        );
        assert_eq!(run(&mut req, &[api_secret(Some(b"s"))]), Ok(0));
        assert_eq!(req.headers()[header::AUTHORIZATION], "Bearer own");
        assert!(req.extensions().get::<CarriesSecret>().is_none());
    }

    #[test]
    fn a_placeholder_out_of_place_refuses() {
        let cases = [
            (
                "other destination",
                "https://paste.example.net/".to_string(),
                vec![bearer()],
                CredentialError::Destination,
            ),
            (
                "plaintext",
                "http://api.example.com/".to_string(),
                vec![bearer()],
                CredentialError::NotTls,
            ),
            (
                "header the secret is not for",
                "https://api.example.com/".to_string(),
                vec![("x-echo", PLACEHOLDER.to_string())],
                CredentialError::Misplaced,
            ),
            (
                "undeclared query parameter",
                format!("https://api.example.com/?key={PLACEHOLDER}"),
                vec![],
                CredentialError::Misplaced,
            ),
            (
                "path",
                format!("https://api.example.com/{PLACEHOLDER}"),
                vec![],
                CredentialError::Misplaced,
            ),
            (
                "host name",
                format!("https://{}.example.net/", PLACEHOLDER.to_lowercase()),
                vec![],
                CredentialError::Unknown,
            ),
        ];
        for (case, uri, headers, expected) in cases {
            let mut req = request(&uri, &headers);
            assert_eq!(
                run(&mut req, &[api_secret(Some(b"s"))]),
                Err(expected),
                "{case}"
            );
        }
    }

    #[test]
    fn a_placeholder_the_caller_does_not_hold_refuses() {
        let other = ("authorization", format!("Bearer {OTHER}"));
        for secrets in [vec![], vec![api_secret(Some(b"s"))]] {
            let mut req = request("https://api.example.com/", std::slice::from_ref(&other));
            assert_eq!(run(&mut req, &secrets), Err(CredentialError::Unknown));
        }

        let mut req = request(
            "https://api.example.com/",
            &[("authorization", format!("Bearer {PLACEHOLDER} {OTHER}"))],
        );
        assert_eq!(
            run(&mut req, &[api_secret(Some(b"s"))]),
            Err(CredentialError::Unknown)
        );

        let mut req = request(
            "https://api.example.com/",
            &[("x-note", "wash_ph_short".into())],
        );
        assert_eq!(
            run(&mut req, &[]),
            Ok(0),
            "the marker alone is not a placeholder"
        );
    }

    #[test]
    fn a_declared_query_value_is_substituted() {
        let secret = api_secret(Some(b"k&y=1")).in_query(["key"]);
        let mut req = request(
            &format!("https://api.example.com/maps?q=a%20b&key={PLACEHOLDER}"),
            &[],
        );

        assert_eq!(run(&mut req, std::slice::from_ref(&secret)), Ok(1));
        assert_eq!(
            req.uri().to_string(),
            "https://api.example.com/maps?q=a%20b&key=k%26y%3D1"
        );

        let mut req = request(&format!("https://api.example.com/?key=x{PLACEHOLDER}"), &[]);
        assert_eq!(
            run(&mut req, &[secret]),
            Err(CredentialError::Misplaced),
            "only a whole value is substituted"
        );
    }

    #[test]
    fn routes_bound_where_the_secret_may_be_sent() {
        let secret = api_secret(Some(b"s")).for_routes([
            SecretRoute::new(Some(Method::GET), "/v1/repos").unwrap(),
            SecretRoute::new(None, "/v1/search/").unwrap(),
        ]);
        let cases = [
            (Method::GET, "/v1/repos", Ok(1)),
            (Method::GET, "/v1/repos/x/issues", Ok(1)),
            (Method::POST, "/v1/search/code", Ok(1)),
            (Method::DELETE, "/v1/repos/x", Err(CredentialError::Route)),
            (Method::GET, "/v1/reposx", Err(CredentialError::Route)),
            (
                Method::GET,
                "/v1/repos/../admin",
                Err(CredentialError::Route),
            ),
            (
                Method::GET,
                "/v1/repos/%2e%2e/admin",
                Err(CredentialError::Route),
            ),
            (Method::GET, "/v1/repos//x", Err(CredentialError::Route)),
            (
                Method::GET,
                "/v1/repos;x/../admin",
                Err(CredentialError::Route),
            ),
            (
                Method::GET,
                "/v1/repos/x;jsessionid=1",
                Err(CredentialError::Route),
            ),
        ];
        for (method, path, expected) in cases {
            let mut req = request_with(
                method.clone(),
                &format!("https://api.example.com{path}"),
                &[bearer()],
            );
            assert_eq!(
                run(&mut req, std::slice::from_ref(&secret)),
                expected,
                "{method} {path}"
            );
        }
        assert!(SecretRoute::new(None, "v1").is_err());
        assert!(SecretRoute::new(None, "/v1/../x").is_err());
    }

    #[test]
    fn a_header_carrying_a_placeholder_twice_refuses() {
        let mut req = request("https://api.example.com/", &[bearer(), bearer()]);
        assert_eq!(
            run(&mut req, &[api_secret(Some(b"s"))]),
            Err(CredentialError::Misplaced)
        );
    }

    #[test]
    fn a_host_spelled_differently_never_gets_the_secret_elsewhere() {
        let mut req = request("https://api.example.com./", &[bearer()]);
        assert_eq!(
            run(&mut req, &[api_secret(Some(b"s"))]),
            Err(CredentialError::Destination),
            "a trailing dot is refused, not normalized"
        );
        let mut req = request("https://API.example.com/", &[bearer()]);
        let outcome = run(&mut req, &[api_secret(Some(b"s"))]);
        assert!(
            matches!(outcome, Ok(1) | Err(CredentialError::Destination)),
            "case is the same host or refused: {outcome:?}"
        );
    }

    #[test]
    fn an_unavailable_source_refuses() {
        let mut req = request("https://api.example.com/", &[bearer()]);
        let refused = substitute(&mut req, &[api_secret(None)]).unwrap_err();
        assert_eq!(refused.error, CredentialError::Unavailable);
        assert_eq!(refused.placeholder.as_deref(), Some(PLACEHOLDER));
    }

    #[test]
    fn a_lease_stops_serving_when_it_ends_or_is_revoked() {
        let source = LeasedSource::default();
        assert_eq!(source.current(), Err(CredentialError::Unavailable));

        source.renew(Zeroizing::new(b"v1".to_vec()), Duration::from_secs(60));
        assert_eq!(source.current().unwrap().as_slice(), b"v1");
        source.revoke();
        assert_eq!(source.current(), Err(CredentialError::Unavailable));

        source.renew(Zeroizing::new(b"v2".to_vec()), Duration::ZERO);
        assert_eq!(source.current(), Err(CredentialError::Unavailable));
    }

    #[test]
    fn a_value_that_is_not_a_header_refuses() {
        let mut req = request("https://api.example.com/", &[bearer()]);
        assert_eq!(
            run(&mut req, &[api_secret(Some(b"bad\r\nx-injected: 1"))]),
            Err(CredentialError::InvalidValue)
        );
    }

    #[test]
    fn placeholders_are_shaped_and_headers_unreserved() {
        let source = || Arc::new(Fixed(Some(b"s"))) as Arc<dyn EgressCredentialSource>;
        assert!(EgressSecret::new("short", source()).is_err());
        assert!(EgressSecret::new("x".repeat(40), source()).is_err());
        assert!(EgressSecret::new(format!("{PLACEHOLDER}=="), source()).is_err());
        assert!(EgressSecret::new(format!("sk-ant-{PLACEHOLDER}"), source()).is_ok());
        for name in RESERVED {
            assert!(api_secret(Some(b"s")).in_headers([name]).is_err());
        }

        let minted = mint_placeholder("ghp_", 40).unwrap();
        assert_eq!(minted.len(), 40);
        assert!(minted.starts_with("ghp_wash_ph_"));
        assert!(EgressSecret::new(minted.as_str(), source()).is_ok());
        assert_ne!(minted, mint_placeholder("ghp_", 40).unwrap());
        assert!(mint_placeholder("", 0).unwrap().len() >= PLACEHOLDER_MARKER.len() + MIN_RANDOM);
        assert!(mint_placeholder("a wash_ph_", 40).is_err());
    }

    #[test]
    fn destinations_are_exact_https_hosts() {
        let secret = || EgressSecret::new(PLACEHOLDER, Arc::new(Fixed(Some(b"s")))).unwrap();
        for refused in [
            "*",
            "*.example.com",
            "https://*.example.com",
            "http://api.example.com",
        ] {
            assert!(
                secret()
                    .for_destinations([refused.parse().unwrap()])
                    .is_err(),
                "{refused}"
            );
        }
        assert!(secret().for_destinations([]).is_err());
        for allowed in ["api.example.com", "https://api.example.com:8443"] {
            assert!(
                secret()
                    .for_destinations([allowed.parse().unwrap()])
                    .is_ok(),
                "{allowed}"
            );
        }
    }

    #[test]
    fn debug_never_shows_the_value() {
        let shown = format!("{:?}", api_secret(Some(b"host-secret")));
        assert!(!shown.contains("host-secret"), "{shown}");
    }
}
