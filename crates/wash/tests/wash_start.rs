use anyhow::Result;
use serial_test::serial;

mod common;
use common::{TestWashInstance, HELLO_OCI_REF, PROVIDER_HTTPSERVER_OCI_REF};

#[tokio::test]
#[serial]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_start_stop_component_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    // Start the component via OCI ref
    wash_instance
        .start_component(HELLO_OCI_REF, "hello_component_id")
        .await?;

    wash_instance
        .stop_component("hello_component_id", None)
        .await?;

    Ok(())
}

#[tokio::test]
#[serial]
async fn integration_start_stop_provider_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    wash_instance
        .start_provider(PROVIDER_HTTPSERVER_OCI_REF, "httpserver_start_stop")
        .await?;

    wash_instance
        .stop_provider("httpserver_start_stop", None)
        .await?;

    Ok(())
}
