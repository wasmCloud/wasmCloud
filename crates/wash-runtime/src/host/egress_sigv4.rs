//! AWS Signature Version 4 re-signing for placeholder access keys, for the S3
//! family of services only.
//!
//! The guest's SDK signs with a placeholder access key ID and any secret key.
//! When the host finds a placeholder it holds in the `Credential=` scope, it
//! replaces the access key ID, sets `x-amz-security-token` and `x-amz-date`
//! from its own credentials and clock, and recomputes the signature.
//!
//! The body is never read. The payload hash comes from `x-amz-content-sha256`
//! as the SDK sent it, so a request without one is refused, as are chunked
//! uploads, whose signature chains through every chunk, and query-string
//! (presigned) signing, which would let the host mint URLs anyone can use.
//! Only S3 SDKs send that header, which is why other services are refused:
//! signing them would mean buffering the body to hash it.

use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use hmac::{Hmac, Mac as _};
use http::{HeaderName, HeaderValue};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use crate::host::egress_credential::{
    CarriesSecret, CredentialError, MIN_RANDOM, PLACEHOLDER_MARKER, Refused, SecretDestination,
};

const ALGORITHM: &str = "AWS4-HMAC-SHA256";
const CONTENT_SHA256: HeaderName = HeaderName::from_static("x-amz-content-sha256");
const AMZ_DATE: HeaderName = HeaderName::from_static("x-amz-date");
const SECURITY_TOKEN: HeaderName = HeaderName::from_static("x-amz-security-token");
const DECODED_LENGTH: HeaderName = HeaderName::from_static("x-amz-decoded-content-length");

/// Services that send `x-amz-content-sha256` and sign the path as sent.
pub const S3_SERVICES: [&str; 4] = ["s3", "s3express", "s3-object-lambda", "s3-outposts"];

/// Credentials the host signs with.
pub struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: Zeroizing<String>,
    pub session_token: Option<Zeroizing<String>>,
    /// When the credentials stop working, for temporary ones.
    pub expires_at: Option<SystemTime>,
}

impl std::fmt::Debug for AwsCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsCredentials")
            .field("access_key_id", &self.access_key_id)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// Where signing credentials come from.
///
/// Called on the request path, so it must answer from what it already holds,
/// as [`EgressCredentialSource`](crate::host::egress_credential::EgressCredentialSource)
/// does.
pub trait AwsCredentialSource: Send + Sync + 'static {
    fn current(&self) -> Result<Arc<AwsCredentials>, CredentialError>;
}

/// Credentials served until their lease runs out, they expire, or they are
/// revoked.
#[derive(Default)]
pub struct LeasedAwsCredentials {
    held: Mutex<Option<(Arc<AwsCredentials>, Instant)>>,
}

impl LeasedAwsCredentials {
    pub fn renew(&self, credentials: AwsCredentials, valid_for: std::time::Duration) {
        let until = Instant::now() + valid_for;
        *self.held.lock().unwrap_or_else(|e| e.into_inner()) = Some((Arc::new(credentials), until));
    }

    pub fn revoke(&self) {
        self.held.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

impl AwsCredentialSource for LeasedAwsCredentials {
    fn current(&self) -> Result<Arc<AwsCredentials>, CredentialError> {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        match &*held {
            Some((credentials, until))
                if Instant::now() < *until
                    && credentials
                        .expires_at
                        .is_none_or(|at| at > SystemTime::now()) =>
            {
                Ok(Arc::clone(credentials))
            }
            _ => {
                held.take();
                Err(CredentialError::Unavailable)
            }
        }
    }
}

/// A placeholder access key ID the host re-signs for.
pub struct SigV4Secret {
    placeholder: Arc<str>,
    source: Arc<dyn AwsCredentialSource>,
    destinations: Vec<SecretDestination>,
    services: Vec<String>,
    regions: Vec<String>,
}

impl SigV4Secret {
    /// `placeholder` is an optional prefix, [`PLACEHOLDER_MARKER`], then at
    /// least [`MIN_RANDOM`] alphanumerics: the shape the host looks for
    /// before it asks for secrets at all. Only letters, digits and '_', as an
    /// access key ID allows.
    pub fn new(placeholder: &str, source: Arc<dyn AwsCredentialSource>) -> anyhow::Result<Self> {
        let shaped = placeholder
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && placeholder
                .split_once(PLACEHOLDER_MARKER)
                .is_some_and(|(prefix, random)| {
                    !prefix.contains(PLACEHOLDER_MARKER)
                        && random.len() >= MIN_RANDOM
                        && random.bytes().all(|b| b.is_ascii_alphanumeric())
                });
        anyhow::ensure!(
            shaped,
            "a SigV4 placeholder is an optional [A-Za-z0-9_] prefix, `{PLACEHOLDER_MARKER}`, then              at least {MIN_RANDOM} alphanumerics"
        );
        Ok(Self {
            placeholder: placeholder.into(),
            source,
            destinations: Vec::new(),
            services: Vec::new(),
            regions: Vec::new(),
        })
    }

    /// The exact https hosts and ports requests may be re-signed for. None
    /// declared refuses every request.
    pub fn for_destinations(
        mut self,
        destinations: impl IntoIterator<Item = SecretDestination>,
    ) -> Self {
        self.destinations.extend(destinations);
        self
    }

    /// Services (the scope's fourth field) requests may be signed for, each
    /// one of [`S3_SERVICES`]. None declared refuses every request.
    pub fn for_services<S: AsRef<str>>(
        mut self,
        services: impl IntoIterator<Item = S>,
    ) -> anyhow::Result<Self> {
        for service in services {
            let service = service.as_ref();
            anyhow::ensure!(
                S3_SERVICES.contains(&service),
                "SigV4 re-signing is for {S3_SERVICES:?} only, not {service:?}"
            );
            self.services.push(service.to_string());
        }
        Ok(self)
    }

    /// Regions requests may be signed for. None declared permits any.
    pub fn for_regions<S: Into<String>>(mut self, regions: impl IntoIterator<Item = S>) -> Self {
        self.regions.extend(regions.into_iter().map(Into::into));
        self
    }

    pub fn placeholder(&self) -> &str {
        &self.placeholder
    }
}

impl std::fmt::Debug for SigV4Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SigV4Secret")
            .field("destinations", &self.destinations)
            .field("services", &self.services)
            .field("regions", &self.regions)
            .finish_non_exhaustive()
    }
}

/// The guest's `Authorization` header, parsed.
struct Signed<'a> {
    access_key_id: &'a str,
    region: &'a str,
    service: &'a str,
    signed_headers: Vec<&'a str>,
}

fn parse_authorization(value: &str) -> Option<Signed<'_>> {
    let rest = value.strip_prefix(ALGORITHM)?.strip_prefix(' ')?;
    let mut credential = None;
    let mut signed_headers = None;
    let mut signature = false;
    for field in rest.split(',') {
        let (name, value) = field.trim().split_once('=')?;
        match name {
            "Credential" => credential = Some(value),
            "SignedHeaders" => signed_headers = Some(value),
            "Signature" => signature = true,
            _ => return None,
        }
    }
    let mut scope = credential?.split('/');
    let (Some(access_key_id), Some(_date), Some(region), Some(service), Some("aws4_request"), None) = (
        scope.next(),
        scope.next(),
        scope.next(),
        scope.next(),
        scope.next(),
        scope.next(),
    ) else {
        return None;
    };
    signature.then_some(Signed {
        access_key_id,
        region,
        service,
        signed_headers: signed_headers?.split(';').collect(),
    })
}

/// Whether `value` is a SigV4 `Authorization` naming a placeholder.
fn names_placeholder(value: &HeaderValue) -> bool {
    let marker = PLACEHOLDER_MARKER.as_bytes();
    value.as_bytes().starts_with(ALGORITHM.as_bytes())
        && value.as_bytes().windows(marker.len()).any(|w| w == marker)
}

/// Take `request`'s SigV4 `Authorization` if it names a placeholder, so
/// substitution leaves it to [`resign`].
fn take_authorization<B>(request: &mut hyper::Request<B>) -> Option<HeaderValue> {
    let mut values = request
        .headers()
        .get_all(http::header::AUTHORIZATION)
        .iter();
    match (values.next(), values.next()) {
        (Some(value), None) if names_placeholder(value) => {
            request.headers_mut().remove(http::header::AUTHORIZATION)
        }
        _ => None,
    }
}

/// Substitute `secrets` into `request`, then re-sign it for `signers`, so
/// the signature covers the values that go out. The SigV4 placeholder in
/// `Authorization` is held back from substitution for the re-sign to find.
pub(crate) fn substitute_and_sign<B>(
    request: &mut hyper::Request<B>,
    secrets: &[crate::host::egress_credential::EgressSecret],
    signers: &[SigV4Secret],
    now: SystemTime,
) -> Result<crate::host::egress_credential::Substituted, Refused> {
    let signing = take_authorization(request);
    let mut done = crate::host::egress_credential::substitute(request, secrets)?;
    if let Some(authorization) = signing {
        request
            .headers_mut()
            .insert(http::header::AUTHORIZATION, authorization);
    }
    done.used.extend(resign(request, signers, now)?);
    Ok(done)
}

/// Re-sign `request` if its SigV4 access key ID is a placeholder in
/// `secrets`. Returns the placeholder re-signed for, or `None` when the
/// request carries no SigV4 placeholder.
pub(crate) fn resign<B>(
    request: &mut hyper::Request<B>,
    secrets: &[SigV4Secret],
    now: SystemTime,
) -> Result<Option<Arc<str>>, Refused> {
    if request
        .uri()
        .query()
        .is_some_and(|q| q.contains("X-Amz-Credential") && q.contains(PLACEHOLDER_MARKER))
    {
        return Err(CredentialError::Misplaced.into());
    }
    let mut authorizations = request
        .headers()
        .get_all(http::header::AUTHORIZATION)
        .iter();
    let (Some(authorization), None) = (authorizations.next(), authorizations.next()) else {
        return Ok(None);
    };
    if !names_placeholder(authorization) {
        return Ok(None);
    }
    let value = authorization
        .to_str()
        .map_err(|_| Refused::from(CredentialError::Misplaced))?
        .to_string();
    let signed = parse_authorization(&value).ok_or(CredentialError::Misplaced)?;
    let secret = secrets
        .iter()
        .find(|s| *s.placeholder == *signed.access_key_id)
        .ok_or(CredentialError::Unknown)?;
    let refused = |error| Refused {
        placeholder: Some(Arc::clone(&secret.placeholder)),
        error,
    };

    if request.uri().scheme() != Some(&http::uri::Scheme::HTTPS) {
        return Err(refused(CredentialError::NotTls));
    }
    if request
        .uri()
        .authority()
        .is_none_or(|a| a.as_str().contains('@'))
        || !secret
            .destinations
            .iter()
            .any(|destination| destination.matches(request.uri()))
        || !secret.services.iter().any(|s| s == signed.service)
        || (!secret.regions.is_empty() && !secret.regions.iter().any(|r| r == signed.region))
    {
        return Err(refused(CredentialError::Destination));
    }

    let payload_hash = payload_hash(request.headers()).map_err(refused)?;
    let credentials = secret.source.current().map_err(refused)?;

    let region = signed.region.to_string();
    let service = signed.service.to_string();
    let mut names: Vec<String> = signed
        .signed_headers
        .iter()
        .map(|h| h.to_ascii_lowercase())
        .filter(|h| h != SECURITY_TOKEN.as_str())
        .collect();
    for required in ["host", AMZ_DATE.as_str(), CONTENT_SHA256.as_str()] {
        names.push(required.to_string());
    }
    if credentials.session_token.is_some() {
        names.push(SECURITY_TOKEN.to_string());
    }
    names.sort();
    names.dedup();
    // A placeholder still in what is signed would be sent, and echoed back in
    // any signature error, so only a fully substituted request is signed.
    let marker = PLACEHOLDER_MARKER.as_bytes();
    let unsubstituted = |bytes: &[u8]| {
        percent_decode(bytes)
            .windows(marker.len())
            .any(|w| w == marker)
    };
    let headers_left = names.iter().any(|name| {
        request
            .headers()
            .get_all(name.as_str())
            .iter()
            .any(|v| unsubstituted(v.as_bytes()))
    });
    if headers_left
        || request
            .uri()
            .path_and_query()
            .is_some_and(|p| unsubstituted(p.as_str().as_bytes()))
    {
        return Err(refused(CredentialError::Misplaced));
    }
    let host = sent_host(request.uri()).ok_or_else(|| refused(CredentialError::Destination))?;

    let stamp = chrono::DateTime::<chrono::Utc>::from(now);
    let amz_date = stamp.format("%Y%m%dT%H%M%SZ").to_string();
    let date = stamp.format("%Y%m%d").to_string();
    let headers = request.headers_mut();
    headers.insert(
        http::header::HOST,
        HeaderValue::from_str(&host).map_err(|_| refused(CredentialError::InvalidValue))?,
    );
    headers.remove(&SECURITY_TOKEN);
    headers.insert(
        AMZ_DATE,
        HeaderValue::from_str(&amz_date).map_err(|_| refused(CredentialError::InvalidValue))?,
    );
    if let Some(token) = &credentials.session_token {
        let mut value =
            HeaderValue::from_str(token).map_err(|_| refused(CredentialError::InvalidValue))?;
        value.set_sensitive(true);
        headers.insert(SECURITY_TOKEN, value);
    }

    let canonical = canonical_request(request, &names, &payload_hash).map_err(refused)?;
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let string_to_sign = format!(
        "{ALGORITHM}\n{amz_date}\n{scope}\n{}",
        hex(&Sha256::digest(canonical.as_bytes()))
    );
    let signature = signature(
        &credentials.secret_access_key,
        &date,
        &region,
        &service,
        &string_to_sign,
    );
    let authorization = format!(
        "{ALGORITHM} Credential={}/{scope}, SignedHeaders={}, Signature={signature}",
        credentials.access_key_id,
        names.join(";")
    );
    let mut value = HeaderValue::from_str(&authorization)
        .map_err(|_| refused(CredentialError::InvalidValue))?;
    value.set_sensitive(true);
    request
        .headers_mut()
        .insert(http::header::AUTHORIZATION, value);
    request.extensions_mut().insert(CarriesSecret);
    Ok(Some(Arc::clone(&secret.placeholder)))
}

/// The `Host` hyper sends for `uri`: its host, and its port unless it is
/// the scheme's default.
fn sent_host(uri: &hyper::Uri) -> Option<String> {
    let host = uri.host()?;
    Some(match uri.port_u16() {
        Some(port) if port != 443 => format!("{host}:{port}"),
        _ => host.to_string(),
    })
}

/// The payload hash as the guest sent it, if it is one the host can sign
/// without reading the body.
fn payload_hash(headers: &http::HeaderMap) -> Result<String, CredentialError> {
    let chunked = headers
        .get_all(http::header::CONTENT_ENCODING)
        .iter()
        .any(|v| v.to_str().is_ok_and(|v| v.contains("aws-chunked")));
    if chunked || headers.contains_key(&DECODED_LENGTH) {
        return Err(CredentialError::Misplaced);
    }
    let mut values = headers.get_all(&CONTENT_SHA256).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return Err(CredentialError::Misplaced);
    };
    let value = value.to_str().map_err(|_| CredentialError::Misplaced)?;
    let is_hash = value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if is_hash || value == "UNSIGNED-PAYLOAD" {
        Ok(value.to_string())
    } else {
        Err(CredentialError::Misplaced)
    }
}

/// The canonical request as the S3 family signs it: the path exactly as
/// sent, never normalized or encoded again.
fn canonical_request<B>(
    request: &hyper::Request<B>,
    names: &[String],
    payload_hash: &str,
) -> Result<String, CredentialError> {
    let uri = request.uri();
    let path = match uri.path() {
        "" => "/",
        path => path,
    };
    let mut query: Vec<(String, String)> = uri
        .query()
        .unwrap_or_default()
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (
                encode(&percent_decode(k.as_bytes())),
                encode(&percent_decode(v.as_bytes())),
            )
        })
        .collect();
    query.sort();
    let query = query
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");

    let mut headers = String::new();
    for name in names {
        let value = if name == "host" {
            match request.headers().get(http::header::HOST) {
                Some(host) => host
                    .to_str()
                    .map_err(|_| CredentialError::InvalidValue)?
                    .to_string(),
                None => sent_host(uri).ok_or(CredentialError::Destination)?,
            }
        } else {
            let values: Vec<String> = request
                .headers()
                .get_all(name.as_str())
                .iter()
                .map(|v| v.to_str().map(collapse))
                .collect::<Result<_, _>>()
                .map_err(|_| CredentialError::InvalidValue)?;
            if values.is_empty() {
                return Err(CredentialError::Misplaced);
            }
            values.join(",")
        };
        headers.push_str(name);
        headers.push(':');
        headers.push_str(&value);
        headers.push('\n');
    }
    Ok(format!(
        "{}\n{path}\n{query}\n{headers}\n{}\n{payload_hash}",
        request.method().as_str(),
        names.join(";")
    ))
}

/// Trim and collapse runs of spaces, as canonical header values are.
fn collapse(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// URI-encode as SigV4 does a query component: everything but unreserved
/// characters.
fn encode(value: &[u8]) -> String {
    let mut out = String::with_capacity(value.len());
    for &byte in value {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte))
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The bytes `bytes` percent-decodes to, which need not be UTF-8.
fn percent_decode(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        let hex = |b: u8| char::from(b).to_digit(16);
        match (byte, bytes.get(i + 1).copied(), bytes.get(i + 2).copied()) {
            (b'%', Some(h), Some(l)) if hex(h).is_some() && hex(l).is_some() => {
                let value = hex(h).unwrap_or_default() * 16 + hex(l).unwrap_or_default();
                out.push(u8::try_from(value).unwrap_or_default());
                i += 3;
            }
            _ => {
                out.push(byte);
                i += 1;
            }
        }
    }
    out
}

fn hmac(key: &[u8], data: &[u8]) -> Zeroizing<Vec<u8>> {
    // HMAC accepts keys of every length, so this never fails.
    let Ok(mut mac) = <Hmac<Sha256>>::new_from_slice(key) else {
        return Zeroizing::new(Vec::new());
    };
    mac.update(data);
    Zeroizing::new(mac.finalize().into_bytes().to_vec())
}

fn signature(
    secret: &str,
    date: &str,
    region: &str,
    service: &str,
    string_to_sign: &str,
) -> String {
    let key = Zeroizing::new(format!("AWS4{secret}"));
    let k_date = hmac(key.as_bytes(), date.as_bytes());
    let k_region = hmac(&k_date, region.as_bytes());
    let k_service = hmac(&k_region, service.as_bytes());
    let k_signing = hmac(&k_service, b"aws4_request");
    hex(&hmac(&k_signing, string_to_sign.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests;
