//! Access tokens obtained with a workload's JWT-SVID at an OAuth token
//! endpoint, so no client secret exists anywhere.
//!
//! Three grants: RFC 8693 token exchange with the SVID as `subject_token`,
//! RFC 7523 §2.1 with it as the authorization grant, and RFC 7523 §2.2 with it
//! as the client's own authentication. The issued token is checked before it
//! is served, and leased only until it expires, counted from just before the
//! request that obtained it. Dropping a [`TokenRefresh`] forgets the token
//! locally. The endpoint is not told (no RFC 7009 revocation), so the token
//! stays valid upstream until it expires.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use http_body_util::{BodyExt as _, Full, Limited};
use serde::Deserialize;
use tracing::warn;
use zeroize::Zeroizing;

use crate::host::dpop::{DPOP, DpopKey};
use crate::host::egress_credential::{CredentialError, EgressCredentialSource, LeasedSource};

const TOKEN_EXCHANGE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
const JWT_BEARER: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";
const CLIENT_ASSERTION_JWT: &str = "urn:ietf:params:oauth:client-assertion-type:jwt-bearer";
const TOKEN_TYPE_JWT: &str = "urn:ietf:params:oauth:token-type:jwt";
const TOKEN_TYPE_ACCESS: &str = "urn:ietf:params:oauth:token-type:access_token";

const MAX_RESPONSE: usize = 64 * 1024;
const RETRY: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
const MIN_SKEW: Duration = Duration::from_secs(30);
const CLOCK_SKEW: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const TIMEOUT: Duration = Duration::from_secs(30);

/// How the SVID is presented to the token endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Grant {
    /// RFC 8693, the SVID as `subject_token`.
    TokenExchange,
    /// RFC 7523 §2.1, the SVID as the authorization grant.
    JwtBearer,
    /// `client_credentials`, the SVID as the client assertion (RFC 7523 §2.2).
    ClientAssertion,
}

/// How a token is presented once issued.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenType {
    Bearer,
    Dpop,
}

/// One token endpoint, one grant, one set of requested parameters.
pub struct TokenExchange {
    endpoint: http::Uri,
    grant: Grant,
    subject: Arc<dyn EgressCredentialSource>,
    client_id: Option<String>,
    audiences: Vec<String>,
    resources: Vec<String>,
    scope: Option<String>,
    authorization_details: Option<String>,
    expected_audience: Option<String>,
    max_lifetime: Duration,
    default_lifetime: Duration,
    dpop: Option<Arc<DpopKey>>,
    tls: Arc<rustls::ClientConfig>,
    held: LeasedSource,
    token_type: Mutex<Option<TokenType>>,
    refreshing: AtomicBool,
}

/// A token as the endpoint issued it, after every check passed.
pub struct IssuedToken {
    pub access_token: Zeroizing<String>,
    pub token_type: TokenType,
    pub lifetime: Duration,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<serde_json::Value>,
    issued_token_type: Option<String>,
    scope: Option<String>,
    error: Option<String>,
}

impl Drop for TokenResponse {
    fn drop(&mut self) {
        use zeroize::Zeroize as _;
        self.access_token.zeroize();
    }
}

impl TokenExchange {
    /// `subject` serves the workload's JWT-SVID, as
    /// [`crate::host::workload_identity::WorkloadCredentials::jwt_source`] does.
    pub fn new(
        endpoint: &str,
        grant: Grant,
        subject: Arc<dyn EgressCredentialSource>,
        tls: Arc<rustls::ClientConfig>,
    ) -> anyhow::Result<Self> {
        let endpoint: http::Uri = endpoint.parse()?;
        anyhow::ensure!(
            endpoint.scheme() == Some(&http::uri::Scheme::HTTPS),
            "the token endpoint must be https"
        );
        Ok(Self {
            endpoint,
            grant,
            subject,
            client_id: None,
            audiences: Vec::new(),
            resources: Vec::new(),
            scope: None,
            authorization_details: None,
            expected_audience: None,
            max_lifetime: Duration::from_secs(24 * 3600),
            default_lifetime: Duration::from_secs(300),
            dpop: None,
            tls,
            held: LeasedSource::default(),
            token_type: Mutex::default(),
            refreshing: AtomicBool::new(false),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_test_endpoint(mut self, endpoint: http::Uri) -> Self {
        self.endpoint = endpoint;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_test_token(self, token: &[u8], token_type: TokenType) -> Self {
        self.held
            .renew(Zeroizing::new(token.to_vec()), Duration::from_secs(60));
        *self.token_type.lock().unwrap_or_else(|e| e.into_inner()) = Some(token_type);
        self
    }

    pub fn with_client_id(mut self, client_id: &str) -> Self {
        self.client_id = Some(client_id.to_string());
        self
    }

    /// RFC 8693 `audience`. Sent only with [`Grant::TokenExchange`].
    pub fn with_audience(mut self, audience: &str) -> Self {
        self.audiences.push(audience.to_string());
        self
    }

    /// RFC 8707 `resource`, an absolute URI without a fragment.
    pub fn with_resource(mut self, resource: &str) -> anyhow::Result<Self> {
        let uri: http::Uri = resource.parse()?;
        anyhow::ensure!(
            uri.scheme().is_some() && !resource.contains('#'),
            "a resource indicator is an absolute URI without a fragment"
        );
        self.resources.push(resource.to_string());
        Ok(self)
    }

    pub fn with_scope(mut self, scope: &str) -> Self {
        self.scope = Some(scope.to_string());
        self
    }

    /// RFC 9396 `authorization_details`: an array of objects, each with a
    /// string `type`.
    pub fn with_authorization_details(
        mut self,
        details: serde_json::Value,
    ) -> anyhow::Result<Self> {
        let entries = details
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or_else(|| anyhow::anyhow!("authorization_details is a non-empty array"))?;
        anyhow::ensure!(
            entries
                .iter()
                .all(|e| e.get("type").is_some_and(|t| t.is_string())),
            "every authorization_details entry has a string `type`"
        );
        self.authorization_details = Some(serde_json::to_string(&details)?);
        Ok(self)
    }

    /// Refuse a JWT access token whose `aud` does not include `audience`.
    pub fn expecting_audience(mut self, audience: &str) -> Self {
        self.expected_audience = Some(audience.to_string());
        self
    }

    /// The longest `expires_in` accepted, and the lease when none is given.
    pub fn with_lifetimes(mut self, max: Duration, default: Duration) -> Self {
        self.max_lifetime = max;
        self.default_lifetime = default.min(max);
        self
    }

    /// Ask for a token bound to `key` (RFC 9449). A Bearer token is then
    /// refused as a downgrade.
    pub fn with_dpop(mut self, key: Arc<DpopKey>) -> Self {
        self.dpop = Some(key);
        self
    }

    pub fn dpop_key(&self) -> Option<&Arc<DpopKey>> {
        self.dpop.as_ref()
    }

    /// The type of the token currently served.
    pub fn token_type(&self) -> Option<TokenType> {
        *self.token_type.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Exchange the current SVID for a token, without leasing it.
    pub async fn exchange(&self) -> anyhow::Result<IssuedToken> {
        let mut sent = Instant::now();
        let (mut status, headers, mut response) = self.attempt().await?;
        let retry = status == http::StatusCode::BAD_REQUEST
            && response.error.as_deref() == Some("use_dpop_nonce")
            && self
                .dpop
                .as_ref()
                .is_some_and(|key| key.observe(&self.endpoint, &headers));
        if retry {
            sent = Instant::now();
            (status, _, response) = self.attempt().await?;
        }
        let mut issued = self.check(status, response)?;
        issued.lifetime = issued.lifetime.saturating_sub(sent.elapsed());
        anyhow::ensure!(
            !issued.lifetime.is_zero(),
            "the access token expired before it arrived"
        );
        Ok(issued)
    }

    /// One post with the SVID read afresh, so a retry carries whatever the
    /// identity source serves now rather than a replay of the last assertion.
    async fn attempt(&self) -> anyhow::Result<(http::StatusCode, http::HeaderMap, TokenResponse)> {
        let svid = self
            .subject
            .current()
            .map_err(|e| anyhow::anyhow!("no workload identity to exchange: {e}"))?;
        let svid = std::str::from_utf8(&svid)?;
        let body = self.form(svid);
        tokio::time::timeout(TIMEOUT, self.post(&body))
            .await
            .map_err(|_| anyhow::anyhow!("the token endpoint did not answer in time"))?
    }

    fn form(&self, svid: &str) -> Zeroizing<String> {
        let mut pairs: Vec<(&str, &str)> = Vec::new();
        match self.grant {
            Grant::TokenExchange => {
                pairs.extend([
                    ("grant_type", TOKEN_EXCHANGE),
                    ("subject_token", svid),
                    ("subject_token_type", TOKEN_TYPE_JWT),
                    ("requested_token_type", TOKEN_TYPE_ACCESS),
                ]);
                pairs.extend(self.audiences.iter().map(|a| ("audience", a.as_str())));
            }
            Grant::JwtBearer => pairs.extend([("grant_type", JWT_BEARER), ("assertion", svid)]),
            Grant::ClientAssertion => pairs.extend([
                ("grant_type", "client_credentials"),
                ("client_assertion_type", CLIENT_ASSERTION_JWT),
                ("client_assertion", svid),
            ]),
        }
        if let Some(client_id) = &self.client_id {
            pairs.push(("client_id", client_id));
        }
        pairs.extend(self.resources.iter().map(|r| ("resource", r.as_str())));
        if let Some(scope) = &self.scope {
            pairs.push(("scope", scope));
        }
        if let Some(details) = &self.authorization_details {
            pairs.push(("authorization_details", details));
        }
        let mut form = Zeroizing::new(String::new());
        for (i, (k, v)) in pairs.iter().enumerate() {
            if i > 0 {
                form.push('&');
            }
            form_encode(&mut form, k);
            form.push('=');
            form_encode(&mut form, v);
        }
        form
    }

    async fn post(
        &self,
        body: &str,
    ) -> anyhow::Result<(http::StatusCode, http::HeaderMap, TokenResponse)> {
        let mut request = http::Request::post(self.endpoint.clone())
            .header(
                http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .header(http::header::ACCEPT, "application/json")
            .body(Full::new(bytes::Bytes::from_owner(Zeroizing::new(
                body.as_bytes().to_vec(),
            ))))?;
        if let Some(key) = &self.dpop {
            let proof = key.proof(&http::Method::POST, &self.endpoint, None)?;
            request.headers_mut().insert(DPOP, proof);
        }
        let client =
            hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
                .build(connector(&self.tls));
        let response = client.request(request).await?;
        let (parts, body) = response.into_parts();
        if let Some(key) = &self.dpop {
            key.observe(&self.endpoint, &parts.headers);
        }
        let mut body = std::pin::pin!(Limited::new(body, MAX_RESPONSE));
        let mut read = Zeroizing::new(Vec::new());
        while let Some(frame) = body.frame().await {
            let frame =
                frame.map_err(|e| anyhow::anyhow!("failed to read the token response: {e}"))?;
            if let Ok(chunk) = frame.into_data() {
                read.extend_from_slice(&chunk);
                wipe(chunk);
            }
        }
        let body = read;
        let parsed: TokenResponse = serde_json::from_slice(&body).map_err(|_| {
            anyhow::anyhow!("the token endpoint answered {} without JSON", parts.status)
        })?;
        Ok((parts.status, parts.headers, parsed))
    }

    fn check(
        &self,
        status: http::StatusCode,
        mut response: TokenResponse,
    ) -> anyhow::Result<IssuedToken> {
        if !status.is_success() {
            let error = response.error.take().unwrap_or_default();
            anyhow::bail!("the token endpoint refused the exchange ({status}, {error})");
        }
        let access_token = Zeroizing::new(
            response
                .access_token
                .take()
                .filter(|t| !t.is_empty())
                .ok_or_else(|| anyhow::anyhow!("the token response has no access_token"))?,
        );
        let token_type = match response.token_type.as_deref() {
            Some(t) if t.eq_ignore_ascii_case("dpop") && self.dpop.is_some() => TokenType::Dpop,
            Some(t) if t.eq_ignore_ascii_case("bearer") && self.dpop.is_none() => TokenType::Bearer,
            other => anyhow::bail!(
                "the token endpoint issued token_type {other:?}, not the {} asked for",
                if self.dpop.is_some() {
                    "DPoP"
                } else {
                    "Bearer"
                }
            ),
        };
        if self.grant == Grant::TokenExchange {
            anyhow::ensure!(
                response.issued_token_type.as_deref() == Some(TOKEN_TYPE_ACCESS),
                "the token endpoint issued {:?}, not an access token",
                response.issued_token_type
            );
        }
        let lifetime = match &response.expires_in {
            None => self.default_lifetime,
            Some(raw) => {
                let secs = match raw {
                    serde_json::Value::Number(n) => n.as_u64(),
                    serde_json::Value::String(s) => s.parse().ok(),
                    _ => None,
                };
                match secs {
                    Some(secs) if secs > 0 && secs <= self.max_lifetime.as_secs() => {
                        Duration::from_secs(secs)
                    }
                    _ => anyhow::bail!(
                        "the token's expires_in of {raw} is not a whole number of seconds in (0, {}]",
                        self.max_lifetime.as_secs()
                    ),
                }
            }
        };
        if let (Some(requested), Some(granted)) = (&self.scope, &response.scope) {
            let requested: Vec<&str> = requested.split(' ').collect();
            if let Some(extra) = granted.split(' ').find(|s| !requested.contains(s)) {
                anyhow::bail!(
                    "the token endpoint granted scope {extra:?}, which was not asked for"
                );
            }
        }
        let lifetime = self.check_jwt(&access_token, lifetime)?;
        Ok(IssuedToken {
            access_token,
            token_type,
            lifetime,
        })
    }

    /// A token that parses as a JWT is checked against what was asked for. Its
    /// signature is the resource server's business; a token without a JWT
    /// shape is opaque and passes.
    fn check_jwt(&self, token: &str, lifetime: Duration) -> anyhow::Result<Duration> {
        let Some(claims) = jwt_claims(token) else {
            anyhow::ensure!(
                self.expected_audience.is_none(),
                "an audience is expected but the access token is not a JWT"
            );
            return Ok(lifetime);
        };
        if let Some(expected) = &self.expected_audience {
            let matches = match claims.get("aud") {
                Some(serde_json::Value::String(aud)) => aud == expected,
                Some(serde_json::Value::Array(auds)) => auds.iter().any(|a| a == expected.as_str()),
                _ => false,
            };
            anyhow::ensure!(matches, "the access token is not for audience {expected}");
        }
        if let Some(key) = &self.dpop {
            let jkt = claims.pointer("/cnf/jkt").and_then(|j| j.as_str());
            anyhow::ensure!(
                jkt.is_some(),
                "a DPoP access token carries no cnf.jkt binding"
            );
            anyhow::ensure!(
                jkt == Some(key.thumbprint()),
                "the access token is bound to another key"
            );
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64();
        if let Some(nbf) = claims.get("nbf") {
            let nbf = numeric_date(nbf, "nbf")?;
            anyhow::ensure!(
                nbf <= now + CLOCK_SKEW.as_secs_f64(),
                "the access token is not valid yet"
            );
        }
        match claims.get("exp") {
            None => Ok(lifetime),
            Some(exp) => {
                let exp = numeric_date(exp, "exp")?;
                anyhow::ensure!(exp > now, "the access token has already expired");
                Ok(Duration::try_from_secs_f64(exp - now)
                    .map_or(lifetime, |left| lifetime.min(left)))
            }
        }
    }
}

impl EgressCredentialSource for TokenExchange {
    fn current(&self) -> Result<Zeroizing<Vec<u8>>, CredentialError> {
        self.held.current()
    }
}

/// Keeps a [`TokenExchange`] holding a current token until dropped, which
/// stops the refresh and closes the exchange for good. The token is forgotten
/// locally, not revoked at the endpoint.
#[must_use = "the refresh stops and the token is forgotten when this is dropped"]
pub struct TokenRefresh {
    task: tokio::task::JoinHandle<()>,
    exchange: Arc<TokenExchange>,
}

impl Drop for TokenRefresh {
    fn drop(&mut self) {
        self.task.abort();
        self.exchange.held.close();
    }
}

/// Keep `exchange` holding a current token until the returned guard is
/// dropped. Each token is renewed well before it expires; failures back off
/// exponentially with jitter. An exchange is refreshed at most once in its
/// life, so a second call is refused.
pub fn spawn_token_refresh(exchange: Arc<TokenExchange>) -> anyhow::Result<TokenRefresh> {
    anyhow::ensure!(
        !exchange.refreshing.swap(true, Ordering::AcqRel),
        "this token exchange is already refreshed"
    );
    let held = Arc::clone(&exchange);
    let task = tokio::spawn(async move {
        let mut failures: u32 = 0;
        loop {
            let wait = match exchange.exchange().await {
                Ok(issued) => {
                    failures = 0;
                    *exchange
                        .token_type
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) = Some(issued.token_type);
                    exchange.held.renew(
                        Zeroizing::new(issued.access_token.as_bytes().to_vec()),
                        issued.lifetime,
                    );
                    refresh_after(issued.lifetime)
                }
                Err(err) => {
                    warn!(endpoint = %exchange.endpoint, %err, "failed to exchange a workload identity for an access token");
                    failures = failures.saturating_add(1);
                    backoff(failures)
                }
            };
            tokio::time::sleep(wait).await;
        }
    });
    Ok(TokenRefresh {
        task,
        exchange: held,
    })
}

/// Clear a chunk the response was read into, when nothing else shares it.
fn wipe(chunk: bytes::Bytes) {
    use zeroize::Zeroize as _;
    if let Ok(mut owned) = chunk.try_into_mut() {
        owned.as_mut().zeroize();
    }
}

/// When to renew a token: a fifth of its life early, at least [`MIN_SKEW`],
/// but never in the first half of a short token's life.
fn refresh_after(lifetime: Duration) -> Duration {
    let skew = (lifetime / 5).max(MIN_SKEW).min(lifetime / 2);
    lifetime.saturating_sub(skew).max(Duration::from_secs(1))
}

/// Equal jitter, half to all of an exponential ceiling, so many workloads failing
/// against one endpoint do not retry in step.
fn backoff(failures: u32) -> Duration {
    let ceiling = RETRY
        .saturating_mul(1 << failures.saturating_sub(1).min(16))
        .min(MAX_BACKOFF);
    ceiling.mul_f64(0.5 + fastrand::f64() / 2.0)
}

fn numeric_date(value: &serde_json::Value, claim: &str) -> anyhow::Result<f64> {
    value
        .as_f64()
        .filter(|v| v.is_finite())
        .ok_or_else(|| anyhow::anyhow!("the access token's {claim} is not a NumericDate"))
}

fn connector(
    tls: &rustls::ClientConfig,
) -> hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector> {
    crate::init_crypto();
    let mut http = hyper_util::client::legacy::connect::HttpConnector::new();
    http.enforce_http(false);
    http.set_nodelay(true);
    http.set_connect_timeout(Some(CONNECT_TIMEOUT));
    hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(tls.clone())
        .https_or_http()
        .enable_http1()
        .wrap_connector(http)
}

fn jwt_claims(token: &str) -> Option<serde_json::Value> {
    let mut parts = token.split('.');
    let (_, payload, _) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    serde_json::from_slice(&B64.decode(payload).ok()?).ok()
}

fn form_encode(out: &mut String, value: &str) {
    use std::fmt::Write as _;
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte))
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
