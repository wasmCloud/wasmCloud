#![cfg(feature = "providers")]

use core::str;
use core::time::Duration;

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context};
use tokio::task::JoinSet;
use tokio::time::{sleep, timeout};
use tokio::try_join;
use tracing_subscriber::prelude::*;
use wasmcloud_test_util::lattice::config::assert_config_put;
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

const LATTICES: &[&str] = &["default", "test-lattice"];
const COMPONENT_ID: &str = "http_hello_world";

#[tokio::test]
async fn host_multiple_lattices() -> anyhow::Result<()> {
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
    let default_client = wasmcloud_control_interface::ClientBuilder::new(nats_client.clone())
        .lattice(LATTICES[0].to_string())
        .build();
    // Build the host
    let lattices: Vec<Box<str>> = LATTICES.iter().map(|l| Box::from(*l)).collect();
    let host = WasmCloudTestHost::start(&nats_url, Arc::from(lattices))
        .await
        .context("failed to start test host")?;

    let http_port = free_port().await?;
    let http_server_config_name = "http-server".to_string();
    let rust_http_server = providers::rust_http_server().await;
    let rust_http_server_id = rust_http_server.subject.public_key();

    try_join!(
        async {
            assert_config_put(
                &default_client,
                &http_server_config_name,
                [(
                    "ADDRESS".to_string(),
                    format!("{}:{http_port}", Ipv4Addr::LOCALHOST),
                )],
            )
            .await
            .context("failed to put configuration")
        },
        async {
            let host_key = host.host_key();
            let rust_http_server_url = rust_http_server.url();
            assert_start_provider(StartProviderArgs {
                client: &default_client,
                lattice: LATTICES[0],
                host_key: &host_key,
                provider_key: &rust_http_server.subject,
                provider_id: &rust_http_server_id,
                url: &rust_http_server_url,
                config: vec![],
            })
            .await
            .context("failed to start providers")
        },
        async {
            assert_scale_component(
                &default_client,
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
        &default_client,
        &rust_http_server_id,
        COMPONENT_ID,
        "default",
        "wasi",
        "http",
        vec!["incoming-handler".to_string()],
        vec![http_server_config_name],
        vec![],
    )
    .await
    .context("failed to advertise link")?;

    // Wait for data to be propagated across lattice
    sleep(Duration::from_secs(1)).await;

    let test_lattice_client = host.get_ctl_client(Some(nats_client), LATTICES[1]).await?;
    let inventory = test_lattice_client
        .get_host_inventory(&host.host_key().public_key())
        .await
        .map_err(|e| anyhow!(e).context("failed to get host inventory"))?;
    let d = inventory.data().unwrap();
    assert_eq!(d.components().len(), 0);

    assert_scale_component(
        &test_lattice_client,
        &host.host_key(),
        format!("file://{RUST_HTTP_HELLO_WORLD}"),
        COMPONENT_ID,
        None,
        5,
        Vec::new(),
    )
    .await
    .context("failed to scale `rust-http-hello-world` component on test-lattice")?;
    let inventory_resp = test_lattice_client
        .get_host_inventory(&host.host_key().public_key())
        .await
        .map_err(|e| anyhow!(e).context("failed to get host inventory"))?;
    let inventory = inventory_resp.data().unwrap();

    // Check that the component started and there isn't a provider
    assert_eq!(inventory.components().len(), 1);
    assert_eq!(inventory.providers().len(), 0);

    // Verify that the second lattice has no links
    let links = test_lattice_client
        .get_links()
        .await
        .map_err(|e| anyhow!(e).context("failed to get links"))?;

    let links = links.data().unwrap();
    assert_eq!(links.len(), 0);

    let test_claims = test_lattice_client
        .get_claims()
        .await
        .map_err(|e| anyhow!(e).context("failed to get claims"))?;
    let claims = test_claims.data().unwrap();

    let default_claims = default_client.get_claims().await.map_err(|e| anyhow!(e))?;
    let default_claims = default_claims.data().unwrap();
    assert_ne!(claims, default_claims);

    _ = nats_server.stop().await;
    Ok(())
}

#[tokio::test]
async fn multiple_lattices_host_api() -> anyhow::Result<()> {
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
    let default_client = wasmcloud_control_interface::ClientBuilder::new(nats_client.clone())
        .lattice(LATTICES[0].to_string())
        .build();
    // Build the host
    let lattices: Vec<Box<str>> = LATTICES.iter().map(|l| Box::from(*l)).collect();
    let host = WasmCloudTestHost::start(&nats_url, Arc::from(lattices))
        .await
        .context("failed to start test host")?;

    let http_port = free_port().await?;
    let http_server_config_name = "http-server".to_string();
    let rust_http_server = providers::rust_http_server().await;
    let rust_http_server_id = rust_http_server.subject.public_key();

    try_join!(
        async {
            assert_config_put(
                &default_client,
                &http_server_config_name,
                [(
                    "ADDRESS".to_string(),
                    format!("{}:{http_port}", Ipv4Addr::LOCALHOST),
                )],
            )
            .await
            .context("failed to put configuration")
        },
        async {
            let host_key = host.host_key();
            let rust_http_server_url = rust_http_server.url();
            assert_start_provider(StartProviderArgs {
                client: &default_client,
                lattice: LATTICES[0],
                host_key: &host_key,
                provider_key: &rust_http_server.subject,
                provider_id: &rust_http_server_id,
                url: &rust_http_server_url,
                config: vec![],
            })
            .await
            .context("failed to start providers")
        },
        async {
            assert_scale_component(
                &default_client,
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

    // Verify that host labels still change regardless of the lattice client
    let test_lattice_client = host.get_ctl_client(Some(nats_client), LATTICES[1]).await?;
    test_lattice_client
        .put_label(&host.host_key().public_key(), "i-am-a-test-label", "true")
        .await
        .map_err(|e| anyhow!(e).context("failed to put label"))?;

    let inventory = default_client
        .get_host_inventory(&host.host_key().public_key())
        .await
        .map_err(|e| anyhow!(e).context("failed to get host inventory"))?;
    let inventory = inventory.data().unwrap();
    let labels = inventory.labels();
    let value = labels.get("i-am-a-test-label");
    assert!(value.is_some());
    assert_eq!(value.unwrap(), "true");

    default_client
        .delete_label(&host.host_key().public_key(), "i-am-a-test-label")
        .await
        .map_err(|e| anyhow!(e).context("failed to delete label"))?;

    let inventory = test_lattice_client
        .get_host_inventory(&host.host_key().public_key())
        .await
        .map_err(|e| anyhow!(e).context("failed to get host inventory"))?;
    let inventory = inventory.data().unwrap();
    let labels = inventory.labels();
    let value = labels.get("i-am-a-test-label");
    assert!(value.is_none());

    default_client
        .put_config(
            "test-config",
            HashMap::from([("test-key".to_string(), "test-value".to_string())]),
        )
        .await
        .map_err(|e| anyhow!(e).context("failed to put config"))?;

    let result = test_lattice_client
        .get_config("test-config")
        .await
        .map_err(|e| anyhow!(e).context("failed to make config call"))?;
    assert!(result.data().is_none());

    _ = nats_server.stop().await;
    Ok(())
}

#[tokio::test]
async fn host_events_multiple_lattices() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,cranelift_codegen=warn,wasmcloud=trace")
            }),
        )
        .init();

    let (_nats_server, nats_url, nats_client) =
        start_nats().await.context("failed to start NATS")?;

    let default_client = wasmcloud_control_interface::ClientBuilder::new(nats_client.clone())
        .lattice(LATTICES[0].to_string())
        .build();
    let test_lattice_client = wasmcloud_control_interface::ClientBuilder::new(nats_client.clone())
        .lattice(LATTICES[1].to_string())
        .build();

    let mut default_evt = default_client
        .events_receiver(vec!["component_scaled".to_string()])
        .await
        .map_err(|e| anyhow!(e).context("failed to get events receiver"))?;
    let mut test_evt = test_lattice_client
        .events_receiver(vec!["component_scaled".to_string()])
        .await
        .map_err(|e| anyhow!(e).context("failed to get events receiver"))?;

    let (scaled_event_tx, scaled_event_rx) =
        tokio::sync::oneshot::channel::<anyhow::Result<serde_json::Value>>();
    tokio::spawn(async move {
        let evt = default_evt.recv().await.unwrap();
        let data = evt.data().unwrap();
        match data {
            cloudevents::event::Data::Json(v) => {
                scaled_event_tx.send(Ok(v.clone())).unwrap();
            }
            _ => scaled_event_tx
                .send(Err(anyhow!("unexpected data type")))
                .unwrap(),
        }
    });

    let (test_lattice_tx, test_lattice_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = test_evt.recv().await.unwrap();
        test_lattice_tx.send(()).unwrap();
    });

    // Build the host
    let lattices: Vec<Box<str>> = LATTICES.iter().map(|l| Box::from(*l)).collect();
    let host = WasmCloudTestHost::start(&nats_url, Arc::from(lattices))
        .await
        .context("failed to start test host")?;

    let http_port = free_port().await?;
    let http_server_config_name = "http-server".to_string();
    let rust_http_server = providers::rust_http_server().await;
    let rust_http_server_id = rust_http_server.subject.public_key();

    try_join!(
        async {
            assert_config_put(
                &default_client,
                &http_server_config_name,
                [(
                    "ADDRESS".to_string(),
                    format!("{}:{http_port}", Ipv4Addr::LOCALHOST),
                )],
            )
            .await
            .context("failed to put configuration")
        },
        async {
            let host_key = host.host_key();
            let rust_http_server_url = rust_http_server.url();
            assert_start_provider(StartProviderArgs {
                client: &default_client,
                lattice: LATTICES[0],
                host_key: &host_key,
                provider_key: &rust_http_server.subject,
                provider_id: &rust_http_server_id,
                url: &rust_http_server_url,
                config: vec![],
            })
            .await
            .context("failed to start providers")
        },
        async {
            assert_scale_component(
                &default_client,
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

    let evt = scaled_event_rx
        .await
        .context("failed to receive cloudevent")?;
    let evt = evt.context("failed to receive event: wrong type (string or binary)")?;
    let component_id = evt["component_id"].as_str().unwrap();
    assert_eq!(component_id, COMPONENT_ID);

    // If we recieve a component_scaled event on the test lattice, that's an error
    if timeout(Duration::from_millis(500), test_lattice_rx)
        .await
        .is_ok()
    {
        anyhow::bail!("received unexpected component_scaled event on test lattice");
    };

    Ok(())
}

#[tokio::test]
async fn thousands_of_lattices() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,cranelift_codegen=warn,wasmcloud=trace")
            }),
        )
        .init();

    let (_nats_server, nats_url, nats_client) =
        start_nats().await.context("failed to start NATS")?;

    // Arbitrary number of lattices
    let lattices: Vec<Box<str>> = (0..2000).map(|i| format!("lattice-{}", i).into()).collect();
    let _host = WasmCloudTestHost::start(&nats_url, Arc::from(lattices.clone()))
        .await
        .context("failed to start test host")?;

    let mut set: JoinSet<anyhow::Result<()>> = JoinSet::new();
    for lattice in &lattices {
        let lattice = lattice.clone();
        let nats_client = nats_client.clone();
        set.spawn(async move {
            let client = wasmcloud_control_interface::ClientBuilder::new(nats_client)
                .lattice(lattice.to_string())
                .build();
            client
                .put_config(
                    &format!("test-config-{lattice}"),
                    HashMap::from([("test-key".to_string(), "test-value".to_string())]),
                )
                .await
                .map_err(|e| anyhow!(e).context("failed to put config"))?;

            Ok(())
        });
    }
    while let Some(result) = set.join_next().await {
        let result = result?;
        if result.is_err() {
            bail!("failed to put config in a lattice");
        }
    }

    Ok(())
}
