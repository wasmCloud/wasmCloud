use async_nats::HeaderMap;
use nkeys::XKey;
use wasmcloud_secrets_types::{
    Secret, SecretRequest, SecretResponse, RESPONSE_XKEY, WASMCLOUD_HOST_XKEY,
};

/// Default API version of the secrets API implementation in wasmCloud
const DEFAULT_API_VERSION: &str = "v1alpha1";

/// Errors that can be returned during creation/use of a [`Client`]
#[derive(Debug, thiserror::Error)]
pub enum SecretClientError {
    #[error("failed to convert server xkey: {0}")]
    ConvertServerXkey(String),
    #[error("failed to parse server xkey: {0}")]
    ParseServerXkey(nkeys::error::Error),
    #[error("failed to fetch server xkey: {0}")]
    RequestServerXkey(async_nats::RequestError),
    #[error("invalid xkey: {0}")]
    InvalidXkey(nkeys::error::Error),
    #[error("failed to seal secret request: {0}")]
    SealSecretRequest(nkeys::error::Error),
    #[error("failed to send secret request: {0}")]
    SendSecretRequest(async_nats::RequestError),
    #[error("failed to serialize secret request: {0}")]
    SerializeSecretRequest(serde_json::error::Error),
    #[error("failed to parse xkey from server response: {0}")]
    ParseServerResponseXkey(nkeys::error::Error),
    #[error("failed to open secret response: {0}")]
    OpenSecretResponse(nkeys::error::Error),
    #[error("failed to deserialize secret response: {0}")]
    DeserializeSecretResponse(serde_json::error::Error),
    #[error("server error: {0}")]
    Server(String),
    #[error("missing secret: {0}")]
    MissingSecret(String),
}

/// Topic on which secrets can be requested.
///
/// This topic is normally a *prefix* from which other requests can be made,
/// for example retrieving a secret or a server xkey.
#[derive(Debug)]
struct SecretsTopic(String);

impl SecretsTopic {
    pub(crate) fn new(prefix: &str, backend: &str, api_version: Option<&str>) -> Self {
        let version = api_version.unwrap_or(DEFAULT_API_VERSION);
        Self(format!("{}.{}.{}", prefix, version, backend))
    }

    pub fn get(&self) -> String {
        format!("{}.{}", self.0, "get")
    }

    pub fn server_xkey(&self) -> String {
        format!("{}.{}", self.0, "server_xkey")
    }
}

/// NATS client that can be used to interact with secrets
#[derive(Debug)]
pub struct Client {
    /// NATS client to use to make requests
    client: async_nats::Client,
    /// Topic on which secrets-related requests can be made
    topic: SecretsTopic,
    /// Server Xkey (retrieved at client creation time)
    server_xkey: XKey,
}

impl Client {
    /// Create a new [`Client`], negotiating a server Xkey along the way
    pub async fn new(
        backend: &str,
        prefix: &str,
        nats_client: async_nats::Client,
    ) -> Result<Self, SecretClientError> {
        Self::new_with_version(backend, prefix, nats_client, None).await
    }

    /// Create a new [`Client`] with a specific API version
    pub async fn new_with_version(
        backend: &str,
        prefix: &str,
        nats_client: async_nats::Client,
        api_version: Option<&str>,
    ) -> Result<Self, SecretClientError> {
        let secrets_topic = SecretsTopic::new(prefix, backend, api_version);

        // Fetch server XKey so we can use it to encrypt requests to the server.
        let resp = nats_client
            .request(secrets_topic.server_xkey(), "".into())
            .await
            .map_err(SecretClientError::RequestServerXkey)?;
        let s = std::str::from_utf8(&resp.payload)
            .map_err(|e| SecretClientError::ConvertServerXkey(e.to_string()))?;
        let server_xkey = XKey::from_public_key(s).map_err(SecretClientError::ParseServerXkey)?;

        Ok(Self {
            client: nats_client,
            topic: secrets_topic,
            server_xkey,
        })
    }

    /// Retrieve a given secret
    pub async fn get(
        &self,
        secret_request: SecretRequest,
        request_xkey: XKey,
    ) -> Result<Secret, SecretClientError> {
        // Ensure the provided xkey can be used for sealing
        if let Err(e) = request_xkey.seed() {
            return Err(SecretClientError::InvalidXkey(e));
        }

        let request = serde_json::to_string(&secret_request)
            .map_err(SecretClientError::SerializeSecretRequest)?;
        let encrypted_request = request_xkey
            .seal(request.as_bytes(), &self.server_xkey)
            .map_err(SecretClientError::SealSecretRequest)?;

        let response = self
            .client
            .request_with_headers(
                self.topic.get(),
                self.request_headers(request_xkey.public_key()),
                encrypted_request.into(),
            )
            .await
            .map_err(SecretClientError::SendSecretRequest)?;

        let headers = response.headers.unwrap_or_default();
        // Check whether we got a 'Server-Response-Key' header, signifying an
        // encrypted payload. Otherwise assume that we received an error and
        // we handle that instead.
        let Some(response_xkey_header) = headers.get(RESPONSE_XKEY) else {
            let sr: SecretResponse = serde_json::from_slice(&response.payload)
                .map_err(SecretClientError::DeserializeSecretResponse)?;

            if let Some(error) = sr.error {
                return Err(SecretClientError::Server(error.to_string()));
            }
            return Err(SecretClientError::Server(
                "unhandled server error (the server errored without explanation)".into(),
            ));
        };

        let response_xkey = XKey::from_public_key(response_xkey_header.as_str())
            .map_err(SecretClientError::ParseServerResponseXkey)?;

        let decrypted = request_xkey
            .open(&response.payload, &response_xkey)
            .map_err(SecretClientError::OpenSecretResponse)?;

        let sr: SecretResponse = serde_json::from_slice(&decrypted)
            .map_err(SecretClientError::DeserializeSecretResponse)?;

        sr.secret.ok_or_else(|| {
            SecretClientError::MissingSecret(format!(
                "no secret found with name [{}]",
                secret_request.key
            ))
        })
    }

    /// Generate NATS request headers
    fn request_headers(&self, pubkey: String) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(WASMCLOUD_HOST_XKEY, pubkey.as_str());
        headers
    }
}
