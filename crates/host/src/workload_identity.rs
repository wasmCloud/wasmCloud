use anyhow::{Context as _, Result};

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
    pub fn from_env() -> Result<Self> {
        // TODO(joonas): figure out better naming here. maybe this should be interpolated from a trust domain?
        // This needs to follow format like: "spiffe://{spiffe_trust_domain}/{nats_auth_callout_service}"
        let auth_service_audience = std::env::var(AUTH_SERVICE_AUDIENCE_ENV)
            .context("workload identity auth callout audience environment variable is missing")?;

        Ok(Self {
            auth_service_audience,
        })
    }
}
