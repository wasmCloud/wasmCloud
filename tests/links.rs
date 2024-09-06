//! This module contains tests for the lattice's ability to manage links between components
//!
//! The primary goal of these tests are to ensure that creating links, deleting links,
//! checking link uniqueness, and rejecting invalid links are all functioning as expected.

use core::str;
use core::time::Duration;

use std::net::Ipv4Addr;

use anyhow::{bail, Context as _};
use futures::StreamExt;
use tokio::try_join;
use tracing::instrument;
use tracing_subscriber::prelude::*;
use wasmcloud_control_interface::CtlResponse;
use wasmcloud_core::tls::NativeRootsExt as _;
use wasmcloud_test_util::lattice::config::assert_config_put;
use wasmcloud_test_util::lattice::link::assert_remove_link;
use wasmcloud_test_util::provider::{assert_start_provider, StartProviderArgs};
use wasmcloud_test_util::{
    component::assert_scale_component, host::WasmCloudTestHost,
    lattice::link::assert_advertise_link,
};

use test_components::RUST_HTTP_HELLO_WORLD;

pub mod common;
use common::free_port;
use common::nats::start_nats;
use common::providers;

const LATTICE: &str = "links";
const COMPONENT_ID: &str = "http_hello_world";

/// Create many valid links between a few components, then ensure that links that are invalid
/// (e.g. a new link with the same source, WIT interface, and link name but different target) are rejected
#[instrument(skip_all, ret)]
#[tokio::test]
async fn link_deletes() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,cranelift_codegen=warn,wasmcloud=trace")
            }),
        )
        .init();

    let (nats_server, nats_url, nats_client) =
        start_nats().await.context("failed to start NATS")?;

    // Build client for interacting with the lattice
    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client.clone())
        .lattice(LATTICE.to_string())
        .build();
    // Build the host
    let _host = WasmCloudTestHost::start(&nats_url, LATTICE)
        .await
        .context("failed to start test host")?;

    // Set up a few IDs and interfaces for enumerating tests
    let component_one = "foo";
    let component_two = "bar";
    let interface_one = "test1";
    let interface_two = "test2";
    let interface_three = "test3";
    let link_name_one = "one";
    let link_name_two = "two";
    let link_name_three = "three";

    // Link one -(one)-> two on all interfaces
    define_link(
        &ctl_client,
        component_one,
        component_two,
        link_name_one,
        vec![interface_one, interface_two, interface_three],
    )
    .await?;
    // Link one -(two)-> three on all interfaces
    define_link(
        &ctl_client,
        component_one,
        component_two,
        link_name_two,
        vec![interface_one, interface_two, interface_three],
    )
    .await?;
    // Link two -(three)-> one on all interfaces
    define_link(
        &ctl_client,
        component_two,
        component_one,
        link_name_three,
        vec![interface_one, interface_two, interface_three],
    )
    .await?;

    // Ensure links were validly created
    let resp = ctl_client
        .get_links()
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    assert!(resp.success);
    assert!(resp.response.is_some());
    let links = resp.response.unwrap();
    assert_eq!(links.len(), 3);
    for link in links.iter() {
        match (&link.source_id, &link.name) {
            (id, name) if id == component_one && name == link_name_one => {
                assert_eq!(link.target, component_two);
                assert_eq!(link.interfaces.len(), 3);
            }
            (id, name) if id == component_one && name == link_name_two => {
                assert_eq!(link.target, component_two);
                assert_eq!(link.interfaces.len(), 3);
            }
            (id, name) if id == component_two && name == link_name_three => {
                assert_eq!(link.target, component_one);
                assert_eq!(link.interfaces.len(), 3);
            }
            (id, name) => bail!("unexpected link source {id} with name {name}"),
        }
    }

    let mut deleted_events = nats_client
        .subscribe(format!("wasmbus.evt.{LATTICE}.linkdef_deleted"))
        .await?;
    let mut provider_link_deleted_messages = nats_client
        .subscribe(format!("wasmbus.rpc.{LATTICE}.*.linkdefs.del"))
        .await?;

    // This link doesn't exist, so we should get an event but no provider message
    assert_remove_link(
        &ctl_client,
        component_one,
        NAMESPACE,
        "otherpackage",
        link_name_one,
    )
    .await?;
    // Link delete events are idempotent, so they're always published
    let deleted_event = deleted_events.next().await;
    assert!(deleted_event.is_some());
    // If the link didn't exist, the provider won't receive a message
    let provider_deleted_event = tokio::time::timeout(
        std::time::Duration::from_millis(10),
        provider_link_deleted_messages.next(),
    )
    .await;
    // timed out because it's not available
    assert!(provider_deleted_event.is_err());

    // Remove the one -(one)-> two link, designated by the link name
    assert_remove_link(
        &ctl_client,
        component_one,
        NAMESPACE,
        PACKAGE,
        link_name_one,
    )
    .await?;
    // Link delete events are idempotent, so they're always published
    let deleted_event = deleted_events.next().await;
    assert!(deleted_event.is_some());
    // If the link did exist, the provider should receive a message
    let provider_deleted_event = provider_link_deleted_messages.next().await;
    assert!(provider_deleted_event.is_some());
    // We actually publish one for the source and target, so there's two
    let provider_deleted_event = provider_link_deleted_messages.next().await;
    assert!(provider_deleted_event.is_some());

    nats_server.stop().await.context("failed to stop NATS")?;
    Ok(())
}

/// Ensure that the lattice can create multiple links from the same
/// source on the same WIT interface to different targets or with different configuration
/// (in this case, it's the same target, but the same principle applies to different targets)
#[instrument(skip_all, ret)]
#[tokio::test]
async fn link_name_support() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,cranelift_codegen=warn,wasmcloud=trace")
            }),
        )
        .init();

    let (nats_server, nats_url, nats_client) =
        start_nats().await.context("failed to start NATS")?;

    // Build client for interacting with the lattice
    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client)
        .lattice(LATTICE.to_string())
        .build();
    // Build the host
    let host = WasmCloudTestHost::start(&nats_url, LATTICE)
        .await
        .context("failed to start test host")?;

    let http_port = free_port().await?;
    let rust_http_server = providers::rust_http_server().await;
    let rust_http_server_id = rust_http_server.subject.public_key();

    let http_server_config_name = "http-server".to_string();
    assert_config_put(
        &ctl_client,
        &http_server_config_name,
        [
            (
                "default_address".to_string(),
                format!("{}:{http_port}", Ipv4Addr::LOCALHOST),
            ),
            ("routing_mode".to_string(), "path".to_string()),
        ],
    )
    .await
    .context("failed to put configuration")?;
    try_join!(
        async {
            assert_config_put(
                &ctl_client,
                "path_one",
                [("path".to_string(), "/one".to_string())],
            )
            .await
            .context("failed to put configuration")
        },
        async {
            assert_config_put(
                &ctl_client,
                "path_two",
                [("path".to_string(), "/two".to_string())],
            )
            .await
            .context("failed to put configuration")
        },
        async {
            assert_config_put(
                &ctl_client,
                "path_three",
                [("path".to_string(), "/three".to_string())],
            )
            .await
            .context("failed to put configuration")
        },
        async {
            let host_key = host.host_key();
            let rust_http_server_url = rust_http_server.url();
            assert_start_provider(StartProviderArgs {
                client: &ctl_client,
                lattice: LATTICE,
                host_key: &host_key,
                provider_key: &rust_http_server.subject,
                provider_id: &rust_http_server_id,
                url: &rust_http_server_url,
                config: vec![http_server_config_name],
            })
            .await
            .context("failed to start providers")
        },
        async {
            assert_scale_component(
                &ctl_client,
                &host.host_key(),
                format!("file://{RUST_HTTP_HELLO_WORLD}"),
                COMPONENT_ID,
                None,
                5,
                Vec::new(),
            )
            .await
            .context("failed to scale `rust-http-hello-world` component")
        }
    )?;

    assert_advertise_link(
        &ctl_client,
        &rust_http_server_id,
        COMPONENT_ID,
        "one",
        "wasi",
        "http",
        vec!["incoming-handler".to_string()],
        vec!["path_one".to_string()],
        vec![],
    )
    .await
    .context("failed to advertise link")?;
    assert_advertise_link(
        &ctl_client,
        &rust_http_server_id,
        COMPONENT_ID,
        "two",
        "wasi",
        "http",
        vec!["incoming-handler".to_string()],
        vec!["path_two".to_string()],
        vec![],
    )
    .await
    .context("failed to advertise link")?;
    assert_advertise_link(
        &ctl_client,
        &rust_http_server_id,
        COMPONENT_ID,
        "three",
        "wasi",
        "http",
        vec!["incoming-handler".to_string()],
        vec!["path_three".to_string()],
        vec![],
    )
    .await
    .context("failed to advertise link")?;

    let http_client = reqwest::Client::builder()
        .with_native_certificates()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(20))
        .build()
        .context("failed to build HTTP client")?;

    let base_url = format!("http://localhost:{http_port}");
    let req_one = http_client.get(format!("{base_url}/one")).send();
    let req_two = http_client.get(format!("{base_url}/two")).send();
    let req_three = http_client.get(format!("{base_url}/three")).send();

    let (res_one, res_two, res_three) = try_join!(req_one, req_two, req_three)?;

    assert!(res_one.status().is_success());
    assert!(res_two.status().is_success());
    assert!(res_three.status().is_success());

    let four_oh_four = http_client.get(format!("{base_url}/four")).send().await?;
    assert_eq!(four_oh_four.status().as_u16(), 404);

    nats_server.stop().await.context("failed to stop NATS")?;
    Ok(())
}

/// Create many valid links between a few components, then ensure that links that are invalid
/// (e.g. a new link with the same source, WIT interface, and link name but different target) are rejected
#[instrument(skip_all, ret)]
#[tokio::test]
async fn valid_and_invalid() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,cranelift_codegen=warn,wasmcloud=trace")
            }),
        )
        .init();

    let (nats_server, nats_url, nats_client) =
        start_nats().await.context("failed to start NATS")?;

    // Build client for interacting with the lattice
    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client)
        .lattice(LATTICE.to_string())
        .build();
    // Build the host
    let _host = WasmCloudTestHost::start(&nats_url, LATTICE)
        .await
        .context("failed to start test host")?;

    // Set up a few IDs and interfaces for enumerating tests
    let component_one = "component_one";
    let component_two = "component_two";
    let component_three = "component_three";
    let interface_one = "test1";
    let interface_two = "test2";
    let interface_three = "test3";
    let link_name_one = "one";
    let link_name_two = "two";
    let link_name_three = "three";

    //
    // VALID LINKS
    //

    // Link one -(one)-> two on all interfaces
    define_link(
        &ctl_client,
        component_one,
        component_two,
        link_name_one,
        vec![interface_one, interface_two, interface_three],
    )
    .await?;
    // Link two -(one)-> three on all interfaces
    define_link(
        &ctl_client,
        component_two,
        component_three,
        link_name_one,
        vec![interface_one, interface_two, interface_three],
    )
    .await?;
    // Link three -(one)-> one on all interfaces
    define_link(
        &ctl_client,
        component_three,
        component_one,
        link_name_one,
        vec![interface_one, interface_two, interface_three],
    )
    .await?;
    // NOTE: Defining one-at-a-time actually doesn't work. We need to be able to handle this
    // TODO: disjoint sets issue
    // define_link(
    //     &ctl_client,
    //     component_three,
    //     component_one,
    //     link_name_one,
    //     vec![interface_two],
    // )
    // .await?;
    // define_link(
    //     &ctl_client,
    //     component_three,
    //     &component_one,
    //     link_name_one,
    //     vec![interface_three],
    // )
    // .await?;

    // Ensure links were validly created
    let resp = ctl_client
        .get_links()
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    assert!(resp.success);
    assert!(resp.response.is_some());
    let links = resp.response.unwrap();
    assert_eq!(links.len(), 3);
    for link in links.iter() {
        match &link.source_id {
            id if id == component_one => {
                assert_eq!(link.target, component_two);
                assert_eq!(link.interfaces.len(), 3);
            }
            id if id == component_two => {
                assert_eq!(link.target, component_three);
                assert_eq!(link.interfaces.len(), 3);
            }
            id if id == component_three => {
                assert_eq!(link.target, component_one);
                assert_eq!(link.interfaces.len(), 3);
            }
            id => bail!("unexpected link source {id}"),
        }
    }

    // Components can be linked to different targets on different link names
    // Link one -(two)-> two on all interfaces
    define_link(
        &ctl_client,
        component_one,
        component_two,
        link_name_two,
        vec![interface_one, interface_two, interface_three],
    )
    .await?;
    // Link one -(three)-> three on all interfaces
    define_link(
        &ctl_client,
        component_one,
        component_two,
        link_name_three,
        vec![interface_one, interface_two, interface_three],
    )
    .await?;

    //
    // INVALID LINKS
    //

    // Component one is already linked to component two on all interfaces
    let invalid = define_link(
        &ctl_client,
        component_one,
        component_three,
        link_name_one,
        vec![interface_one, interface_two, interface_three],
    )
    .await?;
    assert!(!invalid.success);
    // This assertion will fail if we change the message, but it's a good test
    assert_eq!(invalid.message,  "link already exists with different target, consider deleting the existing link or using a different link name");
    // Component one is already linked to component two on all interfaces
    let invalid = define_link(
        &ctl_client,
        component_one,
        component_three,
        link_name_three,
        vec![interface_one],
    )
    .await?;
    assert!(!invalid.success);
    assert_eq!(invalid.message,  "link already exists with different target, consider deleting the existing link or using a different link name");
    // Component three is already linked to component one on all interfaces
    let invalid = define_link(
        &ctl_client,
        component_three,
        component_two,
        link_name_one,
        vec![interface_three],
    )
    .await?;
    assert!(!invalid.success);
    assert_eq!(invalid.message,  "link already exists with different target, consider deleting the existing link or using a different link name");

    nats_server.stop().await.context("failed to stop NATS")?;
    Ok(())
}

const NAMESPACE: &str = "wasi";
const PACKAGE: &str = "tests";

/// Helper function for the [`valid_and_invalid`] test when we're
/// defining a link with the same info often
async fn define_link(
    client: &wasmcloud_control_interface::Client,
    source: &str,
    target: &str,
    name: &str,
    interfaces: Vec<&str>,
) -> anyhow::Result<CtlResponse<()>> {
    assert_advertise_link(
        client,
        source,
        target,
        name,
        NAMESPACE,
        PACKAGE,
        interfaces.iter().map(|i| i.to_string()).collect(),
        vec![],
        vec![],
    )
    .await
    .context("failed to advertise link")
}
