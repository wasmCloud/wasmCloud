use async_nats::HeaderMap;
use nkeys::XKey;
use wasmcloud_secrets_types::{
    Secret, SecretRequest, SecretResponse, RESPONSE_XKEY, WASMCLOUD_HOST_XKEY,
};

const DEFAULT_API_VERSION: &str = "v0";

#[derive(Debug)]
pub enum SecretClientError {
    ConvertServerXkeyError,
    ParseServerXkeyError,
    RequestServerXkeyError,
    InvalidXkeyError,
    SealSecretRequestError,
    SendSecretRequestError,
    SerializeSecretRequestError,
    ParseServerResponseXkeyError,
    OpenSecretResponseError,
    DeserializeSecretResponseError,
    ServerError(String),
    MissingSecretError,
}

impl std::fmt::Display for SecretClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                SecretClientError::ConvertServerXkeyError => "Convert Server Xkey Error",
                SecretClientError::ParseServerXkeyError => "Parse Server Xkey Error",
                SecretClientError::RequestServerXkeyError => "Request Server Xkey Error",
                SecretClientError::InvalidXkeyError => "Invalid Xkey Error",
                SecretClientError::SealSecretRequestError => "Seal SecretRequest Error",
                SecretClientError::SendSecretRequestError => "Send SecretRequest Error",
                SecretClientError::SerializeSecretRequestError => "Serialize SecretRequest Error",
                SecretClientError::ParseServerResponseXkeyError => "Parse Server Response Error",
                SecretClientError::OpenSecretResponseError => "Open SecretResponse Error",
                SecretClientError::DeserializeSecretResponseError =>
                    "Deserialize SecretResponse Error",
                SecretClientError::ServerError(_) => "Server Error",
                SecretClientError::MissingSecretError => "Missing Secret Errror",
            }
        )
    }
}

impl std::error::Error for SecretClientError {}

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

#[derive(Debug)]
pub struct Client {
    client: async_nats::Client,
    topic: SecretsTopic,
    server_xkey: XKey,
}

impl Client {
    pub async fn new(
        backend: &str,
        prefix: &str,
        nats_client: async_nats::Client,
    ) -> Result<Self, SecretClientError> {
        Self::new_with_version(backend, prefix, nats_client, None).await
    }

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
            .map_err(|e| {
                println!("got error: {e:?}");
                SecretClientError::RequestServerXkeyError
            })?;
        let s = std::str::from_utf8(&resp.payload)
            .map_err(|_| SecretClientError::ConvertServerXkeyError)?;
        let server_xkey =
            XKey::from_public_key(s).map_err(|_| SecretClientError::ParseServerXkeyError)?;

        Ok(Self {
            client: nats_client,
            topic: secrets_topic,
            server_xkey,
        })
    }

    pub async fn get(
        &self,
        secret_request: SecretRequest,
        request_xkey: XKey,
    ) -> Result<Secret, SecretClientError> {
        // Ensure the provided xkey can be used for sealing
        if request_xkey.seed().is_err() {
            return Err(SecretClientError::InvalidXkeyError);
        }

        let request = serde_json::to_string(&secret_request)
            .map_err(|_| SecretClientError::SerializeSecretRequestError)?;
        let encrypted_request = request_xkey
            .seal(request.as_bytes(), &self.server_xkey)
            .map_err(|_| SecretClientError::SealSecretRequestError)?;

        let response = self
            .client
            .request_with_headers(
                self.topic.get(),
                self.request_headers(request_xkey.public_key()),
                encrypted_request.into(),
            )
            .await
            .map_err(|_| SecretClientError::SendSecretRequestError)?;

        let headers = response.headers.unwrap_or_default();
        // Check whether we got a 'Server-Response-Key' header, signifying an
        // encrypted payload. Otherwise assume that we received an error and
        // we handle that instead.
        let Some(response_xkey_header) = headers.get(RESPONSE_XKEY) else {
            let sr: SecretResponse = serde_json::from_slice(&response.payload)
                .map_err(|_| SecretClientError::DeserializeSecretResponseError)?;

            if let Some(error) = sr.error {
                return Err(SecretClientError::ServerError(error.to_string()));
            }
            // TODO: better error message
            return Err(SecretClientError::ServerError("unknown error".to_string()));
        };

        let response_xkey = XKey::from_public_key(response_xkey_header.as_str())
            .map_err(|_| SecretClientError::ParseServerResponseXkeyError)?;

        let decrypted = request_xkey
            .open(&response.payload, &response_xkey)
            .map_err(|_| SecretClientError::OpenSecretResponseError)?;

        let sr: SecretResponse = serde_json::from_slice(&decrypted)
            .map_err(|_| SecretClientError::DeserializeSecretResponseError)?;

        if let Some(secret) = sr.secret {
            Ok(secret)
        } else {
            Err(SecretClientError::MissingSecretError)
        }
    }

    fn request_headers(&self, pubkey: String) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(WASMCLOUD_HOST_XKEY, pubkey.as_str());
        headers
    }
}
