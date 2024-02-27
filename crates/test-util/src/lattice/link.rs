//! Utilities for managing lattice links

use anyhow::{anyhow, Result};
use wasmcloud_control_interface::InterfaceLinkDefinition;

#[allow(clippy::too_many_arguments)]
pub async fn assert_advertise_link(
    client: impl Into<&wasmcloud_control_interface::Client>,
    source_id: impl AsRef<str>,
    target: impl AsRef<str>,
    link_name: impl AsRef<str>,
    wit_namespace: impl AsRef<str>,
    wit_package: impl AsRef<str>,
    interfaces: Vec<String>,
    source_config: Vec<String>,
    target_config: Vec<String>,
) -> Result<()> {
    let client = client.into();
    let source_id = source_id.as_ref();
    let target = target.as_ref();
    let link_name = link_name.as_ref();
    let wit_namespace = wit_namespace.as_ref();
    let wit_package = wit_package.as_ref();
    client
        .put_link(InterfaceLinkDefinition {
            source_id: source_id.to_string(),
            target: target.to_string(),
            name: link_name.to_string(),
            wit_namespace: wit_namespace.to_string(),
            wit_package: wit_package.to_string(),
            interfaces,
            source_config,
            target_config,
        })
        .await
        .map_err(|e| anyhow!(e).context("failed to advertise link"))?;
    Ok(())
}

pub async fn assert_remove_link(
    client: impl Into<&wasmcloud_control_interface::Client>,
    actor_id: impl AsRef<str>,
    wit_namespace: impl AsRef<str>,
    wit_package: impl AsRef<str>,
    link_name: impl AsRef<str>,
) -> Result<()> {
    let client = client.into();
    let actor_id = actor_id.as_ref();
    let wit_namespace = wit_namespace.as_ref();
    let wit_package = wit_package.as_ref();
    let link_name = link_name.as_ref();
    client
        .delete_link(actor_id, link_name, wit_namespace, wit_package)
        .await
        .map_err(|e| anyhow!(e).context("failed to delete link"))?;
    Ok(())
}
