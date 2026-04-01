#![doc = include_str!("../README.md")]

pub mod engine;
pub mod host;
pub mod observability;
pub mod plugin;
pub mod sockets;
pub mod types;
pub mod wit;

#[cfg(feature = "oci")]
pub mod oci;

#[cfg(feature = "washlet")]
pub mod washlet;

// Re-export wasmtime for convenience
pub use wasmtime;

/// Install the process-level rustls [`CryptoProvider`](rustls::crypto::CryptoProvider).
///
/// wasmCloud standardizes on `aws-lc-rs`. Safe to call any number of times from
/// any number of threads. The install happens at most once per process. If
/// another crate already installed a provider we leave it in place.
///
/// Called automatically by [`host::http::HttpServer::new`] and
/// [`host::http::HttpServer::new_with_tls`]; call it directly from binaries
/// that touch TLS before constructing an `HttpServer` (for example, CLIs that
/// connect to a TLS NATS cluster during startup).
pub fn init_crypto() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        if rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .is_err()
        {
            tracing::warn!(
                "a rustls CryptoProvider was already installed; \
                 wasmCloud standardizes on aws-lc-rs — check dependencies if this is unexpected"
            );
        }
    });
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::host::http::{HttpServer, WasiOutgoingHandler};
    use crate::plugin::wasi_config::DynamicConfig;
    use crate::{
        host::HostApi,
        types::{Workload, WorkloadStartRequest},
    };

    use super::{engine::Engine, host::HostBuilder};

    #[tokio::test]
    async fn can_run_engine() -> anyhow::Result<()> {
        let engine = Engine::builder().build()?;
        let http_handler = crate::host::http::DevRouter::default();
        let http_plugin =
            HttpServer::new(http_handler, WasiOutgoingHandler, "127.0.0.1:0".parse()?).await?;
        let wasi_config_plugin = DynamicConfig::default();

        let host = HostBuilder::new()
            .with_engine(engine)
            .with_http_handler(Arc::new(http_plugin))
            .with_plugin(Arc::new(wasi_config_plugin))?
            .build()?;

        let host = host.start().await?;

        let req = WorkloadStartRequest {
            workload_id: uuid::Uuid::new_v4().to_string(),
            workload: Workload {
                namespace: "test".to_string(),
                name: "test-workload".to_string(),
                annotations: HashMap::new(),
                service: None,
                components: vec![],
                host_interfaces: vec![],
                volumes: vec![],
            },
        };
        let _res = host.workload_start(req).await?;

        Ok(())
    }
}
