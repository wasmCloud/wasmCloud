use anyhow::{bail, Context, Result};
use spiffe::endpoint::{get_default_socket_path, validate_socket_path};
use std::path::Path;

const UNIX_SCHEME: &str = "unix";
// TODO(joonas): Figure out better naming here
const AUTH_SERVICE_AUDIENCE_ENV: &str = "WASMCLOUD_WORKLOAD_IDENTITY_AUTH_SERVICE_AUDIENCE";

#[derive(Clone, Default, Debug)]
pub struct WorkloadIdentityConfig {
    pub spiffe_endpoint: String,
    pub auth_service_audience: String,
}

impl WorkloadIdentityConfig {
    /// Fetch workload identity configuration from environment variables
    pub fn from_env() -> Result<Self> {
        // TODO(joonas): Should we fall back to a default value here instead of requiring the endpoint to be explicitly set?
        let endpoint = get_default_socket_path()
            .context("failed to load SPIFFE workload endpoint from SPIFFE_ENDPOINT_SOCKET")?;

        validate_socket_path(&spiffe_endpoint).context("failed to validate SPIFFE workload endpoint from SPIFFE_ENDPOINT_SOCKET")?;

        let endpoint_uri = url::Url::parse(&endpoint)?;

        // If we're given a path to socket, check that the socket exists.
        if endpoint_uri.scheme() == UNIX_SCHEME && !Path::new(endpoint_uri.path()).exists() {
            bail!("provided SPIFFE workload endpoint does not exist: {endpoint}");
        }

        // TODO(joonas): figure out better naming here. maybe this should be interpolated from a trust domain?
        // This needs to follow format like: "spiffe://{spiffe_trust_domain}/{nats_auth_callout_service}"
        let auth_service_audience = std::env::var(AUTH_SERVICE_AUDIENCE_ENV).context("workload identity auth callout audience environment variable is missing")?;

        Ok(Self {
            spiffe_endpoint,
            auth_service_audience,
        })
    }
}
