use anyhow::{anyhow, Context as _, Result};
use wasmcloud_component::http;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SecretQuery {
    /// The key to use to retrieve the secret value
    ///
    /// If the secret has been supplied via WADM manifest, this will be the secret name (not the `key` field)
    pub(crate) key: String,

    pub(crate) field: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum PasswordCheckRequest {
    RawText { value: String },
    SecretQuery { secret: SecretQuery },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PasswordStrength {
    VeryWeak,
    Weak,
    Medium,
    Strong,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordCheckResponse {
    /// Size of the input
    pub(crate) strength: PasswordStrength,
    /// Size of the input
    pub(crate) length: usize,
    /// Size of the input
    pub(crate) contains: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum ResponseStatus {
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ErrorInfo {
    pub(crate) code: String,
    pub(crate) msg: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ResponseEnvelope<T> {
    Success { body: T },
    Error { error: ErrorInfo },
}

impl<T: serde::Serialize> ResponseEnvelope<T> {
    pub(crate) fn serialize_body(&self) -> Result<Vec<u8>> {
        match self {
            ResponseEnvelope::Success { ref body } => {
                serde_json::to_vec(body).map_err(|e| anyhow!(e))
            }
            ResponseEnvelope::Error { ref error } => {
                serde_json::to_vec(error).map_err(|e| anyhow!(e))
            }
        }
    }

    pub(crate) fn into_http_resp(
        self,
        code: http::StatusCode,
    ) -> http::Result<http::Response<Vec<u8>>> {
        let body_bytes = match self.serialize_body() {
            Ok(b) => b,
            Err(e) => {
                return http::Result::Ok(http::Response::new(
                    format!("internal error while serializing request body: {e}").into_bytes(),
                ));
            }
        };

        http::Result::Ok(
            http::Response::builder()
                .header("Content-Type", "application/json")
                .status(code)
                .body(body_bytes)
                .context("failed to build error response body")
                .unwrap_or_else(|e| {
                    http::Response::new(
                        format!("internal error while building request body: {e}").into_bytes(),
                    )
                }),
        )
    }
}

/// Build an JSON error response
pub(crate) fn error_resp_json(
    http_code: http::StatusCode,
    body: impl serde::Serialize,
) -> http::Result<http::Response<Vec<u8>>> {
    let body = match serde_json::to_vec(&body).context("failed to serialize") {
        Ok(bytes) => bytes,
        Err(e) => format!("internal error while serializing request body: {e}").into_bytes(),
    };
    http::Result::Ok(
        http::Response::builder()
            .header("Content-Type", "application/json")
            .status(http_code)
            .body(body)
            .context("failed to build error response body")
            .unwrap_or_else(|e| {
                http::Response::new(
                    format!("internal error while building request body: {e}").into_bytes(),
                )
            }),
    )
}
