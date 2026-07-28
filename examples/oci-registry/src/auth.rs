//! HTTP Basic authentication for the registry.
//!
//! Credentials come from a host component plugin (the secrets backend), imported
//! as `wasmcloud:secrets/store` and unwrapped with `wasmcloud:secrets/reveal`.
//! Every request must carry a matching `Authorization: Basic` header — including
//! the `GET /v2/` probe, which is where an OCI client discovers it must
//! authenticate.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use sha2::{Digest, Sha256};

use crate::bindings::wasmcloud::secrets::reveal::reveal;
use crate::bindings::wasmcloud::secrets::store::{self, SecretValue};
use crate::http::{header_str, respond};
use crate::{Fields, Response};

/// Config keys the secrets backend is expected to serve.
const USERNAME_KEY: &str = "registry-username";
const PASSWORD_KEY: &str = "registry-password";
const REALM: &str = "wasmcloud-oci-registry";

/// Returns `Some(challenge)` — a `401` with a `WWW-Authenticate: Basic` header —
/// when the request is not authenticated, and `None` when the credentials match.
pub(crate) async fn require_basic(headers: &Fields) -> Option<Response> {
    let (Some(username), Some(password)) = (
        secret_string(USERNAME_KEY).await,
        secret_string(PASSWORD_KEY).await,
    ) else {
        // The backend did not supply both credentials; deny rather than serve
        // the registry without authentication.
        return Some(challenge());
    };
    let expected = STANDARD.encode(format!("{username}:{password}"));
    match header_str(headers, "authorization").and_then(|h| basic_token(&h)) {
        Some(actual) if tokens_match(&actual, &expected) => None,
        _ => Some(challenge()),
    }
}

/// Fetch a credential from the secrets backend and reveal it as a UTF-8 string.
async fn secret_string(key: &str) -> Option<String> {
    let secret = store::get(key.to_string()).await.ok()?;
    match reveal(&secret).await {
        SecretValue::String(value) => Some(value),
        SecretValue::Bytes(bytes) => String::from_utf8(bytes).ok(),
    }
}

/// Extract the base64 credential token from an `Authorization: Basic <token>`
/// header. The scheme is matched case-insensitively per RFC 7617; any other
/// scheme (or a header without a token) yields `None`.
fn basic_token(header: &str) -> Option<String> {
    let (scheme, token) = header.trim().split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("Basic")
        .then(|| token.trim().to_string())
}

/// Constant-time credential comparison that leaks neither the value nor its
/// length: each token is hashed to a fixed 32 bytes and the digests are compared
/// byte-for-byte, so the work done is independent of the inputs. Equal digests
/// imply equal tokens (SHA-256 is collision resistant).
fn tokens_match(actual: &str, expected: &str) -> bool {
    let actual = Sha256::digest(actual.as_bytes());
    let expected = Sha256::digest(expected.as_bytes());
    let mut diff = 0u8;
    for (a, b) in actual.iter().zip(expected.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

fn challenge() -> Response {
    let header = format!("Basic realm=\"{REALM}\"");
    respond(
        401,
        &[
            ("www-authenticate", header.as_str()),
            ("content-type", "application/json"),
            ("docker-distribution-api-version", "registry/2.0"),
        ],
        br#"{"errors":[{"code":"UNAUTHORIZED","message":"authentication required"}]}"#.to_vec(),
    )
}
