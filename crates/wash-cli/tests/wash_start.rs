use anyhow::Result;
use serial_test::serial;

mod common;
use common::{TestWashInstance, ECHO_OCI_REF, PROVIDER_HTTPSERVER_OCI_REF};

#[tokio::test]
#[serial]
#[cfg_attr(
    not(can_reach_wasmcloud_azurecr_io),
    ignore = "wasmcloud.azurecr.io is not reachable"
)]
async fn integration_start_stop_actor_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    // Start the actor via OCI ref
    wash_instance.start_actor(ECHO_OCI_REF).await?;

    // Test stopping using only aliases, yes I know this mixes stop and start, but saves on copied
    // code
    wash_instance.stop_actor("echo", None).await?;

    Ok(())
}

#[tokio::test]
#[serial]
async fn integration_start_stop_provider_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    wash_instance
        .start_provider(PROVIDER_HTTPSERVER_OCI_REF)
        .await?;

    // Test stopping using only aliases, yes I know this mixes stop and start, but saves on copied
    // code
    wash_instance
        .stop_provider("server", "wasmcloud:httpserver", None, None)
        .await?;

    Ok(())
}
