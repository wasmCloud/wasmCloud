use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// wasmCloud Host configuration
pub struct Host {
    /// URL to connect to
    pub url: Url,
    /// Optional host seed
    pub host_seed: Option<String>,
    /// Optional cluster seed
    pub cluster_seed: Option<String>,
}

impl Default for Host {
    fn default() -> Self {
        Self {
            url: Url::parse("nats://localhost:4222").expect("failed to parse URL"),
            host_seed: None,
            cluster_seed: None,
        }
    }
}
