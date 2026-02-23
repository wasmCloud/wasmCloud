//! Import bridging: polyfill component imports by invoking remote services via wRPC/NATS.
//!
//! For each component import that has a `wrpc:name` config key, this module uses
//! `wrpc_runtime_wasmtime::link_instance` to bind linker entries. All encoding/decoding
//! is handled by wrpc-runtime-wasmtime internally — the plugin only needs to configure
//! the routing invoker on the store's `WrpcView` so `link_instance` can find the
//! correct NATS client for each interface.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use tracing::{debug, info, instrument};
use wasmtime::component::types::ComponentItem;

use crate::engine::ctx::SharedCtx;
use crate::engine::workload::WorkloadItem;
use crate::wit::WitInterface;

/// Bind import functions for all wrpc-matched interfaces on this component.
///
/// For each interface in `interfaces` that the component imports:
/// 1. Creates a `wrpc_transport_nats::Client` for the routing key
/// 2. Registers the route on the component's `WorkloadMetadata` routing invoker
/// 3. Uses `wrpc_runtime_wasmtime::link_instance` to polyfill all functions
///    in that interface — wrpc handles all encoding/decoding automatically
#[instrument(skip_all)]
pub(super) async fn bind_imports(
    nats_client: &Arc<async_nats::Client>,
    prefix: &str,
    item: &mut WorkloadItem<'_>,
    interfaces: &std::collections::HashSet<WitInterface>,
) -> anyhow::Result<()> {
    let component = item.component().clone();
    let engine = component.engine();
    let component_type = component.component_type();

    for interface in interfaces {
        let routing_key = match interface.config.get("wrpc:name") {
            Some(key) => key.clone(),
            None => continue,
        };

        // Find this interface in the component's imports
        for (import_name, import_item) in component_type.imports(engine) {
            let ComponentItem::ComponentInstance(instance_ty) = import_item else {
                continue;
            };

            let wit_import = WitInterface::from(import_name);
            if interface.contains(&wit_import) {
                info!(
                    import_name = %import_name,
                    routing_key = %routing_key,
                    "interface matches component import, collecting functions"
                );
            } else {
                info!(
                    import_name = %import_name,
                    routing_key = %routing_key,
                    "interface does not match component import, skipping"
                );
                continue;
            }

            let wrpc_prefix = format!("{prefix}.{routing_key}");

            // Create a wrpc NATS client for this routing key
            let wrpc_client = Arc::new(
                wrpc_transport_nats::Client::new(
                    nats_client.clone(),
                    Arc::from(wrpc_prefix.as_str()),
                    None,
                )
                .await
                .context("failed to create wrpc NATS client")?,
            );

            debug!(
                import_name = %import_name,
                routing_key = %routing_key,
                "binding wrpc import via link_instance"
            );

            // Register the route so the RoutingInvoker can delegate to this client
            item.wrpc_invoker_mut().add_route(import_name, wrpc_client);

            // Use wrpc-runtime-wasmtime's link_instance to polyfill all functions
            let linker = item.linker();
            let mut linker_instance = linker.instance(import_name)?;
            wrpc_runtime_wasmtime::link_instance::<SharedCtx>(
                engine,
                &mut linker_instance,
                [],                                               // guest_resources: none
                HashMap::<Box<str>, HashMap<Box<str>, _>>::new(), // host_resources: none
                instance_ty,
                import_name,
            )?;
        }
    }

    Ok(())
}
