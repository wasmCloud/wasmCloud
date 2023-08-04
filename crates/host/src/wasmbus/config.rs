use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// wasmCloud Host configuration
pub struct Host {
    /// NATS URL to connect to for control interface connection
    pub ctl_nats_url: Url,
    /// The lattice the host belongs to
    pub lattice_prefix: String,
    /// The seed key (a printable 256-bit Ed25519 private key) used by this host to generate its public key
    pub host_seed: Option<String>,
    /// The seed key (a printable 256-bit Ed25519 private key) used by this host to sign all invocations
    pub cluster_seed: Option<String>,
}

impl Default for Host {
    fn default() -> Self {
        Self {
            ctl_nats_url: Url::parse("nats://localhost:4222")
                .expect("failed to parse control NATS URL"),
            lattice_prefix: "default".to_string(),
            host_seed: None,
            cluster_seed: None,
        }
    }
}
