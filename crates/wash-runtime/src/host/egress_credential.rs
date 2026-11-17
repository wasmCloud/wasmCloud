//! Secrets the host writes into a workload's outgoing HTTP requests.
//!
//! The guest holds a placeholder, never the secret. It receives the
//! placeholder where it would have received the secret (an environment
//! variable or config value) and sends it with an unmodified HTTP client. After
//! the request clears `allowed_hosts`, the ingress swaps each placeholder for
//! its secret, but only in the one place its [`SecretForm`] names, for the
//! destinations, and on the routes that secret names. A placeholder anywhere
//! else refuses the request, and so does any placeholder the caller holds no
//! secret for.
//!
//! The request must not set `Host` apart from its authority, which `wasi:http`
//! already forbids, nor any header that asks a server to treat it as another
//! method, path or host (see [`OVERRIDE_HEADERS`]). In the response head the
//! host swaps the secret back for the placeholder in headers and trailers, so
//! an upstream that echoes it (a redirect's `Location`, say) does not hand it
//! to the guest. Response bodies are not scanned, and neither are request
//! bodies, so a server that reads a method override from the body (a `_method`
//! form field) is not covered.

use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hyper::Method;
use hyper::header::{self, HeaderMap, HeaderName, HeaderValue};
use hyper::http::uri::Scheme;
use zeroize::Zeroizing;

/// Every placeholder ends in this marker and at least [`MIN_RANDOM`] random
/// alphanumerics, so the host can recognise one it holds no secret for.
pub const PLACEHOLDER_MARKER: &str = "wash_ph_";

/// Random characters after the marker: about 119 bits, so no two placeholders
/// and no real value ever collide.
pub const MIN_RANDOM: usize = 20;

/// The shortest secret the host substitutes. A shorter one is refused when it
/// is used: swapping it back out of a response would let the guest probe for
/// it a character at a time.
pub const MIN_SECRET_LEN: usize = 16;

/// The longest a [`LeasedSource`] lease runs, whatever it is renewed for.
pub const MAX_LEASE: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// Headers a request carrying a secret may not set, because servers and
/// proxies commonly honor them as another method, path, host or scheme.
pub const OVERRIDE_HEADERS: &[&str] = &[
    "x-http-method-override",
    "x-http-method",
    "x-method-override",
    "x-original-method",
    "x-original-url",
    "x-original-uri",
    "x-original-host",
    "x-rewrite-url",
    "x-envoy-original-path",
    "x-forwarded-host",
    "x-forwarded-prefix",
    "x-forwarded-path",
    "x-forwarded-uri",
    "x-forwarded-server",
    "x-forwarded-proto",
    "x-forwarded-port",
    "x-forwarded-scheme",
    "x-host",
    "forwarded",
];

/// A query parameter frameworks honor as the method, refused like
/// [`OVERRIDE_HEADERS`].
const METHOD_OVERRIDE_PARAM: &str = "_method";

/// The guest that issued an outgoing request, as the host attests it.
#[derive(Clone, Copy, Debug)]
pub struct EgressCaller<'a> {
    pub workload_id: &'a str,
    /// The workload's namespace, for matching it to grants. Empty when the
    /// host was not told it, as for a host component plugin.
    pub namespace: &'a str,
    /// The workload's name across replicas, so grants can target the
    /// deployment: its deployment, daemonset or replicaset annotation, else
    /// its `app.kubernetes.io/name` annotation, else its own name. Empty when
    /// unknown.
    ///
    /// Both names come from the start request, so they are only as
    /// trustworthy as its sender: any workload started in a namespace can
    /// carry another's application-name annotation and claim its name. Bind
    /// authority to something the sender cannot choose where that matters.
    pub name: &'a str,
    /// `None` when no single component can be attested: the request reached
    /// the host without a component behind it, or a concurrent cross-component
    /// call was in flight on the store, so the active component may not be
    /// the caller. Scope a secret to the workload to keep it usable then.
    pub component_id: Option<&'a str>,
}

/// Why a placeholder was not substituted. Every variant refuses the request.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CredentialError {
    Unavailable,
    NotTls,
    /// The destination host, port or `Host` header is not one the secret is
    /// for.
    Destination,
    /// The method or path is not one the secret is for.
    Route,
    /// The request sets a header that overrides its method, path or host.
    Overridden,
    /// The placeholder is somewhere its secret's form does not put it.
    Misplaced,
    /// A placeholder the caller holds no secret for.
    Unknown,
    /// Shorter than [`MIN_SECRET_LEN`], or not a valid value where it is
    /// placed.
    InvalidValue,
    /// The router could not say which secrets the caller holds.
    Undecided,
    /// The host's outgoing handler has not declared it can carry secrets.
    Unsupported,
}

impl std::fmt::Display for CredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "secret is not available",
            Self::NotTls => "secret refused for a request that is not https",
            Self::Destination => "secret is not for this destination",
            Self::Route => "secret is not for this method or path",
            Self::Overridden => "request sets a header that overrides its method, path or host",
            Self::Misplaced => "placeholder is somewhere the host does not substitute",
            Self::Unknown => "placeholder is not one this workload holds",
            Self::InvalidValue => "secret is too short or not a valid value where it is placed",
            Self::Undecided => "no decision on which secrets this caller holds",
            Self::Unsupported => "the host's egress transport does not carry secrets",
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
    /// `valid_for` is capped at [`MAX_LEASE`].
    pub fn renew(&self, value: Zeroizing<Vec<u8>>, valid_for: Duration) {
        let now = Instant::now();
        let until = now.checked_add(valid_for.min(MAX_LEASE)).unwrap_or(now);
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

/// The one place in a request a secret's placeholder may stand.
///
/// In every form the placeholder is the whole credential, never part of one,
/// so the host never splices a secret into text the guest chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretForm {
    /// `authorization: Bearer <placeholder>`.
    Bearer,
    /// `authorization: Basic`, with the placeholder as the whole password.
    BasicPassword,
    /// The whole value of this header.
    Header(HeaderName),
    /// The whole value of this query parameter.
    Query(String),
}

/// An exact https host and port a secret may be sent to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretDestination {
    host: Arc<str>,
    port: u16,
}

impl SecretDestination {
    /// `host` is a DNS name or IP literal (IPv6 in brackets), with no wildcard
    /// and no trailing dot. A wildcard would let the guest send the secret to
    /// any name under it, including one it controls.
    pub fn new(host: &str, port: u16) -> anyhow::Result<Self> {
        let named = !host.is_empty()
            && !host.starts_with(['.', '-'])
            && !host.ends_with(['.', '-'])
            && !host.contains("..")
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
        let ipv6 = host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .is_some_and(|h| h.parse::<std::net::Ipv6Addr>().is_ok());
        anyhow::ensure!(
            named || ipv6,
            "a secret's destination is an exact host name or IP address, not `{host}`"
        );
        anyhow::ensure!(port != 0, "a secret's destination needs a port");
        Ok(Self {
            host: host.to_ascii_lowercase().into(),
            port,
        })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub(crate) fn matches(&self, uri: &hyper::Uri) -> bool {
        uri.host()
            .is_some_and(|h| h.eq_ignore_ascii_case(&self.host))
            && uri.port_u16().unwrap_or(443) == self.port
    }
}

/// `host`, `host:port` or `https://host[:port]`; the port defaults to 443.
impl FromStr for SecretDestination {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        let rest = match s.split_once("://") {
            Some(("https", rest)) => rest,
            Some(_) => anyhow::bail!("a secret's destination must be https"),
            None => s,
        };
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        let (host, port) = match rest.rsplit_once(':') {
            Some((host, port))
                if !host.ends_with(':') && (!host.starts_with('[') || host.ends_with(']')) =>
            {
                (
                    host,
                    port.parse()
                        .map_err(|_| anyhow::anyhow!("`{port}` is not a port"))?,
                )
            }
            _ => (rest, 443),
        };
        Self::new(host, port)
    }
}

impl std::fmt::Display for SecretDestination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
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
    form: SecretForm,
    destinations: Arc<[SecretDestination]>,
    routes: Arc<[SecretRoute]>,
    refused_headers: Arc<[HeaderName]>,
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
        // Rejection sampling keeps every character equally likely.
        let chars = random
            .iter()
            .filter(|byte| **byte < 248)
            .filter_map(|byte| ALPHANUMERIC.get(usize::from(byte % 62)))
            .take(target - placeholder.len());
        placeholder.extend(chars.map(|c| char::from(*c)));
    }
    Ok(placeholder)
}

impl EgressSecret {
    /// Sent for no destination until [`Self::for_destinations`].
    ///
    /// The placeholder is `[prefix]wash_ph_` and at least [`MIN_RANDOM`]
    /// alphanumerics (see [`mint_placeholder`]).
    pub fn new(
        placeholder: impl Into<Arc<str>>,
        source: Arc<dyn EgressCredentialSource>,
        form: SecretForm,
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
        match &form {
            SecretForm::Header(name) => {
                anyhow::ensure!(
                    !RESERVED.contains(name) && !is_override(name, &[]),
                    "`{name}` cannot carry a secret"
                );
            }
            SecretForm::Query(name) => {
                anyhow::ensure!(!name.is_empty(), "a query parameter needs a name");
            }
            SecretForm::Bearer | SecretForm::BasicPassword => {}
        }
        Ok(Self {
            placeholder,
            form,
            destinations: Arc::from([]),
            routes: Arc::from([]),
            refused_headers: Arc::from([]),
            source,
        })
    }

    pub fn for_destinations(
        mut self,
        destinations: impl IntoIterator<Item = SecretDestination>,
    ) -> anyhow::Result<Self> {
        let destinations: Arc<[SecretDestination]> = destinations.into_iter().collect();
        anyhow::ensure!(!destinations.is_empty(), "a secret needs a destination");
        self.destinations = destinations;
        Ok(self)
    }

    /// Methods and paths the secret may be sent on. An empty list is refused,
    /// since it reads as "none"; use [`Self::any_route`] to allow every route.
    pub fn for_routes(
        mut self,
        routes: impl IntoIterator<Item = SecretRoute>,
    ) -> anyhow::Result<Self> {
        let routes: Arc<[SecretRoute]> = routes.into_iter().collect();
        anyhow::ensure!(
            !routes.is_empty(),
            "a secret needs a route, or `any_route` to allow every one"
        );
        self.routes = routes;
        Ok(self)
    }

    /// Send the secret on any method and path of its destinations, which is
    /// also the default.
    pub fn any_route(mut self) -> Self {
        self.routes = Arc::from([]);
        self
    }

    /// Headers refused on a request carrying this secret, beyond
    /// [`OVERRIDE_HEADERS`], which are always refused.
    pub fn refusing_headers(mut self, headers: impl IntoIterator<Item = HeaderName>) -> Self {
        self.refused_headers = headers.into_iter().collect();
        self
    }

    pub fn placeholder(&self) -> &str {
        &self.placeholder
    }

    pub fn form(&self) -> &SecretForm {
        &self.form
    }

    pub fn destinations(&self) -> &[SecretDestination] {
        &self.destinations
    }

    fn found_in(&self, haystack: &[u8]) -> bool {
        contains(haystack, self.placeholder.as_bytes())
    }

    fn check<B>(&self, request: &hyper::Request<B>) -> Result<(), CredentialError> {
        let uri = request.uri();
        if uri.scheme() != Some(&Scheme::HTTPS) {
            return Err(CredentialError::NotTls);
        }
        if !self.destinations.iter().any(|d| d.matches(uri)) {
            return Err(CredentialError::Destination);
        }
        let authority = uri.authority().map_or("", |a| a.as_str());
        let host_header = request.headers().get_all(header::HOST);
        if host_header
            .iter()
            .any(|h| !h.as_bytes().eq_ignore_ascii_case(authority.as_bytes()))
        {
            return Err(CredentialError::Destination);
        }
        let method_param = uri.query().is_some_and(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .any(|(name, _)| name.eq_ignore_ascii_case(METHOD_OVERRIDE_PARAM))
        });
        if method_param
            || request
                .headers()
                .keys()
                .any(|name| is_override(name, &self.refused_headers))
        {
            return Err(CredentialError::Overridden);
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

    /// The secret's value, refused if it is too short to swap back safely.
    fn value(&self) -> Result<Zeroizing<Vec<u8>>, Refused> {
        let value = self.source.current().map_err(|e| self.refuse(e))?;
        if value.len() < MIN_SECRET_LEN {
            return Err(self.refuse(CredentialError::InvalidValue));
        }
        Ok(value)
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
            .field("form", &self.form)
            .field("destinations", &self.destinations)
            .field("routes", &self.routes)
            .finish_non_exhaustive()
    }
}

fn is_override(name: &HeaderName, extra: &[HeaderName]) -> bool {
    OVERRIDE_HEADERS.contains(&name.as_str()) || extra.contains(name)
}

/// Set on a request once the host has written a secret into it.
///
/// An [`crate::host::http::OutgoingHandler`] must send such a request only
/// over TLS verified against the host's own trust roots, ignoring any
/// per-workload TLS configuration.
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

/// What a substitution wrote, kept until the response head is back.
#[derive(Default)]
pub(crate) struct Substituted {
    pub used: Vec<Arc<str>>,
    /// Each spelling of a secret that went out, and the placeholder spelling
    /// it replaced, longest first.
    sent: Vec<(Zeroizing<Vec<u8>>, Vec<u8>)>,
    /// Each secret's own value.
    values: Vec<Zeroizing<Vec<u8>>>,
}

impl std::fmt::Debug for Substituted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Substituted")
            .field("used", &self.used)
            .finish_non_exhaustive()
    }
}

impl Substituted {
    fn note_value(&mut self, secret: &EgressSecret, value: &[u8]) {
        if !self.values.iter().any(|v| **v == *value) {
            self.values.push(Zeroizing::new(value.to_vec()));
        }
        self.note(secret, value, secret.placeholder.as_bytes());
    }

    fn note(&mut self, secret: &EgressSecret, sent: &[u8], placeholder: &[u8]) {
        if !self.used.contains(&secret.placeholder) {
            self.used.push(Arc::clone(&secret.placeholder));
        }
        if !sent.is_empty() && !self.sent.iter().any(|(s, _)| **s == *sent) {
            self.sent
                .push((Zeroizing::new(sent.to_vec()), placeholder.to_vec()));
        }
    }

    /// Swap every secret in `headers` back for its placeholder.
    ///
    /// A value that still holds a secret in some other spelling after the
    /// swap (see [`try_each_spelling`]), or that no longer parses as a header,
    /// is dropped. Other encodings, such as JSON's `\/` or base64 of anything
    /// but the Basic credential the host sent, are not recognised, and bodies
    /// are never scanned.
    pub(crate) fn revert(&self, headers: &mut HeaderMap) {
        if self.sent.is_empty() {
            return;
        }
        let mut dropped = Vec::new();
        for (name, value) in headers.iter_mut() {
            let mut text = Zeroizing::new(value.as_bytes().to_vec());
            let mut changed = false;
            for (sent, placeholder) in &self.sent {
                if contains(&text, sent) {
                    text = Zeroizing::new(replace(&text, sent, placeholder));
                    changed = true;
                }
            }
            if any_spelling(&text, |t| self.values.iter().any(|v| contains(t, v))) {
                dropped.push(name.clone());
            } else if changed {
                match HeaderValue::from_bytes(&text) {
                    Ok(reverted) => *value = reverted,
                    Err(_) => dropped.push(name.clone()),
                }
            }
        }
        for name in dropped {
            headers.remove(name);
        }
    }
}

/// Swap every placeholder in `request` for its secret.
pub(crate) fn substitute<B>(
    request: &mut hyper::Request<B>,
    secrets: &[EgressSecret],
) -> Result<Substituted, Refused> {
    let in_uri = placeholder_in_uri(request.uri());
    let mut done = Substituted::default();
    if !in_uri && !placeholder_in_headers(request.headers()) {
        return Ok(done);
    }

    let uri = if in_uri {
        substitute_uri(request, secrets, &mut done)?
    } else {
        None
    };
    let rewrites = substitute_headers(request, secrets, &mut done)?;

    if let Some(uri) = uri {
        *request.uri_mut() = uri;
    }
    let mut rewrites = rewrites.into_iter().peekable();
    for (index, (_, value)) in request.headers_mut().iter_mut().enumerate() {
        if let Some((_, rewritten)) = rewrites.next_if(|(i, _)| *i == index) {
            *value = rewritten;
        }
    }
    if !done.used.is_empty() {
        request.extensions_mut().insert(CarriesSecret);
    }
    done.sent
        .sort_by_key(|(sent, _)| std::cmp::Reverse(sent.len()));
    Ok(done)
}

/// Whether `request` holds anything shaped like a placeholder in its URI or
/// headers, in any spelling the host decodes. Outbound, the router is asked
/// for secrets only when this holds.
///
/// Inbound, ingress refuses such a request. A placeholder is worthless outside
/// its workload, but a workload that copies caller input into its own outgoing
/// requests would turn one sent in from outside into an authenticated call.
/// That check is best-effort: a body, or an encoding the host does not decode,
/// still carries one in.
pub(crate) fn carries_placeholder<B>(request: &hyper::Request<B>) -> bool {
    placeholder_in_uri(request.uri()) || placeholder_in_headers(request.headers())
}

fn placeholder_in_uri(uri: &hyper::Uri) -> bool {
    [
        uri.authority().map_or("", |a| a.as_str()),
        uri.path_and_query().map_or("", |p| p.as_str()),
    ]
    .iter()
    .any(|part| any_spelling(part.as_bytes(), has_placeholder))
}

fn placeholder_in_headers(headers: &HeaderMap) -> bool {
    headers
        .iter()
        .any(|(name, value)| placeholder_in_header(name, value))
}

fn placeholder_in_header(name: &HeaderName, value: &HeaderValue) -> bool {
    any_spelling(value.as_bytes(), has_placeholder)
        || basic_credentials(name, value).is_some_and(|b| any_spelling(&b, has_placeholder))
}

/// The URI with the query value substituted, or `None` if there was nothing
/// to substitute.
fn substitute_uri<B>(
    request: &hyper::Request<B>,
    secrets: &[EgressSecret],
    done: &mut Substituted,
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
        if !any_spelling(piece.as_bytes(), has_placeholder) {
            pieces.push(Zeroizing::new(piece.to_string()));
            continue;
        }
        let (raw_name, raw_value) = piece.split_once('=').unwrap_or((piece, ""));
        let Some((name, value)) = url::form_urlencoded::parse(piece.as_bytes()).next() else {
            pieces.push(Zeroizing::new(piece.to_string()));
            continue;
        };
        refuse_any_placeholder(name.as_bytes(), secrets)?;
        let placed = secrets.iter().find(|s| {
            *s.placeholder == *value && matches!(&s.form, SecretForm::Query(q) if **q == *name)
        });
        let Some(secret) = placed else {
            refuse_any_placeholder(value.as_bytes(), secrets)?;
            pieces.push(Zeroizing::new(piece.to_string()));
            continue;
        };
        secret.check(request).map_err(|e| secret.refuse(e))?;
        let value = secret.value()?;
        let value = std::str::from_utf8(&value)
            .map_err(|_| secret.refuse(CredentialError::InvalidValue))?;
        let encoded = Zeroizing::new(
            url::form_urlencoded::byte_serialize(value.as_bytes()).collect::<String>(),
        );
        done.note_value(secret, value.as_bytes());
        done.note(secret, encoded.as_bytes(), raw_value.as_bytes());
        pieces.push(Zeroizing::new(format!("{raw_name}={}", encoded.as_str())));
        changed = true;
    }
    if !changed {
        return Ok(None);
    }
    let path_and_query = Zeroizing::new(format!("{}?{}", uri.path(), join(&pieces).as_str()));
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

/// How a header holds a placeholder when it holds one in a substitutable
/// place.
enum Placed {
    Bearer,
    Basic { user: Zeroizing<Vec<u8>> },
    Whole,
}

/// The secret whose placeholder is the whole credential in this header, and
/// how. `None` if no secret's form puts its placeholder here.
fn placed_in<'s>(
    name: &HeaderName,
    value: &HeaderValue,
    secrets: &'s [EgressSecret],
) -> Option<(&'s EgressSecret, Placed)> {
    let bytes = value.as_bytes();
    let find = |token: &[u8], form: &dyn Fn(&SecretForm) -> bool| {
        secrets
            .iter()
            .find(|s| s.placeholder.as_bytes() == token && form(&s.form))
    };
    if let Some(secret) = find(
        bytes,
        &|form| matches!(form, SecretForm::Header(h) if h == name),
    ) {
        return Some((secret, Placed::Whole));
    }
    if name != header::AUTHORIZATION {
        return None;
    }
    if let Some(token) = bearer_token(bytes) {
        return find(token, &|form| *form == SecretForm::Bearer).map(|s| (s, Placed::Bearer));
    }
    let decoded = basic_credentials(name, value)?;
    let colon = decoded.iter().position(|b| *b == b':')?;
    let (user, password) = decoded.split_at(colon);
    let password = password.get(1..).unwrap_or_default();
    let secret = find(password, &|form| *form == SecretForm::BasicPassword)?;
    Some((
        secret,
        Placed::Basic {
            user: Zeroizing::new(user.to_vec()),
        },
    ))
}

fn substitute_headers<B>(
    request: &hyper::Request<B>,
    secrets: &[EgressSecret],
    done: &mut Substituted,
) -> Result<Vec<(usize, HeaderValue)>, Refused> {
    let mut rewrites = Vec::new();
    let mut carried: Vec<&HeaderName> = Vec::new();
    for (index, (name, value)) in request.headers().iter().enumerate() {
        if !placeholder_in_header(name, value) {
            continue;
        }
        let Some((secret, placed)) = placed_in(name, value, secrets) else {
            refuse_any_placeholder(value.as_bytes(), secrets)?;
            if let Some(decoded) = &basic_credentials(name, value) {
                refuse_any_placeholder(decoded, secrets)?;
            }
            continue;
        };
        if carried.contains(&name) {
            return Err(secret.refuse(CredentialError::Misplaced));
        }
        carried.push(name);
        if let Placed::Basic { user } = &placed {
            refuse_any_placeholder(user, secrets)?;
        }
        secret.check(request).map_err(|e| secret.refuse(e))?;
        let current = secret.value()?;
        done.note_value(secret, &current);
        let bytes = match &placed {
            Placed::Whole => current,
            Placed::Bearer => {
                let mut out = Zeroizing::new(b"Bearer ".to_vec());
                out.extend_from_slice(&current);
                out
            }
            Placed::Basic { user } => {
                let mut credential = Zeroizing::new(user.to_vec());
                credential.push(b':');
                credential.extend_from_slice(&current);
                let encoded = Zeroizing::new(STANDARD.encode(&*credential));
                let mut original = user.to_vec();
                original.push(b':');
                original.extend_from_slice(secret.placeholder.as_bytes());
                done.note(
                    secret,
                    encoded.as_bytes(),
                    STANDARD.encode(original).as_bytes(),
                );
                Zeroizing::new(format!("Basic {}", encoded.as_str()).into_bytes())
            }
        };
        let mut value = HeaderValue::from_bytes(&bytes)
            .map_err(|_| secret.refuse(CredentialError::InvalidValue))?;
        value.set_sensitive(true);
        rewrites.push((index, value));
    }
    Ok(rewrites)
}

/// The token of a `Bearer` credential.
fn bearer_token(value: &[u8]) -> Option<&[u8]> {
    let (scheme, token) = value.split_at_checked(7)?;
    scheme.eq_ignore_ascii_case(b"bearer ").then_some(token)
}

/// Refuse if `text` holds any placeholder in any spelling: misplaced if the
/// caller holds it, unknown if not.
fn refuse_any_placeholder(text: &[u8], secrets: &[EgressSecret]) -> Result<(), Refused> {
    try_each_spelling(text, |text| {
        if let Some(secret) = secrets.iter().find(|s| s.found_in(text)) {
            return Err(secret.refuse(CredentialError::Misplaced));
        }
        if has_placeholder(text) {
            return Err(CredentialError::Unknown.into());
        }
        Ok(())
    })
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

fn join(pieces: &[Zeroizing<String>]) -> Zeroizing<String> {
    let mut out = Zeroizing::new(String::new());
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

/// Decodes `Basic` payloads with or without padding, and with stray low bits,
/// so a placeholder cannot hide in a non-canonical encoding.
const LENIENT_BASE64: base64::engine::GeneralPurpose = base64::engine::GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    base64::engine::GeneralPurposeConfig::new()
        .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

/// How many times a value is percent-decoded when looking for a placeholder or
/// a secret, so one escaped two or three times over is still found.
const DECODE_ROUNDS: usize = 3;

/// Run `f` on `text` and on each reading a server might decode it to:
/// percent-decoded up to [`DECODE_ROUNDS`] times, with `+` read as a space and
/// as itself. Stops at the first error. Never used to rewrite a request.
fn try_each_spelling<E>(text: &[u8], mut f: impl FnMut(&[u8]) -> Result<(), E>) -> Result<(), E> {
    f(text)?;
    if !text.iter().any(|b| *b == b'%' || *b == b'+') {
        return Ok(());
    }
    let mut seen = vec![Zeroizing::new(text.to_vec())];
    let mut round = 0..seen.len();
    for _ in 0..DECODE_ROUNDS {
        let end = seen.len();
        for i in round {
            for plus_as_space in [true, false] {
                let Some(current) = seen.get(i) else {
                    continue;
                };
                let decoded = Zeroizing::new(percent_decode(current, plus_as_space).into_owned());
                if seen.iter().any(|s| **s == *decoded) {
                    continue;
                }
                f(&decoded)?;
                seen.push(decoded);
            }
        }
        round = end..seen.len();
    }
    Ok(())
}

fn any_spelling(text: &[u8], mut found: impl FnMut(&[u8]) -> bool) -> bool {
    try_each_spelling(text, |t| if found(t) { Err(()) } else { Ok(()) }).is_err()
}

/// `text` with `%XX` escapes decoded, and `+` read as a space if
/// `plus_as_space`.
fn percent_decode(text: &[u8], plus_as_space: bool) -> std::borrow::Cow<'_, [u8]> {
    if !text
        .iter()
        .any(|b| *b == b'%' || (plus_as_space && *b == b'+'))
    {
        return std::borrow::Cow::Borrowed(text);
    }
    let hex = |b: u8| {
        char::from(b)
            .to_digit(16)
            .and_then(|d| u8::try_from(d).ok())
    };
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while let Some(&b) = text.get(i) {
        let escaped = (b == b'%')
            .then(|| Some(hex(*text.get(i + 1)?)? << 4 | hex(*text.get(i + 2)?)?))
            .flatten();
        match (b, escaped) {
            (_, Some(decoded)) => {
                out.push(decoded);
                i += 3;
            }
            (b'+', None) if plus_as_space => {
                out.push(b' ');
                i += 1;
            }
            (b, None) => {
                out.push(b);
                i += 1;
            }
        }
    }
    std::borrow::Cow::Owned(out)
}

/// The scheme-stripped, decoded payload of a `Basic` credential.
fn basic_credentials(name: &HeaderName, value: &HeaderValue) -> Option<Zeroizing<Vec<u8>>> {
    if name != header::AUTHORIZATION && name != header::PROXY_AUTHORIZATION {
        return None;
    }
    let bytes = value.as_bytes();
    let (scheme, encoded) = bytes.split_at_checked(6)?;
    if !scheme.eq_ignore_ascii_case(b"basic ") {
        return None;
    }
    LENIENT_BASE64
        .decode(encoded.trim_ascii())
        .ok()
        .map(Zeroizing::new)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

fn replace(haystack: &[u8], needle: &[u8], with: &[u8]) -> Vec<u8> {
    if needle.is_empty() {
        return haystack.to_vec();
    }
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

    pub(crate) fn secret_in(form: SecretForm, value: Option<&'static [u8]>) -> EgressSecret {
        EgressSecret::new(PLACEHOLDER, Arc::new(Fixed(value)), form)
            .unwrap()
            .for_destinations(["api.example.com".parse().unwrap()])
            .unwrap()
    }

    /// A bearer secret for `api.example.com:443`.
    pub(crate) fn api_secret(value: Option<&'static [u8]>) -> EgressSecret {
        secret_in(SecretForm::Bearer, value)
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
            .map(|u| u.used.len())
            .map_err(|r| r.error)
    }

    #[test]
    fn the_placeholder_becomes_the_secret_in_place() {
        let mut req = request(
            "https://api.example.com/v1",
            &[bearer(), ("x-trace", "keep".into())],
        );

        let done = substitute(&mut req, &[api_secret(Some(b"host-secret-0123456789"))]).unwrap();

        assert_eq!(done.used, [Arc::<str>::from(PLACEHOLDER)]);
        let value = &req.headers()[header::AUTHORIZATION];
        assert_eq!(value, "Bearer host-secret-0123456789");
        assert!(value.is_sensitive());
        assert_eq!(req.headers()["x-trace"], "keep");
        assert!(req.extensions().get::<CarriesSecret>().is_some());
    }

    #[test]
    fn each_form_substitutes_only_its_whole_credential() {
        let basic = |user: &str, password: &str| {
            (
                "authorization",
                format!("Basic {}", STANDARD.encode(format!("{user}:{password}"))),
            )
        };
        let key = HeaderName::from_static("x-api-key");
        type Case = (
            &'static str,
            SecretForm,
            String,
            Vec<(&'static str, String)>,
            Result<usize, CredentialError>,
        );
        let cases: Vec<Case> = vec![
            (
                "bearer",
                SecretForm::Bearer,
                "/".into(),
                vec![bearer()],
                Ok(1),
            ),
            (
                "bearer, scheme in any case",
                SecretForm::Bearer,
                "/".into(),
                vec![("authorization", format!("bEaReR {PLACEHOLDER}"))],
                Ok(1),
            ),
            (
                "bearer inside other text",
                SecretForm::Bearer,
                "/".into(),
                vec![("authorization", format!("Bearer x{PLACEHOLDER}"))],
                Err(CredentialError::Misplaced),
            ),
            (
                "bearer secret sent as basic",
                SecretForm::Bearer,
                "/".into(),
                vec![basic("svc", PLACEHOLDER)],
                Err(CredentialError::Misplaced),
            ),
            (
                "basic password",
                SecretForm::BasicPassword,
                "/".into(),
                vec![basic("svc", PLACEHOLDER)],
                Ok(1),
            ),
            (
                "basic, placeholder as user",
                SecretForm::BasicPassword,
                "/".into(),
                vec![basic(PLACEHOLDER, "x")],
                Err(CredentialError::Misplaced),
            ),
            (
                "basic, part of the password",
                SecretForm::BasicPassword,
                "/".into(),
                vec![basic("svc", &format!("{PLACEHOLDER}x"))],
                Err(CredentialError::Misplaced),
            ),
            (
                "basic secret sent as bearer",
                SecretForm::BasicPassword,
                "/".into(),
                vec![bearer()],
                Err(CredentialError::Misplaced),
            ),
            (
                "header",
                SecretForm::Header(key.clone()),
                "/".into(),
                vec![("x-api-key", PLACEHOLDER.into())],
                Ok(1),
            ),
            (
                "header, another header",
                SecretForm::Header(key.clone()),
                "/".into(),
                vec![("x-echo", PLACEHOLDER.into())],
                Err(CredentialError::Misplaced),
            ),
            (
                "header, part of the value",
                SecretForm::Header(key.clone()),
                "/".into(),
                vec![("x-api-key", format!("key={PLACEHOLDER}"))],
                Err(CredentialError::Misplaced),
            ),
            (
                "header secret sent as bearer",
                SecretForm::Header(key.clone()),
                "/".into(),
                vec![bearer()],
                Err(CredentialError::Misplaced),
            ),
            (
                "query",
                SecretForm::Query("key".into()),
                format!("/?key={PLACEHOLDER}"),
                vec![],
                Ok(1),
            ),
            (
                "query, another parameter",
                SecretForm::Query("key".into()),
                format!("/?token={PLACEHOLDER}"),
                vec![],
                Err(CredentialError::Misplaced),
            ),
            (
                "query secret sent in a header",
                SecretForm::Query("key".into()),
                "/".into(),
                vec![("x-api-key", PLACEHOLDER.into())],
                Err(CredentialError::Misplaced),
            ),
            (
                "path",
                SecretForm::Bearer,
                format!("/{PLACEHOLDER}"),
                vec![],
                Err(CredentialError::Misplaced),
            ),
        ];
        for (case, form, path, headers, expected) in cases {
            let mut req = request(&format!("https://api.example.com{path}"), &headers);
            assert_eq!(
                run(
                    &mut req,
                    &[secret_in(form, Some(b"s3cret-0123456789abcdef"))]
                ),
                expected,
                "{case}"
            );
        }
    }

    #[test]
    fn basic_credentials_are_decoded_and_encoded_again() {
        let encoded = STANDARD.encode(format!("svc:{PLACEHOLDER}"));
        let mut req = request(
            "https://api.example.com/",
            &[("authorization", format!("Basic {encoded}"))],
        );

        let secret = secret_in(SecretForm::BasicPassword, Some(b"pa55-0123456789abcdef"));
        assert_eq!(run(&mut req, &[secret]), Ok(1));

        let expected = format!("Basic {}", STANDARD.encode("svc:pa55-0123456789abcdef"));
        assert_eq!(req.headers()[header::AUTHORIZATION], expected.as_str());
    }

    #[test]
    fn a_request_without_a_placeholder_is_untouched() {
        let mut req = request(
            "https://api.example.com/",
            &[("authorization", "Bearer own".into())],
        );
        assert_eq!(
            run(&mut req, &[api_secret(Some(b"s-0123456789abcdef"))]),
            Ok(0)
        );
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
                "other port",
                "https://api.example.com:8443/".to_string(),
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
                "Host apart from the authority",
                "https://api.example.com/".to_string(),
                vec![bearer(), ("host", "paste.example.net".into())],
                CredentialError::Destination,
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
                run(&mut req, &[api_secret(Some(b"s-0123456789abcdef"))]),
                Err(expected),
                "{case}"
            );
        }

        let mut req = request(
            "https://api.example.com/",
            &[bearer(), ("host", "API.example.com".into())],
        );
        assert_eq!(
            run(&mut req, &[api_secret(Some(b"s-0123456789abcdef"))]),
            Ok(1),
            "a Host equal to the authority is allowed"
        );
    }

    #[test]
    fn override_headers_refuse_a_request_carrying_a_secret() {
        for name in OVERRIDE_HEADERS {
            let mut req = request(
                "https://api.example.com/",
                &[bearer(), (name, "/admin".into())],
            );
            assert_eq!(
                run(&mut req, &[api_secret(Some(b"s-0123456789abcdef"))]),
                Err(CredentialError::Overridden),
                "{name}"
            );
        }

        let secret = api_secret(Some(b"s-0123456789abcdef"))
            .refusing_headers([HeaderName::from_static("x-tenant")]);
        let mut req = request(
            "https://api.example.com/",
            &[bearer(), ("x-tenant", "b".into())],
        );
        assert_eq!(run(&mut req, &[secret]), Err(CredentialError::Overridden));

        let mut req = request(
            "https://api.example.com/",
            &[("x-http-method-override", "DELETE".into())],
        );
        assert_eq!(
            run(&mut req, &[api_secret(Some(b"s-0123456789abcdef"))]),
            Ok(0),
            "a request with no secret is not this check's business"
        );
    }

    #[test]
    fn a_placeholder_the_caller_does_not_hold_refuses() {
        let other = ("authorization", format!("Bearer {OTHER}"));
        for secrets in [vec![], vec![api_secret(Some(b"s-0123456789abcdef"))]] {
            let mut req = request("https://api.example.com/", std::slice::from_ref(&other));
            assert_eq!(run(&mut req, &secrets), Err(CredentialError::Unknown));
        }

        let mut req = request(
            "https://api.example.com/",
            &[bearer(), ("x-note", OTHER.into())],
        );
        assert_eq!(
            run(&mut req, &[api_secret(Some(b"s-0123456789abcdef"))]),
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
    fn a_query_value_is_substituted_whole() {
        let secret = secret_in(
            SecretForm::Query("key".into()),
            Some(b"k&y=1-0123456789abcdef"),
        );
        let mut req = request(
            &format!("https://api.example.com/maps?q=a%20b&key={PLACEHOLDER}"),
            &[],
        );

        assert_eq!(run(&mut req, std::slice::from_ref(&secret)), Ok(1));
        assert_eq!(
            req.uri().to_string(),
            "https://api.example.com/maps?q=a%20b&key=k%26y%3D1-0123456789abcdef"
        );

        let mut req = request(&format!("https://api.example.com/?key=x{PLACEHOLDER}"), &[]);
        assert_eq!(
            run(&mut req, &[secret]),
            Err(CredentialError::Misplaced),
            "only a whole value is substituted"
        );
    }

    #[test]
    fn the_response_head_never_shows_the_secret() {
        let secret = secret_in(
            SecretForm::Query("key".into()),
            Some(b"k&y=1-0123456789abcdef"),
        );
        let mut req = request(
            &format!("https://api.example.com/maps?key={PLACEHOLDER}"),
            &[],
        );
        let done = substitute(&mut req, &[secret]).unwrap();

        let mut headers = HeaderMap::new();
        headers.insert(
            header::LOCATION,
            HeaderValue::from_static(
                "https://api.example.com/maps/?key=k%26y%3D1-0123456789abcdef",
            ),
        );
        headers.insert(
            "x-echo",
            HeaderValue::from_static("got k&y=1-0123456789abcdef"),
        );
        headers.insert("x-other", HeaderValue::from_static("untouched"));
        done.revert(&mut headers);

        assert_eq!(
            headers[header::LOCATION],
            format!("https://api.example.com/maps/?key={PLACEHOLDER}").as_str()
        );
        assert_eq!(headers["x-echo"], format!("got {PLACEHOLDER}").as_str());
        assert_eq!(headers["x-other"], "untouched");

        let basic = STANDARD.encode(format!("svc:{PLACEHOLDER}"));
        let mut req = request(
            "https://api.example.com/",
            &[("authorization", format!("Basic {basic}"))],
        );
        let done = substitute(
            &mut req,
            &[secret_in(
                SecretForm::BasicPassword,
                Some(b"pa55-0123456789abcdef"),
            )],
        )
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-echo-auth", req.headers()[header::AUTHORIZATION].clone());
        done.revert(&mut headers);
        assert_eq!(headers["x-echo-auth"], format!("Basic {basic}").as_str());
    }

    #[test]
    fn routes_bound_where_the_secret_may_be_sent() {
        let secret = api_secret(Some(b"s-0123456789abcdef"))
            .for_routes([
                SecretRoute::new(Some(Method::GET), "/v1/repos").unwrap(),
                SecretRoute::new(None, "/v1/search/").unwrap(),
            ])
            .unwrap();
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
        assert!(api_secret(None).for_routes([]).is_err());

        let anywhere = secret.any_route();
        let mut req = request_with(
            Method::DELETE,
            "https://api.example.com/v1/repos/x",
            &[bearer()],
        );
        assert_eq!(run(&mut req, &[anywhere]), Ok(1));
    }

    #[test]
    fn a_header_carrying_a_placeholder_twice_refuses() {
        let mut req = request("https://api.example.com/", &[bearer(), bearer()]);
        assert_eq!(
            run(&mut req, &[api_secret(Some(b"s-0123456789abcdef"))]),
            Err(CredentialError::Misplaced)
        );
    }

    #[test]
    fn a_host_spelled_differently_never_gets_the_secret_elsewhere() {
        let mut req = request("https://api.example.com./", &[bearer()]);
        assert_eq!(
            run(&mut req, &[api_secret(Some(b"s-0123456789abcdef"))]),
            Err(CredentialError::Destination),
            "a trailing dot is refused, not normalized"
        );
        let mut req = request("https://API.example.com/", &[bearer()]);
        assert_eq!(
            run(&mut req, &[api_secret(Some(b"s-0123456789abcdef"))]),
            Ok(1),
            "host names compare without case"
        );
        let mut req = request("https://api.example.com:443/", &[bearer()]);
        assert_eq!(
            run(&mut req, &[api_secret(Some(b"s-0123456789abcdef"))]),
            Ok(1)
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
        let source =
            || Arc::new(Fixed(Some(b"s-0123456789abcdef"))) as Arc<dyn EgressCredentialSource>;
        let new = |placeholder: String, form| EgressSecret::new(placeholder, source(), form);
        assert!(new("short".into(), SecretForm::Bearer).is_err());
        assert!(new("x".repeat(40), SecretForm::Bearer).is_err());
        assert!(new(format!("{PLACEHOLDER}=="), SecretForm::Bearer).is_err());
        assert!(new(format!("sk-ant-{PLACEHOLDER}"), SecretForm::Bearer).is_ok());
        for name in RESERVED {
            assert!(new(PLACEHOLDER.into(), SecretForm::Header(name)).is_err());
        }
        for name in OVERRIDE_HEADERS {
            let name = HeaderName::from_static(name);
            assert!(new(PLACEHOLDER.into(), SecretForm::Header(name)).is_err());
        }
        assert!(new(PLACEHOLDER.into(), SecretForm::Query(String::new())).is_err());

        let minted = mint_placeholder("ghp_", 40).unwrap();
        assert_eq!(minted.len(), 40);
        assert!(minted.starts_with("ghp_wash_ph_"));
        assert!(new(minted.clone(), SecretForm::Bearer).is_ok());
        assert_ne!(minted, mint_placeholder("ghp_", 40).unwrap());
        assert!(mint_placeholder("", 0).unwrap().len() >= PLACEHOLDER_MARKER.len() + MIN_RANDOM);
        assert!(mint_placeholder("a wash_ph_", 40).is_err());
    }

    #[test]
    fn destinations_are_exact_https_hosts_and_ports() {
        for refused in [
            "*",
            "*.example.com",
            "https://*.example.com",
            "http://api.example.com",
            "api.example.com.",
            "api..example.com",
            "user@api.example.com",
            "api.example.com/path",
            "api.example.com:0",
            "api.example.com:x",
            "",
        ] {
            assert!(refused.parse::<SecretDestination>().is_err(), "{refused}");
        }
        for (allowed, host, port) in [
            ("api.example.com", "api.example.com", 443),
            ("API.example.com:8443", "api.example.com", 8443),
            ("https://api.example.com:8443/", "api.example.com", 8443),
            ("10.0.0.1", "10.0.0.1", 443),
            ("[::1]:8443", "[::1]", 8443),
        ] {
            let parsed: SecretDestination = allowed.parse().unwrap();
            assert_eq!((parsed.host(), parsed.port()), (host, port), "{allowed}");
        }
        let secret = EgressSecret::new(
            PLACEHOLDER,
            Arc::new(Fixed(Some(b"s-0123456789abcdef"))),
            SecretForm::Bearer,
        )
        .unwrap();
        assert!(secret.for_destinations([]).is_err());
    }

    #[test]
    fn inbound_placeholders_are_recognised() {
        let basic = STANDARD.encode(format!("u:{PLACEHOLDER}"));
        for (uri, headers) in [
            (format!("https://h/?k={PLACEHOLDER}"), vec![]),
            (format!("https://h/{PLACEHOLDER}"), vec![]),
            (
                "https://h/".to_string(),
                vec![("x-k", PLACEHOLDER.to_string())],
            ),
            (
                "https://h/".to_string(),
                vec![("authorization", format!("Basic {basic}"))],
            ),
        ] {
            assert!(
                carries_placeholder(&request(&uri, &headers)),
                "{uri} {headers:?}"
            );
        }
        assert!(!carries_placeholder(&request(
            "https://h/?k=wash_ph_short",
            &[("x-k", "plain".into())]
        )));
    }

    #[test]
    fn a_secret_too_short_to_swap_back_is_refused() {
        let mut req = request("https://api.example.com/", &[bearer()]);
        assert_eq!(
            run(&mut req, &[api_secret(Some(b"0123456789abcde"))]),
            Err(CredentialError::InvalidValue)
        );
        let mut req = request("https://api.example.com/", &[bearer()]);
        assert_eq!(
            run(&mut req, &[api_secret(Some(b"0123456789abcdef"))]),
            Ok(1)
        );
    }

    #[test]
    fn a_method_override_in_the_query_refuses() {
        for query in ["_method=DELETE", "_METHOD=delete", "a=1&_method=PUT"] {
            let mut req = request(&format!("https://api.example.com/?{query}"), &[bearer()]);
            assert_eq!(
                run(&mut req, &[api_secret(Some(b"s-0123456789abcdef"))]),
                Err(CredentialError::Overridden),
                "{query}"
            );
        }
    }

    #[test]
    fn a_placeholder_spelled_another_way_is_still_found() {
        let encoded: String = PLACEHOLDER.bytes().map(|b| format!("%{b:02X}")).collect();
        let unpadded = STANDARD
            .encode(format!("{PLACEHOLDER}:x"))
            .trim_end_matches('=')
            .to_string();
        let cases = [
            (format!("https://api.example.com/{encoded}"), vec![]),
            (format!("https://api.example.com/?k={encoded}"), vec![]),
            (
                "https://api.example.com/".to_string(),
                vec![("x-note", encoded.clone())],
            ),
            (
                "https://api.example.com/".to_string(),
                vec![("authorization", format!("Basic {unpadded}"))],
            ),
        ];
        let twice: String = encoded.bytes().map(|b| format!("%{b:02X}")).collect();
        let cases = cases.into_iter().chain([
            (format!("https://api.example.com/?k={twice}"), vec![]),
            (
                "https://api.example.com/".to_string(),
                vec![("x-note", twice.replace("%25", "%2525"))],
            ),
        ]);
        for (uri, headers) in cases {
            let mut req = request(&uri, &headers);
            assert!(carries_placeholder(&req), "{uri} {headers:?}");
            assert_eq!(
                run(&mut req, &[api_secret(Some(b"s-0123456789abcdef"))]),
                Err(CredentialError::Misplaced),
                "{uri} {headers:?}"
            );
        }
    }

    #[test]
    fn a_secret_echoed_in_another_encoding_is_dropped() {
        let mut req = request("https://api.example.com/", &[bearer()]);
        let done = substitute(&mut req, &[api_secret(Some(b"s-0123456789abcdef"))]).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::LOCATION,
            HeaderValue::from_static("https://x/?t=s%2D0123456789abcdef"),
        );
        headers.insert("x-plus", HeaderValue::from_static("s-0123456789+abcdef"));
        headers.insert("x-other", HeaderValue::from_static("untouched"));
        done.revert(&mut headers);
        assert!(headers.get(header::LOCATION).is_none());
        assert_eq!(headers["x-plus"], "s-0123456789+abcdef", "not the secret");
        assert_eq!(headers["x-other"], "untouched");
    }

    #[test]
    fn a_bare_marker_is_not_a_placeholder() {
        let mut req = request(
            "https://api.example.com/?k=wash_ph_short",
            &[("x-k", "a wash_ph_ note".into())],
        );
        assert!(!carries_placeholder(&req));
        assert_eq!(
            run(&mut req, &[api_secret(Some(b"s-0123456789abcdef"))]),
            Ok(0)
        );
    }

    #[test]
    fn a_secret_echoed_in_any_reading_is_dropped() {
        const VALUE: &str = "ab+cd/ef=gh-0123456789";
        let escape = |s: &str| -> String { s.bytes().map(|b| format!("%{b:02X}")).collect() };
        let mut req = request("https://api.example.com/", &[bearer()]);
        let done = substitute(&mut req, &[api_secret(Some(VALUE.as_bytes()))]).unwrap();
        let echoes = [
            "ab+cd%2Fef%3Dgh-0123456789".to_string(),
            "ab%2Bcd%2Fef%3Dgh-0123456789".to_string(),
            escape(VALUE),
            escape(&escape(VALUE)),
            escape(&escape(&escape(VALUE))),
        ];
        let mut headers = HeaderMap::new();
        for (i, echo) in echoes.iter().enumerate() {
            headers.insert(
                HeaderName::try_from(format!("x-echo-{i}")).unwrap(),
                HeaderValue::try_from(echo.as_str()).unwrap(),
            );
        }
        headers.insert("x-raw", HeaderValue::from_static(VALUE));
        headers.insert(
            "x-space",
            HeaderValue::from_static("ab cd/ef=gh-0123456789"),
        );
        done.revert(&mut headers);
        for (i, echo) in echoes.iter().enumerate() {
            assert!(headers.get(format!("x-echo-{i}")).is_none(), "{echo}");
        }
        assert_eq!(headers["x-raw"], PLACEHOLDER);
        assert_eq!(
            headers["x-space"], "ab cd/ef=gh-0123456789",
            "not the secret"
        );

        let mut req = request(&format!("https://api.example.com/?key={PLACEHOLDER}"), &[]);
        let secret = secret_in(SecretForm::Query("key".into()), Some(VALUE.as_bytes()));
        let done = substitute(&mut req, &[secret]).unwrap();
        let sent = req.uri().query().unwrap().to_string();
        assert_eq!(sent, format!("key={}", "ab%2Bcd%2Fef%3Dgh-0123456789"));
        let mut headers = HeaderMap::new();
        headers.insert(
            header::LOCATION,
            HeaderValue::try_from(format!("https://api.example.com/next?{sent}")).unwrap(),
        );
        done.revert(&mut headers);
        assert_eq!(
            headers[header::LOCATION],
            format!("https://api.example.com/next?key={PLACEHOLDER}")
        );
    }

    #[test]
    fn a_lease_never_overflows() {
        let source = LeasedSource::default();
        source.renew(Zeroizing::new(b"v".to_vec()), Duration::MAX);
        assert_eq!(source.current().unwrap().as_slice(), b"v");
    }

    #[test]
    fn debug_never_shows_the_value() {
        let shown = format!("{:?}", api_secret(Some(b"host-secret-0123456789")));
        assert!(!shown.contains("host-secret"), "{shown}");
    }
}
