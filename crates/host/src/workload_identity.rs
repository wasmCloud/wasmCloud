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
    pub fn new() -> Result<Self> {
        // TODO(joonas): Should we fall back to a default value here instead of requiring the endpoint to be explicitly set?
        let endpoint = get_default_socket_path()
            .context("failed to load SPIFFE workload endpoint from SPIFFE_ENDPOINT_SOCKET")?;

        if let Err(err) = validate_socket_path(&endpoint) {
            bail!(
                "failed to validate SPIFFE workload endpoint from SPIFFE_ENDPOINT_SOCKET: {:?}",
                err
            );
        }

        let endpoint_uri = url::Url::parse(&endpoint)?;

        // If we're given a path to socket, check that the socket exists.
        if endpoint_uri.scheme() == UNIX_SCHEME && !Path::new(endpoint_uri.path()).exists() {
            bail!(
                "provided SPIFFE workload endpoint does not exist: {}",
                endpoint,
            );
        }

        // TODO(joonas): figure out better naming here. maybe this should be interpolated from a trust domain?
        // This needs to follow format like: "spiffe://{spiffe_trust_domain}/{nats_auth_callout_service}"
        let auth_service_audience = match std::env::var(AUTH_SERVICE_AUDIENCE_ENV) {
            Ok(value) => value,
            Err(_) => {
                bail!("workload identity auth callout audience environment variable is missing")
            }
        };

        Ok(Self {
            spiffe_endpoint: endpoint,
            auth_service_audience,
        })
    }
}
