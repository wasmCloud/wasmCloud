use std::sync::Arc;

use anyhow::Context as _;
use tracing::instrument;
use wasmcloud_tracing::context::TraceContextInjector;

use crate::wasmbus::{injector_to_headers, providers::ProviderManager};

/// NATS implementation of the wasmCloud [crate::wasmbus::provider::ProviderManager] extension trait
pub struct NatsProviderManager {
    pub(crate) nats_client: Arc<async_nats::Client>,
    pub(crate) lattice: String,
}

impl NatsProviderManager {
    /// Create a new NATS provider manager
    pub fn new(nats_client: Arc<async_nats::Client>, lattice: String) -> Self {
        Self {
            nats_client,
            lattice,
        }
    }
}

#[async_trait::async_trait]
impl ProviderManager for NatsProviderManager {
    #[instrument(level = "debug", skip(self))]
    async fn put_link(
        &self,
        link: &wasmcloud_core::InterfaceLinkDefinition,
        target: &str,
    ) -> anyhow::Result<()> {
        let lattice = &self.lattice;
        let payload =
            serde_json::to_vec(link).context("failed to serialize provider link definition")?;
        self.nats_client
            .publish_with_headers(
                format!("wasmbus.rpc.{lattice}.{target}.linkdefs.put"),
                injector_to_headers(&TraceContextInjector::default_with_span()),
                payload.into(),
            )
            .await
            .context("failed to publish provider link definition")?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self))]
    async fn delete_link(
        &self,
        link: &wasmcloud_core::InterfaceLinkDefinition,
        target: &str,
    ) -> anyhow::Result<()> {
        let lattice = &self.lattice;
        let payload =
            serde_json::to_vec(link).context("failed to serialize provider link definition")?;
        self.nats_client
            .publish_with_headers(
                format!("wasmbus.rpc.{lattice}.{target}.linkdefs.del"),
                injector_to_headers(&TraceContextInjector::default_with_span()),
                payload.into(),
            )
            .await
            .context("failed to publish provider link definition")?;
        Ok(())
    }
}
