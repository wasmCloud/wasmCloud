use anyhow::Result;
use serial_test::serial;

mod common;
use common::{TestWashInstance, HELLO_OCI_REF, PROVIDER_HTTPSERVER_OCI_REF};

#[tokio::test]
#[serial]
#[cfg_attr(
    not(can_reach_wasmcloud_azurecr_io),
    ignore = "wasmcloud.azurecr.io is not reachable"
)]
async fn integration_start_stop_actor_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    // Start the actor via OCI ref
    wash_instance
        .start_actor(HELLO_OCI_REF, "hello_actor_id")
        .await?;

    wash_instance.stop_actor("hello_actor_id", None).await?;

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
