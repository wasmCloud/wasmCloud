use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use tokio::sync::RwLock;
use tracing::{debug, error, info};
use wasmcloud_provider_sdk::{
    run_provider, serve_provider_exports, Context, LinkConfig, LinkDeleteInfo, Provider,
    ProviderInitConfig,
};

use crate::config::ProviderConfig;

pub(crate) mod bindings {
    wit_bindgen_wrpc::generate!();
}

// The code generated by `wit-bindgen-wrpc` for your exports follow a pattern:
// crate::<world_name>::exports::<namespace>::<package>::<interface>::*
use bindings::exports::wasmcloud::example::system_info::Handler;
use bindings::exports::wasmcloud::example::system_info::Kind;
// The code generated by `wit-bindgen-wrpc` for your imports follow a pattern:
// crate::<world_name>::<namespace>::<package>::<interface>::*
use bindings::wasmcloud::example::process_data;
use bindings::wasmcloud::example::process_data::Data;

#[derive(Default, Clone)]
/// Your provider struct is where you can store any state or configuration that your provider needs to keep track of.
pub struct CustomTemplateProvider {
    config: Arc<RwLock<ProviderConfig>>,
    /// All components linked to this provider and their config.
    linked_from: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
    /// All components this provider is linked to and their config
    linked_to: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
}

/// This `impl` block is where you can implement additional methods for your provider. We've provided two examples
/// to run and load [`HostData`], and when you have custom logic to implement, you can add it here.
impl CustomTemplateProvider {
    fn name() -> &'static str {
        "custom-template-provider"
    }

    /// Execute the provider, loading [`HostData`] from the host which includes the provider's configuration and
    /// information about the host. Once you use the passed configuration to construct a [`CustomTemplateProvider`],
    /// you can run the provider by calling `run_provider` and then serving the provider's exports on the proper
    /// RPC topics via `wrpc::serve`.
    ///
    /// This step is essentially the same for every provider, and you shouldn't need to modify this function.
    pub async fn run() -> anyhow::Result<()> {
        let provider = Self::default();
        let shutdown = run_provider(provider.clone(), CustomTemplateProvider::name())
            .await
            .context("failed to run provider")?;

        // The [`serve`] function will set up RPC topics for your provider's exports and await invocations.
        // This is a generated function based on the contents in your `wit/world.wit` file.
        let connection = wasmcloud_provider_sdk::get_connection();
        serve_provider_exports(
            &connection.get_wrpc_client(connection.provider_key()),
            provider,
            shutdown,
            bindings::serve,
        )
        .await

        // If your provider has no exports, simply await the shutdown to keep the provider running
        // shutdown.await;
        // Ok(())
    }
}

// NOTE: These `use` statements by convention go at the top of the file, but they are included here for clarity.

/// When a provider specifies an `export` in its `wit/world.wit` file, the `wit-bindgen-wrpc` tool generates
/// a trait that the provider must implement. This trait is used to handle invocations from components that
/// link to the provider. The `Handler` trait is generated for each export in the WIT world.
impl Handler<Option<Context>> for CustomTemplateProvider {
    /// Request information about the system the provider is running on
    async fn request_info(&self, ctx: Option<Context>, kind: Kind) -> anyhow::Result<String> {
        // The `ctx` contains information about the component that invoked the request. You can use
        // this information to look up the configuration of the component that invoked the request.
        let requesting_component = ctx
            .and_then(|c| c.component)
            .unwrap_or_else(|| "UNKNOWN".to_string());
        let component_config = self
            .linked_from
            .read()
            .await
            .get(&requesting_component)
            .cloned()
            .unwrap_or_default();

        info!(
            requesting_component,
            ?kind,
            ?component_config,
            "received request for system information"
        );

        let info = match kind {
            Kind::Os => std::env::consts::OS,
            Kind::Arch => std::env::consts::ARCH,
        };
        Ok(format!("{info}"))
    }

    /// Request the provider to send some data to all linked components
    ///
    /// This function is easy to invoke with `wash call`,
    async fn call(&self, _ctx: Option<Context>) -> anyhow::Result<String> {
        info!("received call to send data to linked components");
        let mut last_response = None;
        for (component_id, config) in self.linked_to.read().await.iter() {
            debug!(component_id, ?config, "sending data to component");
            let sample_data = Data {
                name: "sup".to_string(),
                count: 3,
            };
            let client = wasmcloud_provider_sdk::get_connection().get_wrpc_client(component_id);
            match process_data::process(&client, None, &sample_data).await {
                Ok(response) => {
                    last_response = Some(response);
                    info!(
                        component_id,
                        ?config,
                        ?last_response,
                        "successfully sent data to component"
                    );
                }
                Err(e) => {
                    error!(
                        component_id,
                        ?config,
                        ?e,
                        "failed to send data to component"
                    );
                }
            }
        }

        Ok(last_response.unwrap_or_else(|| "No components responded to request".to_string()))
    }
}

/// Implementing the [`Provider`] trait is optional. Implementing the methods in the trait allow you to set up
/// custom logic for handling links, deletions, and shutdowns. This is useful to set up any connections, state,
/// resources, or cleanup that your provider needs to do when it is linked to or unlinked from a component.
impl Provider for CustomTemplateProvider {
    /// Initialize your provider with the given configuration. This is a good place to set up any state or
    /// resources your provider needs to run.
    async fn init(&self, config: impl ProviderInitConfig) -> anyhow::Result<()> {
        let provider_id = config.get_provider_id();
        let initial_config = config.get_config();
        info!(provider_id, ?initial_config, "initializing provider");

        // Save configuration to provider state
        *self.config.write().await = ProviderConfig::from(initial_config);

        Ok(())
    }

    /// When your provider is linked to a component, this method will be called with the [`LinkConfig`] that
    /// is passed in as source configuration. You can store this configuration in your provider's state to
    /// keep track of the components your provider is linked to.
    ///
    /// A concrete use case for this can be seen in our HTTP server provider, where we are given configuration
    /// for a port or an address to listen on, and we can use that configuration to start a webserver and forward
    /// any incoming requests to the linked component.
    async fn receive_link_config_as_source(
        &self,
        LinkConfig {
            target_id, config, ..
        }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        // We're storing the configuration as an example of how to keep track of linked components, but
        // the provider SDK does not require you to store this information.
        self.linked_to
            .write()
            .await
            .insert(target_id.to_string(), config.to_owned());

        debug!(
            "finished processing link from provider to component [{}]",
            target_id
        );
        Ok(())
    }

    /// When a component links to your provider, this method will be called with the [`LinkConfig`] that
    /// is passed in as target configuration. You can store this configuration in your provider's state to
    /// keep track of the components linked to your provider.
    ///
    /// A concrete use case for this can be seen in our key-value Redis provider, where we are given configuration
    /// for a Redis connection, and we can use that configuration to store and retrieve data from Redis. When an
    /// invocation is received from a component, we can look up the configuration for that component and use it
    /// to interact with the correct Redis instance.
    async fn receive_link_config_as_target(
        &self,
        LinkConfig {
            source_id, config, ..
        }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        self.linked_from
            .write()
            .await
            .insert(source_id.to_string(), config.to_owned());

        debug!(
            "finished processing link from component [{}] to provider",
            source_id
        );
        Ok(())
    }

    /// When a link is deleted from your provider to a component, this method will be called with the target ID
    /// of the component that was unlinked. You can use this method to clean up any state or resources that were
    /// associated with the linked component.
    async fn delete_link_as_source(&self, link: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let target = link.get_target_id();
        self.linked_to.write().await.remove(target);

        debug!(
            "finished processing delete link from provider to component [{}]",
            target
        );
        Ok(())
    }

    /// When a link is deleted from a component to your provider, this method will be called with the source ID
    /// of the component that was unlinked. You can use this method to clean up any state or resources that were
    /// associated with the linked component.
    async fn delete_link_as_target(&self, link: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let source_id = link.get_source_id();
        self.linked_from.write().await.remove(source_id);

        debug!(
            "finished processing delete link from component [{}] to provider",
            source_id
        );
        Ok(())
    }

    /// Handle shutdown request by cleaning out all linked components. This is a good place to clean up any
    /// resources or connections your provider has established.
    async fn shutdown(&self) -> anyhow::Result<()> {
        self.linked_from.write().await.clear();
        self.linked_to.write().await.clear();

        Ok(())
    }
}
