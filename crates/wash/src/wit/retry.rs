//! Which registry failures a WIT fetch retries, how often, and how long it waits between tries.

use std::{io::ErrorKind, time::Duration};

use oci_client::errors::{OciDistributionError, OciErrorCode};
use tracing::warn;

pub(super) const WIT_FETCH_MAX_ATTEMPTS: u32 = 3;
const WIT_FETCH_INITIAL_BACKOFF: Duration = Duration::from_millis(100);

pub(super) async fn wait_to_retry_wit_fetch(error: &anyhow::Error, attempt: u32) {
    let delay = wit_fetch_retry_delay(attempt);
    warn!(
        attempt,
        max_attempts = WIT_FETCH_MAX_ATTEMPTS,
        delay_ms = delay.as_millis(),
        error = %error,
        "transient registry fetch failed; retrying"
    );
    tokio::time::sleep(delay).await;
}

fn wit_fetch_retry_delay(failed_attempt: u32) -> Duration {
    let exponent = failed_attempt.saturating_sub(1).min(3);
    let base = WIT_FETCH_INITIAL_BACKOFF.saturating_mul(1 << exponent);
    base + Duration::from_millis(fastrand::u64(0..base.as_millis() as u64))
}

// Downcasts require the same dependency versions used by wasm-pkg-client and oci-client.
const _: () = {
    use wasm_pkg_client::oci::client::{Client, ClientConfig};
    let _: fn(<Client as TryFrom<ClientConfig>>::Error) -> OciDistributionError = |error| error;
    let _: fn(reqwest_oci::Error) -> OciDistributionError = OciDistributionError::RequestError;
};

pub(super) fn is_transient_registry_fetch_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        if let Some(error) = cause.downcast_ref::<OciDistributionError>() {
            return match error {
                OciDistributionError::ServerError { code, .. } => {
                    *code == 429 || (500..=599).contains(code)
                }
                OciDistributionError::RequestError(error) => is_transient_reqwest_error(error),
                OciDistributionError::IoError(error) => is_transient_io_error(error),
                OciDistributionError::RegistryError { envelope, .. } => {
                    !envelope.errors.is_empty()
                        && envelope
                            .errors
                            .iter()
                            .all(|error| error.code == OciErrorCode::Toomanyrequests)
                }
                _ => false,
            };
        }

        if let Some(error) = cause.downcast_ref::<reqwest_oci::Error>() {
            return is_transient_reqwest_error(error);
        }

        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(is_transient_io_error)
    })
}

fn is_transient_reqwest_error(error: &reqwest_oci::Error) -> bool {
    error.is_timeout()
        || error.is_connect()
        || error.is_body()
        || error
            .status()
            .is_some_and(|status| status.as_u16() == 429 || status.is_server_error())
}

fn is_transient_io_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::ConnectionAborted
            | ErrorKind::ConnectionReset
            | ErrorKind::Interrupted
            | ErrorKind::TimedOut
            | ErrorKind::UnexpectedEof
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use oci_client::errors::{OciEnvelope, OciError};

    fn registry_fetch_error(error: OciDistributionError) -> anyhow::Error {
        wasm_pkg_client::Error::RegistryError(anyhow::Error::new(error)).into()
    }

    fn registry_envelope_error(codes: Vec<OciErrorCode>) -> anyhow::Error {
        registry_fetch_error(OciDistributionError::RegistryError {
            envelope: OciEnvelope {
                errors: codes
                    .into_iter()
                    .map(|code| OciError {
                        code,
                        message: String::new(),
                        detail: serde_json::Value::Null,
                    })
                    .collect(),
            },
            url: "https://registry.example.test".to_string(),
        })
    }

    #[test]
    fn wit_fetch_retries_only_transient_registry_statuses() {
        for code in [429, 500, 503] {
            let error = registry_fetch_error(OciDistributionError::ServerError {
                code,
                url: "https://registry.example.test".to_string(),
                message: String::new(),
            });
            assert!(is_transient_registry_fetch_error(&error), "status {code}");
        }

        for code in [400, 401, 403, 404] {
            let error = registry_fetch_error(OciDistributionError::ServerError {
                code,
                url: "https://registry.example.test".to_string(),
                message: String::new(),
            });
            assert!(!is_transient_registry_fetch_error(&error), "status {code}");
        }
    }

    #[test]
    fn wit_fetch_retries_rate_limit_envelopes_but_not_authentication_errors() {
        assert!(is_transient_registry_fetch_error(&registry_envelope_error(
            vec![OciErrorCode::Toomanyrequests]
        )));
        assert!(!is_transient_registry_fetch_error(
            &registry_envelope_error(vec![OciErrorCode::Unauthorized])
        ));
        assert!(!is_transient_registry_fetch_error(
            &registry_envelope_error(vec![
                OciErrorCode::Toomanyrequests,
                OciErrorCode::Unauthorized,
            ])
        ));
    }

    #[test]
    fn wit_fetch_retries_transient_io_errors() {
        let reset = registry_fetch_error(OciDistributionError::IoError(std::io::Error::from(
            ErrorKind::ConnectionReset,
        )));
        assert!(is_transient_registry_fetch_error(&reset));

        let denied = registry_fetch_error(OciDistributionError::IoError(std::io::Error::from(
            ErrorKind::PermissionDenied,
        )));
        assert!(!is_transient_registry_fetch_error(&denied));
    }

    #[tokio::test]
    async fn wit_fetch_retries_truncated_response_bodies() {
        let body = reqwest_oci::Body::wrap_stream(futures::stream::iter([
            Ok(bytes::Bytes::from_static(b"partial")),
            Err(std::io::Error::from(ErrorKind::ConnectionReset)),
        ]));
        let response = reqwest_oci::Response::from(http::Response::new(body));
        let error = response.bytes().await.unwrap_err();

        let error = anyhow::Error::new(std::io::Error::other(error));
        assert!(is_transient_registry_fetch_error(&error));
    }

    #[test]
    fn wit_fetch_retry_delay_is_exponential_with_bounded_jitter() {
        for (failed_attempt, lower_ms, upper_ms) in [(1, 100, 200), (2, 200, 400), (3, 400, 800)] {
            let delay = wit_fetch_retry_delay(failed_attempt);
            assert!(delay >= Duration::from_millis(lower_ms));
            assert!(delay < Duration::from_millis(upper_ms));
        }
    }
}
