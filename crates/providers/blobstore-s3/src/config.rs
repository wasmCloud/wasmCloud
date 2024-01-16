//! Configuration for blobstore-s3 capability provider
//!
//! See README.md for configuration options using environment variables, aws credentials files,
//! and EC2 IAM authorizations.
//!
use std::collections::HashMap;
use std::env;

use aws_config::SdkConfig;
use aws_sdk_s3::config::{Region, SharedCredentialsProvider};
use base64::Engine;

use wasmcloud_provider_wit_bindgen::deps::{
    serde::Deserialize,
    serde_json,
    wasmcloud_provider_sdk::error::{ProviderInvocationError, ProviderInvocationResult},
};

const DEFAULT_STS_SESSION: &str = "blobstore_s3_provider";

/// Configuration for connecting to S3.
///
#[derive(Clone, Default, Deserialize)]
#[serde(crate = "wasmcloud_provider_wit_bindgen::deps::serde")]
pub struct StorageConfig {
    /// AWS_ACCESS_KEY_ID, can be specified from environment
    pub access_key_id: Option<String>,
    /// AWS_SECRET_ACCESS_KEY, can be in environment
    pub secret_access_key: Option<String>,
    /// Session Token
    pub session_token: Option<String>,
    /// AWS_REGION
    pub region: Option<String>,
    /// override default max_attempts (3) for retries
    pub max_attempts: Option<u32>,
    /// optional configuration for STS Assume Role
    pub sts_config: Option<StsAssumeRoleConfig>,
    /// optional override for the AWS endpoint
    pub endpoint: Option<String>,
    /// optional map of bucket aliases to names
    #[serde(default)]
    pub aliases: HashMap<String, String>,
    /// optional max chunk size
    pub max_chunk_size_bytes: Option<usize>,
    /// optional use WebPKI roots for TLS rather than native (the default for aws_sdk_s3)
    pub tls_use_webpki_roots: Option<bool>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(crate = "wasmcloud_provider_wit_bindgen::deps::serde")]
pub struct StsAssumeRoleConfig {
    /// Role to assume (AWS_ASSUME_ROLE_ARN)
    /// Should be in the form "arn:aws:iam::123456789012:role/example"
    pub role: String,
    /// AWS Region for using sts, not for S3
    pub region: Option<String>,
    /// Optional Session name
    pub session: Option<String>,
    /// Optional external id
    pub external_id: Option<String>,
}

impl StorageConfig {
    /// initialize from linkdef values
    pub fn from_values(
        values: &HashMap<String, String>,
    ) -> ProviderInvocationResult<StorageConfig> {
        let mut config = if let Some(config_b64) = values.get("config_b64") {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(config_b64.as_bytes())
                .map_err(|e| {
                    ProviderInvocationError::Provider(format!("invalid base64 encoding: {e}",))
                })?;
            serde_json::from_slice::<StorageConfig>(&bytes).map_err(|e| {
                ProviderInvocationError::Provider(format!("corrupt config_b64: {e}"))
            })?
        } else if let Some(config) = values.get("config_json") {
            serde_json::from_str::<StorageConfig>(config).map_err(|e| {
                ProviderInvocationError::Provider(format!("corrupt config_json: {e}"))
            })?
        } else {
            StorageConfig::default()
        };
        // load environment variables from file
        if let Some(env_file) = values.get("env") {
            let data = std::fs::read_to_string(env_file).map_err(|e| {
                ProviderInvocationError::Provider(format!("reading env file '{env_file}': {e}",))
            })?;
            simple_env_load::parse_and_set(&data, |k, v| std::env::set_var(k, v));
        }

        if let Ok(arn) = env::var("AWS_ROLE_ARN") {
            let mut sts_config = config.sts_config.unwrap_or_default();
            sts_config.role = arn;
            if let Ok(region) = env::var("AWS_ROLE_REGION") {
                sts_config.region = Some(region);
            }
            if let Ok(session) = env::var("AWS_ROLE_SESSION_NAME") {
                sts_config.session = Some(session);
            }
            if let Ok(external_id) = env::var("AWS_ROLE_EXTERNAL_ID") {
                sts_config.external_id = Some(external_id);
            }
            config.sts_config = Some(sts_config);
        }

        if let Ok(endpoint) = env::var("AWS_ENDPOINT") {
            config.endpoint = Some(endpoint)
        }

        // aliases are added from linkdefs in StorageClient::new()
        Ok(config)
    }

    pub async fn configure_aws(self) -> SdkConfig {
        use aws_config::{
            default_provider::{credentials::DefaultCredentialsChain, region::DefaultRegionChain},
            sts::AssumeRoleProvider,
        };

        let region = match self.region {
            Some(region) => Some(Region::new(region)),
            _ => DefaultRegionChain::builder().build().region().await,
        };

        // use static credentials or defaults from environment
        let mut cred_provider = match (self.access_key_id, self.secret_access_key) {
            (Some(access_key_id), Some(secret_access_key)) => {
                SharedCredentialsProvider::new(aws_sdk_s3::config::Credentials::new(
                    access_key_id,
                    secret_access_key,
                    self.session_token.clone(),
                    None,
                    "static",
                ))
            }
            _ => SharedCredentialsProvider::new(
                DefaultCredentialsChain::builder()
                    .region(region.clone())
                    .build()
                    .await,
            ),
        };

        if let Some(sts_config) = self.sts_config {
            let mut role = AssumeRoleProvider::builder(sts_config.role).session_name(
                sts_config
                    .session
                    .unwrap_or_else(|| DEFAULT_STS_SESSION.to_string()),
            );
            if let Some(region) = sts_config.region {
                role = role.region(Region::new(region));
            }
            if let Some(external_id) = sts_config.external_id {
                role = role.external_id(external_id);
            }
            cred_provider = SharedCredentialsProvider::new(role.build().await);
        }

        let mut retry_config = aws_config::retry::RetryConfig::standard();
        if let Some(max_attempts) = self.max_attempts {
            retry_config = retry_config.with_max_attempts(max_attempts);
        }
        let mut loader = aws_config::defaults(aws_config::BehaviorVersion::v2023_11_09())
            .region(region)
            .credentials_provider(cred_provider)
            .retry_config(retry_config);

        if let Some(endpoint) = self.endpoint {
            loader = loader.endpoint_url(endpoint);
        }

        loader.load().await
    }
}
