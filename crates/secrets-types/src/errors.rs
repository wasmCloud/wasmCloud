use bytes::Bytes;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::SecretResponse;

#[derive(Error, Debug, Serialize, Deserialize)]
pub enum ContextValidationError {
    #[error("Invalid Component JWT: {0}")]
    InvalidComponentJWT(String),
    #[error("Invalid Provider JWT: {0}")]
    InvalidProviderJWT(String),
    #[error("Invalid Host JWT: {0}")]
    InvalidHostJWT(String),
}

#[derive(Error, Debug, Serialize, Deserialize)]
pub enum GetSecretError {
    #[error("Invalid Entity JWT: {0}")]
    InvalidEntityJWT(String),
    #[error("Invalid Host JWT: {0}")]
    InvalidHostJWT(String),
    #[error("Secret not found")]
    SecretNotFound,
    #[error("Invalid XKey")]
    InvalidXKey,
    #[error("Error encrypting secret")]
    EncryptionError,
    #[error("Error decrypting secret")]
    DecryptionError,
    #[error("Error fetching secret: {0}")]
    UpstreamError(String),
    #[error("Error fetching secret: unauthorized")]
    Unauthorized,
    #[error("Invalid request")]
    InvalidRequest,
    #[error("Invalid payload")]
    InvalidPayload,
    #[error("Invalid headers")]
    InvalidHeaders,
    #[error("Encountered an unknown error fetching secret: {0}")]
    Other(String),
}

impl From<GetSecretError> for SecretResponse {
    fn from(e: GetSecretError) -> Self {
        SecretResponse {
            error: Some(e),
            ..Default::default()
        }
    }
}

impl From<SecretResponse> for Bytes {
    fn from(resp: SecretResponse) -> Self {
        let encoded = serde_json::to_vec(&resp).unwrap();
        Bytes::from(encoded)
    }
}
