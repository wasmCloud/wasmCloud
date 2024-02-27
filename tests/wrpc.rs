use anyhow::Context;
use test_actors::{RUST_WRPC_PINGER_COMPONENT, RUST_WRPC_PONGER_COMPONENT};
use wasmcloud_test_util::{
    actor::assert_scale_actor, host::WasmCloudTestHost, lattice::link::assert_advertise_link,
};
use wrpc_transport::Client;

pub mod common;
use common::nats::start_nats;

const LATTICE: &str = "default";
const PINGER_COMPONENT_ID: &str = "wrpc_pinger_component";
const PONGER_COMPONENT_ID: &str = "wrpc_ponger_component";

#[tokio::test]
async fn wrpc() -> anyhow::Result<()> {
    // Start NATS server
    let (nats_server, nats_url, nats_client) =
        start_nats().await.expect("should be able to start NATS");
    // Build client for interacting with the lattice
    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client.clone())
        .lattice(LATTICE.to_string())
        .build();
    let wrpc_client =
        wrpc_transport_nats::Client::new(nats_client, format!("{LATTICE}.{PINGER_COMPONENT_ID}"));
    // Build the host
    let host = WasmCloudTestHost::start(&nats_url, LATTICE, None, None)
        .await
        .context("failed to start test host")?;

    // Scale pinger
    assert_scale_actor(
        &ctl_client,
        &host.host_key(),
        format!("file://{RUST_WRPC_PINGER_COMPONENT}"),
        PINGER_COMPONENT_ID,
        None,
        5,
    )
    .await
    .expect("should've scaled pinger actor");

    // Scale ponger
    assert_scale_actor(
        &ctl_client,
        &host.host_key(),
        format!("file://{RUST_WRPC_PONGER_COMPONENT}"),
        PONGER_COMPONENT_ID,
        None,
        5,
    )
    .await
    .expect("should've scaled actor");

    // Link pinger --wrpc:testing/pingpong--> ponger
    assert_advertise_link(
        &ctl_client,
        PINGER_COMPONENT_ID,
        PONGER_COMPONENT_ID,
        "default",
        "wrpc",
        "testing",
        vec!["pingpong".to_string()],
        vec![],
        vec![],
    )
    .await
    .expect("should advertise link");

    let result = wrpc_client
        .invoke_dynamic(
            "wrpc:testing/invoke",
            "call",
            [],
            &[wrpc_types::Type::String],
        )
        .await;
    match result {
        Ok((values, _tx)) => {
            if let Some(wrpc_transport::Value::String(result)) = values.first() {
                assert_eq!(result, "Ping pong");
            } else {
                panic!("Got something other than a string from the component")
            }
        }
        _ => panic!("Error"),
    }

    nats_server
        .stop()
        .await
        .expect("should be able to stop NATS");
    Ok(())
}
