use std::sync::Arc;

use crate::bindings::wrpc::extension::types::InterfaceConfig;
use crate::WasmbusHost;
use crate::{
    bindings::wrpc::extension::{self},
    wasmbus::{injector_to_headers, providers::ProviderManager},
};

use tracing::instrument;
use wasmcloud_tracing::context::TraceContextInjector;

// todo(luk3ark) - delete
// /// NATS implementation of the wasmCloud [crate::wasmbus::providers::ProviderManager] extension trait
// pub struct NatsProviderManager {
//     pub(crate) nats_client: Arc<async_nats::Client>,
//     pub(crate) lattice: String,
// }

// impl NatsProviderManager {
//     /// Create a new NATS provider manager
//     pub fn new(nats_client: Arc<async_nats::Client>, lattice: String) -> Self {
//         Self {
//             nats_client,
//             lattice,
//         }
//     }
// }

// impl NatsProviderManager {
//     #[instrument(level = "debug", skip(self))]
//     pub(crate) async fn put_link(
//         &self,
//         link: &wasmcloud_core::InterfaceLinkDefinition,
//         target: &str,
//     ) -> anyhow::Result<()> {
//         let lattice = &self.lattice;
//         let payload =
//             serde_json::to_vec(link).context("failed to serialize provider link definition")?;
//         self.nats_client
//             .publish_with_headers(
//                 link_put_subject(lattice, target),
//                 injector_to_headers(&TraceContextInjector::default_with_span()),
//                 payload.into(),
//             )
//             .await
//             .context("failed to publish provider link definition")?;
//         Ok(())
//     }

//     #[instrument(level = "debug", skip(self))]
//     pub(crate) async fn delete_link(
//         &self,
//         link: &wasmcloud_core::InterfaceLinkDefinition,
//         target: &str,
//     ) -> anyhow::Result<()> {
//         let lattice = &self.lattice;
//         let payload =
//             serde_json::to_vec(link).context("failed to serialize provider link definition")?;
//         self.nats_client
//             .publish_with_headers(
//                 link_del_subject(lattice, target),
//                 injector_to_headers(&TraceContextInjector::default_with_span()),
//                 payload.into(),
//             )
//             .await
//             .context("failed to publish provider link definition")?;
//         Ok(())
//     }
// }

/// WRPC implementation of the wasmCloud [crate::wasmbus::providers::ProviderManager] extension trait
pub struct WrpcProviderManager {
    // NOTE(luk3ark): Might be more efficient if we track a map of wrpc clients prebuilt for each provider
    // rather than building on the fly
    pub(crate) nats_client: Arc<async_nats::Client>,
    pub(crate) lattice: String,
    pub(crate) host_id: String,
}

impl WrpcProviderManager {
    /// Create a new NATS provider manager
    pub fn new(nats_client: Arc<async_nats::Client>, lattice: String, host_id: String) -> Self {
        Self {
            nats_client,
            lattice,
            host_id,
        }
    }
}

#[async_trait::async_trait]
impl ProviderManager for WrpcProviderManager {
    async fn put_interface_import_config(
        &self,
        provider_id: &str,
        target_id: &str,
        link_name: &str,
        config: &InterfaceConfig,
    ) -> anyhow::Result<()> {
        let wrpc_client = self.produce_extension_wrpc_client(provider_id).await?;
        let mut headers = injector_to_headers(&TraceContextInjector::default_with_span());
        headers.insert("source-id", WasmbusHost::host_source_id());

        match extension::configurable::update_interface_import_config(
            &wrpc_client,
            Some(headers),
            target_id,
            link_name,
            config,
        )
        .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(app_error)) => {
                // Transport succeeded but logic failed
                Err(anyhow::anyhow!(
                    "Provider failed to update interface import config: {}",
                    app_error
                ))
            }
            Err(transport_error) => {
                // Transport/communication failed
                Err(transport_error.context(
                    "Failed to communicate with provider for interface import config update",
                ))
            }
        }
    }

    #[instrument(level = "debug", skip(self))]
    async fn put_interface_export_config(
        &self,
        provider_id: &str,
        source_id: &str,
        link_name: &str,
        config: &InterfaceConfig,
    ) -> anyhow::Result<()> {
        let wrpc_client = self.produce_extension_wrpc_client(provider_id).await?;
        let mut headers = injector_to_headers(&TraceContextInjector::default_with_span());
        headers.insert("source-id", WasmbusHost::host_source_id());

        match extension::configurable::update_interface_export_config(
            &wrpc_client,
            Some(headers),
            source_id,
            link_name,
            config,
        )
        .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(app_error)) => {
                // Transport succeeded but logic failed
                Err(anyhow::anyhow!(
                    "Provider failed to update interface export config: {}",
                    app_error
                ))
            }
            Err(transport_error) => {
                // Transport/communication failed
                Err(transport_error.context(
                    "Failed to communicate with provider for interface export config update",
                ))
            }
        }
    }

    #[instrument(level = "debug", skip(self))]
    async fn delete_interface_import_config(
        &self,
        provider_id: &str,
        target_id: &str,
        link_name: &str,
    ) -> anyhow::Result<()> {
        let wrpc_client = self.produce_extension_wrpc_client(provider_id).await?;
        let mut headers = injector_to_headers(&TraceContextInjector::default_with_span());
        headers.insert("source-id", WasmbusHost::host_source_id());

        match extension::configurable::delete_interface_import_config(
            &wrpc_client,
            Some(headers),
            target_id,
            link_name,
        )
        .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(app_error)) => {
                // Transport succeeded but logic failed
                Err(anyhow::anyhow!(
                    "Provider failed to delete interface import config: {}",
                    app_error
                ))
            }
            Err(transport_error) => {
                // Transport/communication failed
                Err(transport_error.context(
                    "Failed to communicate with provider for interface import config deletion",
                ))
            }
        }
    }

    #[instrument(level = "debug", skip(self))]
    async fn delete_interface_export_config(
        &self,
        provider_id: &str,
        source_id: &str,
        link_name: &str,
    ) -> anyhow::Result<()> {
        let wrpc_client = self.produce_extension_wrpc_client(provider_id).await?;
        let mut headers = injector_to_headers(&TraceContextInjector::default_with_span());
        headers.insert("source-id", WasmbusHost::host_source_id());

        match extension::configurable::delete_interface_export_config(
            &wrpc_client,
            Some(headers),
            source_id,
            link_name,
        )
        .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(app_error)) => {
                // Transport succeeded but logic failed
                Err(anyhow::anyhow!(
                    "Provider failed to delete interface export config: {}",
                    app_error
                ))
            }
            Err(transport_error) => {
                // Transport/communication failed
                Err(transport_error.context(
                    "Failed to communicate with provider for interface export config deletion",
                ))
            }
        }
    }

    /// Produce a wrpc client for extension interfaces (manageable, configurable).
    /// Extension interfaces are served on a host-specific ctl subject.
    ///
    /// Subject format: `wasmbus.ctl.v1.{lattice}.extension.{provider_id}.{host_id}`
    async fn produce_extension_wrpc_client(
        &self,
        provider_id: &str,
    ) -> anyhow::Result<wrpc_transport_nats::Client> {
        let lattice = &self.lattice;
        let host_id = &self.host_id;
        let prefix = format!("wasmbus.ctl.v1.{lattice}.extension.{provider_id}.{host_id}");
        wrpc_transport_nats::Client::new(
            Arc::clone(&self.nats_client),
            prefix.clone(),
            Some(prefix.into()),
        )
        .await
    }

    /// Request a provider to shutdown
    #[instrument(level = "debug", skip(self))]
    async fn shutdown_provider(&self, provider_id: &str) -> anyhow::Result<()> {
        let wrpc_client = self.produce_extension_wrpc_client(provider_id).await?;
        let mut headers = injector_to_headers(&TraceContextInjector::default_with_span());
        headers.insert("source-id", WasmbusHost::host_source_id());

        match extension::manageable::shutdown(&wrpc_client, Some(headers)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(app_error)) => Err(anyhow::anyhow!(
                "Provider failed to shutdown: {}",
                app_error
            )),
            Err(transport_error) => {
                Err(transport_error.context("Failed to communicate with provider for shutdown"))
            }
        }
    }
}
