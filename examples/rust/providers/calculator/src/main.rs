use crate::bindings::exports::wasmcloud::calculator::calculator;
use crate::bindings::exports::wrpc::extension;
use crate::bindings::wasmcloud::calculator::types::OperationRequest;
use crate::config::CalculatorConfig;
use crate::config::InterfaceSpecificConfig;
use crate::extension::configurable::Config;
use crate::extension::configurable::ConfigId;
use crate::extension::configurable::InterfaceConfig;
use crate::extension::manageable::BindRequest;
use crate::extension::manageable::BindResponse;
use crate::extension::manageable::HealthCheckResponse;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use wasmcloud_provider_sdk::InterfaceConfigHandler;

mod bindings {
    wit_bindgen_wrpc::generate!({
        world: "native-provider-calculator",
        generate_all
    });
}

mod config;

#[derive(Clone, Debug, Default)]
pub struct NativeCalculator {
    base_config: Arc<RwLock<CalculatorConfig>>,
    interface_config_handler: Arc<InterfaceConfigHandler<InterfaceSpecificConfig>>,
}

impl extension::manageable::Handler<Option<async_nats::HeaderMap>> for NativeCalculator {
    async fn bind(
        &self,
        _cx: Option<async_nats::HeaderMap>,
        _req: BindRequest,
    ) -> anyhow::Result<Result<BindResponse, String>> {
        Ok(Ok(BindResponse {
            identity_token: None,
            pubkey: None,
        }))
    }

    async fn health_request(
        &self,
        _cx: Option<async_nats::HeaderMap>,
    ) -> anyhow::Result<Result<HealthCheckResponse, String>> {
        Ok(Ok(HealthCheckResponse {
            healthy: true,
            message: Some("OK".to_string()),
        }))
    }
}

impl extension::configurable::Handler<Option<async_nats::HeaderMap>> for NativeCalculator {
    async fn update_base_config(
        &self,
        _cx: Option<async_nats::HeaderMap>,
        config: Config,
    ) -> anyhow::Result<Result<(), String>> {
        let config: HashMap<String, String> = config.into_iter().collect();

        match CalculatorConfig::from_config(config) {
            Ok(calc_config) => {
                *self.base_config.write().await = calc_config;
                return Ok(Ok(()));
            }
            Err(error_msg) => {
                return Ok(Err(error_msg));
            }
        }
    }

    async fn update_interface_export_config(
        &self,
        _cx: Option<async_nats::HeaderMap>,
        id: ConfigId,
        config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let config: InterfaceSpecificConfig =
            config.try_into().map_err(|e: String| anyhow::anyhow!(e))?;

        self.interface_config_handler
            .set_export_config(id, config)
            .await;

        Ok(Ok(()))
    }

    async fn update_interface_import_config(
        &self,
        _cx: Option<async_nats::HeaderMap>,
        id: ConfigId,
        config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let config: InterfaceSpecificConfig =
            config.try_into().map_err(|e: String| anyhow::anyhow!(e))?;

        self.interface_config_handler
            .set_import_config(id, config)
            .await;

        Ok(Ok(()))
    }

    async fn delete_interface_import_config(
        &self,
        _cx: Option<async_nats::HeaderMap>,
        id: ConfigId,
    ) -> anyhow::Result<Result<(), String>> {
        let _removed = self
            .interface_config_handler
            .remove_import_config(&id)
            .await;

        Ok(Ok(()))
    }

    async fn delete_interface_export_config(
        &self,
        _cx: Option<async_nats::HeaderMap>,
        id: ConfigId,
    ) -> anyhow::Result<Result<(), String>> {
        let _removed = self
            .interface_config_handler
            .remove_export_config(&id)
            .await;

        Ok(Ok(()))
    }
}

impl calculator::Handler<Option<async_nats::HeaderMap>> for NativeCalculator {
    async fn calculate(
        &self,
        _cx: Option<async_nats::HeaderMap>,
        _req: OperationRequest,
    ) -> anyhow::Result<Result<u64, String>> {
        Ok(Ok(0))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let nats = async_nats::connect("localhost:4222").await?;
    let provider_connection = ProviderConnection::new(nats);
    let wrpc = wrpc_transport_nats::Client::new(nats, "test", Some("test".into())).await?;
    let invocations = crate::bindings::serve(&wrpc, NativeCalculator::default()).await?;

    let (join_handle, cancel_token) =
        crate::serve_invocations::serve_invocations(invocations).await?;
    join_handle.await?;
    cancel_token.cancel();

    Ok(())
}
