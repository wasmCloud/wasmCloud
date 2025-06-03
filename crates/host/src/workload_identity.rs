use std::sync::Arc;

#[cfg(target_family = "windows")]
use anyhow::{bail, Result};
#[cfg(unix)]
use anyhow::{Context as _, Result};
use nkeys::KeyPair;

// TODO(joonas): Figure out better naming here
const AUTH_SERVICE_AUDIENCE_ENV: &str = "WASMCLOUD_WORKLOAD_IDENTITY_AUTH_SERVICE_AUDIENCE";

/// WorkloadIdentityConfig is used by the experimental workload-identity feature
#[derive(Clone, Default, Debug)]
pub struct WorkloadIdentityConfig {
    /// auth_service_audience represents the value expected by the Auth Callout Service,
    /// typically this should look something like "spiffe://wasmcloud.dev/auth-callout"
    pub auth_service_audience: String,
}

impl WorkloadIdentityConfig {
    /// Fetch workload identity configuration from environment variables
    #[cfg(unix)]
    pub fn from_env() -> Result<Self> {
        // TODO(joonas): figure out better naming here. maybe this should be interpolated from a trust domain?
        // This needs to follow format like: "spiffe://{spiffe_trust_domain}/{nats_auth_callout_service}"
        let auth_service_audience = std::env::var(AUTH_SERVICE_AUDIENCE_ENV)
            .context("workload identity auth callout audience environment variable is missing")?;

        Ok(Self {
            auth_service_audience,
        })
    }

    #[cfg(target_family = "windows")]
    pub fn from_env() -> Result<Self> {
        anyhow::bail!("workload identity is not supported on Windows")
    }
}

#[cfg(unix)]
pub(crate) async fn setup_workload_identity_nats_connect_options(
    jwt: Option<&String>,
    key: Option<Arc<KeyPair>>,
    wid_cfg: WorkloadIdentityConfig,
) -> anyhow::Result<async_nats::ConnectOptions> {
    let wid_cfg = Arc::new(wid_cfg);
    let jwt = jwt.map(String::to_string).map(Arc::new);
    let key = key.clone();

    // Return an auth callback that'll get called any time the
    // NATS connection needs to be (re-)established. This is
    // necessary to ensure that we always provide a recently
    // issued JWT-SVID.
    Ok(
        async_nats::ConnectOptions::with_auth_callback(move |nonce| {
            let key = key.clone();
            let jwt = jwt.clone();
            let wid_cfg = wid_cfg.clone();

            let fetch_svid_handle = tokio::spawn(async move {
                let mut client = spiffe::WorkloadApiClient::default()
                    .await
                    .map_err(async_nats::AuthError::new)?;
                client
                    .fetch_jwt_svid(&[wid_cfg.auth_service_audience.as_str()], None)
                    .await
                    .map_err(async_nats::AuthError::new)
            });

            async move {
                let svid = fetch_svid_handle
                    .await
                    .map_err(async_nats::AuthError::new)?
                    .map_err(async_nats::AuthError::new)?;

                let mut auth = async_nats::Auth::new();
                if let Some(key) = key {
                    let signature = key.sign(&nonce).map_err(async_nats::AuthError::new)?;
                    auth.signature = Some(signature);
                }
                if let Some(jwt) = jwt {
                    auth.jwt = Some(jwt.to_string());
                }
                auth.token = Some(svid.token().into());
                Ok(auth)
            }
        })
        .name("wasmbus"),
    )
}

#[cfg(target_family = "windows")]
pub(crate) async fn setup_workload_identity_nats_connect_options(
    jwt: Option<&String>,
    key: Option<Arc<KeyPair>>,
    wid_cfg: WorkloadIdentityConfig,
) -> anyhow::Result<async_nats::ConnectOptions> {
    bail!("workload identity is not supported on Windows")
}
