//! This module serves as a copy of axum's Host header as it is soon to be deprecated
//!
//! the `Host` extractor will be deprecated in a future version of axum due to potential
//! for minsuse. This mesage was originally present:
//! ```
//! #[deprecated = "will be removed in the next version; see https://github.com/tokio-rs/axum/issues/3442"]
//! ```
//!
//! Do *not* edit this file, as the upstream tests have been removed!

use axum::{
    extract::{FromRequestParts, OptionalFromRequestParts},
    RequestPartsExt,
};
use axum_extra::extract::rejection::HostRejection;
use http::{
    header::{HeaderMap, FORWARDED},
    request::Parts,
    uri::Authority,
};
use std::convert::Infallible;

const X_FORWARDED_HOST_HEADER_KEY: &str = "X-Forwarded-Host";

/// Extractor that resolves the host of the request.
///
/// Host is resolved through the following, in order:
/// - `Forwarded` header
/// - `X-Forwarded-Host` header
/// - `Host` header
/// - Authority of the request URI
///
/// See <https://www.rfc-editor.org/rfc/rfc9110.html#name-host-and-authority> for the definition of
/// host.
///
/// Note that user agents can set `X-Forwarded-Host` and `Host` headers to arbitrary values so make
/// sure to validate them to avoid security issues.
#[derive(Debug, Clone)]
pub struct Host(pub String);

impl<S> FromRequestParts<S> for Host
where
    S: Send + Sync,
{
    type Rejection = HostRejection;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extract::<Option<Host>>()
            .await
            .ok()
            .flatten()
            .ok_or(HostRejection::FailedToResolveHost(Default::default()))
    }
}

impl<S> OptionalFromRequestParts<S> for Host
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        if let Some(host) = parse_forwarded(&parts.headers) {
            return Ok(Some(Host(host.to_owned())));
        }

        if let Some(host) = parts
            .headers
            .get(X_FORWARDED_HOST_HEADER_KEY)
            .and_then(|host| host.to_str().ok())
        {
            return Ok(Some(Host(host.to_owned())));
        }

        if let Some(host) = parts
            .headers
            .get(http::header::HOST)
            .and_then(|host| host.to_str().ok())
        {
            return Ok(Some(Host(host.to_owned())));
        }

        if let Some(authority) = parts.uri.authority() {
            return Ok(Some(Host(parse_authority(authority).to_owned())));
        }

        Ok(None)
    }
}

#[allow(warnings)]
fn parse_forwarded(headers: &HeaderMap) -> Option<&str> {
    // if there are multiple `Forwarded` `HeaderMap::get` will return the first one
    let forwarded_values = headers.get(FORWARDED)?.to_str().ok()?;

    // get the first set of values
    let first_value = forwarded_values.split(',').nth(0)?;

    // find the value of the `host` field
    first_value.split(';').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case("host")
            .then(|| value.trim().trim_matches('"'))
    })
}

fn parse_authority(auth: &Authority) -> &str {
    auth.as_str()
        .rsplit('@')
        .next()
        .expect("split always has at least 1 item")
}
