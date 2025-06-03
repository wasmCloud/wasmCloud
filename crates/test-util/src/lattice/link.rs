//! Utilities for managing lattice links

use anyhow::{anyhow, Result};
use wasmcloud_control_interface::{CtlResponse, Link};
use wasmcloud_core::KnownConfigName;

#[allow(clippy::too_many_arguments)]
pub async fn assert_advertise_link(
    client: impl Into<&wasmcloud_control_interface::Client>,
    source_id: impl AsRef<str>,
    target: impl AsRef<str>,
    link_name: impl AsRef<str>,
    wit_namespace: impl AsRef<str>,
    wit_package: impl AsRef<str>,
    interfaces: Vec<String>,
    source_config: Vec<KnownConfigName>,
    target_config: Vec<KnownConfigName>,
) -> Result<CtlResponse<()>> {
    let client = client.into();
    let source_id = source_id.as_ref();
    let target = target.as_ref();
    let link_name = link_name.as_ref();
    let wit_namespace = wit_namespace.as_ref();
    let wit_package = wit_package.as_ref();
    client
        .put_link(
            Link::builder()
                .source_id(source_id)
                .target(target)
                .name(link_name)
                .wit_namespace(wit_namespace)
                .wit_package(wit_package)
                .interfaces(interfaces)
                .source_config(source_config)
                .target_config(target_config)
                .build()
                .map_err(|e| anyhow!(e).context("failed to build link"))?,
        )
        .await
        .map_err(|e| anyhow!(e).context("failed to advertise link"))
}

pub async fn assert_remove_link(
    client: impl Into<&wasmcloud_control_interface::Client>,
    component_id: impl AsRef<str>,
    wit_namespace: impl AsRef<str>,
    wit_package: impl AsRef<str>,
    link_name: impl AsRef<str>,
) -> Result<()> {
    let client = client.into();
    let component_id = component_id.as_ref();
    let wit_namespace = wit_namespace.as_ref();
    let wit_package = wit_package.as_ref();
    let link_name = link_name.as_ref();
    client
        .delete_link(component_id, link_name, wit_namespace, wit_package)
        .await
        .map_err(|e| anyhow!(e).context("failed to delete link"))?;
    Ok(())
}
