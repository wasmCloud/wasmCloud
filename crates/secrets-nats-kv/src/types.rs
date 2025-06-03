use bytes::Bytes;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Serialize, Deserialize, Default)]
pub struct PutSecretRequest {
    pub key: String,
    pub version: String,
    pub string_secret: Option<String>,
    pub binary_secret: Option<Vec<u8>>,
}

/// The response to a `put_secret` operation.
/// This response contains the revision number of the secret that was just written.
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct PutSecretResponse {
    pub revision: u64,
    pub error: Option<PutSecretError>,
}

#[derive(Serialize, Deserialize, Debug, Error)]
pub enum PutSecretError {
    #[error("Invalid payload")]
    InvalidPayload,
    #[error("Invalid headers")]
    InvalidHeaders,
    #[error("Invalid XKey")]
    InvalidXKey,
    #[error("Error decrypting secret")]
    DecryptionError,
    #[error("No secret provided")]
    NoSecretProvided,
}

impl From<PutSecretError> for PutSecretResponse {
    fn from(e: PutSecretError) -> Self {
        PutSecretResponse {
            error: Some(e),
            ..Default::default()
        }
    }
}

impl From<PutSecretResponse> for Bytes {
    fn from(resp: PutSecretResponse) -> Self {
        let encoded = serde_json::to_vec(&resp).unwrap();
        Bytes::from(encoded)
    }
}

impl From<u64> for PutSecretResponse {
    fn from(r: u64) -> Self {
        Self {
            revision: r,
            error: None,
        }
    }
}
