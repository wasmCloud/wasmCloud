use std::sync::Arc;

use anyhow::{Context as _, Result};
use tokio::time::Duration;
use tokio::try_join;

use wasmcloud_test_util::control_interface::ClientBuilder;
use wasmcloud_test_util::lattice::link::{assert_advertise_link, assert_remove_link};
use wasmcloud_test_util::nats::wait_for_nats_connection;
use wasmcloud_test_util::provider::StartProviderArgs;
use wasmcloud_test_util::testcontainers::{AsyncRunner as _, ImageExt, NatsServer};
use wasmcloud_test_util::{
    assert_config_put, assert_scale_component, assert_start_provider, WasmCloudTestHost,
};

const PROVIDER_HTTP_SERVER_IMAGE_REF: &str = "ghcr.io/wasmcloud/http-server:0.23.2";
const COMPONENT_HELLO_WORLD_RUST_IMAGE_REF: &str =
    "ghcr.io/wasmcloud/components/http-hello-world-rust:0.1.0";
const COMPONENT_HTTP_JSONIFY_RUST_IMAGE_REF: &str =
    "ghcr.io/wasmcloud/components/http-jsonify-rust:0.1.1";

/// Ensure duplicate links from the http provider to multiple components with
/// the same details except link name act consistently
///
/// In this case, the flow of this regression test is roughly:
///   - create provider & components
///   - link the http server to *both* components (rust first)
///   - Perform a HTTP request to the HTTP server
///   - Delete the default link
///   - The *second* established link (i.e. to the rust component) should still be present
///   - The *third* established link (i.e. to the rust component) should still be present
#[tokio::test]
// TODO(vados-cosmonic): this test can be enabled once a new HTTP server is released
#[ignore]
async fn duplicate_link_shadowing() -> Result<()> {
    // Build a wasmCloud host (assuming you have a local NATS server running)
    let lattice = "default";
    let component_hello_rust_id = "http-hello-rust";
    let component_jsonify_rust_id = "http-jsonify-rust";
    let provider_http_id = "http-server";
    let provider_http_config_name = "http-server-config";
    let provider_http_address = format!(
        "127.0.0.1:{}",
        wasmcloud_test_util::os::free_port_ipv4().await?
    );
    let endpoint_url = format!("http://{provider_http_address}");

    // Start NATS
    let nats_container = NatsServer::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("failed to start nats-server container");
    let nats_port = nats_container
        .get_host_port_ipv4(4222)
        .await
        .expect("should be able to find the NATS port");
    let nats_address = format!("nats://127.0.0.1:{nats_port}");

    // Start the host
    let host = WasmCloudTestHost::start(&nats_address, lattice).await?;
    let host_id = Arc::new(host.host_key().public_key());

    // Once you have a host (AKA a single-member wasmCloud lattice), you'll want a NATS client
    // which you can use to control the host and the lattice:
    let nats_client = wait_for_nats_connection(&nats_address).await?;
    let ctl_client = ClientBuilder::new(nats_client)
        .lattice(host.lattice_name().to_string())
        .build();

    // Perform all link and config puts
    // (NOTE: this *must* be sequential, as we are testing order)
    assert_config_put(
        &ctl_client,
        provider_http_config_name,
        [("address".to_string(), provider_http_address.clone())],
    )
    .await
    .context("failed to set config for provider")?;
    assert_advertise_link(
        &ctl_client,
        provider_http_id,
        component_hello_rust_id,
        "default",
        "wasi",
        "http",
        vec!["incoming-handler".into()],
        vec![provider_http_config_name.into()],
        Vec::with_capacity(0),
    )
    .await?;
    assert_advertise_link(
        &ctl_client,
        provider_http_id,
        component_hello_rust_id,
        "hello",
        "wasi",
        "http",
        vec!["incoming-handler".into()],
        vec![provider_http_config_name.into()],
        Vec::with_capacity(0),
    )
    .await?;
    assert_advertise_link(
        &ctl_client,
        provider_http_id,
        component_jsonify_rust_id,
        "jsonify",
        "wasi",
        "http",
        vec!["incoming-handler".into()],
        vec![provider_http_config_name.into()],
        Vec::with_capacity(0),
    )
    .await?;

    // Start the provider and components
    let ((), (), ()) = try_join!(
        assert_start_provider(StartProviderArgs {
            client: &ctl_client,
            host_id: &host_id,
            provider_id: provider_http_id,
            provider_ref: PROVIDER_HTTP_SERVER_IMAGE_REF,
            config: vec![provider_http_config_name.into()],
        }),
        assert_scale_component(
            &ctl_client,
            host_id.as_ref(),
            COMPONENT_HELLO_WORLD_RUST_IMAGE_REF,
            component_hello_rust_id,
            None,
            1,
            Vec::new(),
            Duration::from_secs(10),
        ),
        assert_scale_component(
            &ctl_client,
            host_id.as_ref(),
            COMPONENT_HTTP_JSONIFY_RUST_IMAGE_REF,
            component_jsonify_rust_id,
            None,
            1,
            Vec::new(),
            Duration::from_secs(10),
        ),
    )
    .context("failed to start provider and components")?;

    // Wait until we can get any response from the HTTP server
    wasmcloud_test_util::http::wait_for_url(&endpoint_url)
        .await
        .context("Provider HTTP URL was never available")?;

    // Perform a request which should go to the rust component ('default' link)
    let get_lowercased_resp = || async {
        reqwest::get(&endpoint_url)
            .await
            .context("failed to get /")?
            .text()
            .await
            .map(|s| s.to_lowercase())
            .context("failed to get textual output of request")
    };
    assert!(
        get_lowercased_resp().await?.contains("hello from rust"),
        "first response should be from rust component"
    );

    // Delete the default link
    assert_remove_link(&ctl_client, provider_http_id, "wasi", "http", "default")
        .await
        .context("failed to remove link 'default'")?;

    // At this point, the provider should *STILL* be pointing to the rust component,
    // as it was set *second* ('hello' link)
    assert!(
        get_lowercased_resp().await?.contains("hello from rust"),
        "second response should be from rust component"
    );

    // Delete the rust link
    assert_remove_link(&ctl_client, provider_http_id, "wasi", "http", "rust")
        .await
        .context("failed to remove link 'rust'")?;

    // At this point, the provider should be pointing to the jsonify component,
    // as it was set *third* ('jsonify' link)
    assert!(
        !get_lowercased_resp().await?.contains("hello from rust"),
        "third response be from jsonify component",
    );

    Ok(())
}
