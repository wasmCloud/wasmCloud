/// Tests for link lifecycle operations (add, delete, re-add, update)
/// These tests validate the fix for the race condition where re-adding links
/// after deletion would fail to properly notify providers.
pub mod common;

use anyhow::Context;
use common::free_port;
use common::nats::start_nats;
use std::net::Ipv4Addr;
use std::time::Duration;
use tokio::time::timeout;
use tracing::instrument;
use wasmcloud_test_util::lattice::config::assert_config_put;
use wasmcloud_test_util::lattice::link::{assert_advertise_link, assert_remove_link};
use wasmcloud_test_util::provider::{
    assert_start_provider, assert_stop_provider, StartProviderArgs,
};
use wasmcloud_test_util::{component::assert_scale_component, host::WasmCloudTestHost};

use test_components::RUST_HTTP_HELLO_WORLD;

const LATTICE: &str = "link_lifecycle_tests";
const COMPONENT_ID: &str = "http_hello_world";
const BUILTIN_HTTP_SERVER: &str = "wasmcloud+builtin://http-server";

/// Get path to the compiled hello world component
fn hello_world_component() -> &'static str {
    RUST_HTTP_HELLO_WORLD
}

/// Test multiple delete/re-add cycles to ensure link registration is idempotent
#[instrument(skip_all)]
#[tokio::test]
async fn test_multiple_link_cycles() -> anyhow::Result<()> {
    let (nats_server, nats_url, nats_client) = start_nats(None, true)
        .await
        .context("failed to start NATS")?;

    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(
        nats_client.expect("failed to build nats client"),
    )
    .lattice(LATTICE.to_string())
    .build();

    let host = WasmCloudTestHost::start(&nats_url, LATTICE)
        .await
        .context("failed to start test host")?;

    let http_port = free_port().await?;
    let http_server_id = "http-server".to_string();

    // Setup HTTP server config
    assert_config_put(
        &ctl_client,
        &http_server_id,
        [
            (
                "default_address".to_string(),
                format!("{}:{http_port}", Ipv4Addr::LOCALHOST),
            ),
            ("routing_mode".to_string(), "host".to_string()),
        ],
    )
    .await?;

    assert_config_put(
        &ctl_client,
        "test_host",
        [("host".to_string(), "test.local".to_string())],
    )
    .await?;

    // Start provider and component
    let host_key = host.host_key();
    assert_start_provider(StartProviderArgs {
        client: &ctl_client,
        host_id: &host_key.public_key(),
        provider_id: &http_server_id,
        provider_ref: BUILTIN_HTTP_SERVER,
        config: vec![http_server_id.clone()],
    })
    .await?;

    assert_scale_component(
        &ctl_client,
        host.host_key().public_key(),
        format!("file://{}", hello_world_component()),
        COMPONENT_ID,
        None,
        5,
        Vec::new(),
        Duration::from_secs(10),
    )
    .await?;

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    let base_url = format!("http://localhost:{http_port}");

    // Perform 5 cycles of add -> verify -> delete -> verify
    for cycle in 1..=5 {
        eprintln!("=== Cycle {cycle} ===");

        // Add link
        assert_advertise_link(
            &ctl_client,
            &http_server_id,
            COMPONENT_ID,
            "test",
            "wasi",
            "http",
            vec!["incoming-handler".to_string()],
            vec!["test_host".to_string()],
            vec![],
        )
        .await
        .with_context(|| format!("Cycle {cycle}: failed to add link"))?;

        // Small delay to let link propagate
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Verify link works
        let response = http_client
            .get(&base_url)
            .header("Host", "test.local")
            .send()
            .await
            .with_context(|| format!("Cycle {cycle}: failed to send request"))?;

        assert_eq!(
            response.status().as_u16(),
            200,
            "Cycle {cycle}: Expected 200 after adding link"
        );

        // Delete link
        assert_remove_link(&ctl_client, &http_server_id, "wasi", "http", "test")
            .await
            .with_context(|| format!("Cycle {cycle}: failed to delete link"))?;

        // Small delay to let deletion propagate
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Verify link is removed
        let response = http_client
            .get(&base_url)
            .header("Host", "test.local")
            .send()
            .await
            .with_context(|| format!("Cycle {cycle}: failed to send request after delete"))?;

        assert_eq!(
            response.status().as_u16(),
            404,
            "Cycle {cycle}: Expected 404 after deleting link"
        );
    }

    Ok(())
}

/// Test concurrent link operations on multiple links
#[instrument(skip_all)]
#[tokio::test]
async fn test_concurrent_link_operations() -> anyhow::Result<()> {
    let (nats_server, nats_url, nats_client) = start_nats(None, true)
        .await
        .context("failed to start NATS")?;

    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(
        nats_client.expect("failed to build nats client"),
    )
    .lattice(LATTICE.to_string())
    .build();

    let host = WasmCloudTestHost::start(&nats_url, LATTICE)
        .await
        .context("failed to start test host")?;

    let http_port = free_port().await?;
    let http_server_id = "http-server".to_string();

    // Setup configs
    assert_config_put(
        &ctl_client,
        &http_server_id,
        [
            (
                "default_address".to_string(),
                format!("{}:{http_port}", Ipv4Addr::LOCALHOST),
            ),
            ("routing_mode".to_string(), "host".to_string()),
        ],
    )
    .await?;

    for i in 1..=10 {
        assert_config_put(
            &ctl_client,
            &format!("host_{i}"),
            [("host".to_string(), format!("host{i}.local"))],
        )
        .await?;
    }

    // Start provider and component
    let host_key = host.host_key();
    assert_start_provider(StartProviderArgs {
        client: &ctl_client,
        host_id: &host_key.public_key(),
        provider_id: &http_server_id,
        provider_ref: BUILTIN_HTTP_SERVER,
        config: vec![http_server_id.clone()],
    })
    .await?;

    assert_scale_component(
        &ctl_client,
        host.host_key().public_key(),
        format!("file://{}", hello_world_component()),
        COMPONENT_ID,
        None,
        20,
        Vec::new(),
        Duration::from_secs(10),
    )
    .await?;

    // Add 10 links concurrently
    let mut tasks = vec![];
    for i in 1..=10 {
        let client = ctl_client.clone();
        let server_id = http_server_id.clone();
        tasks.push(tokio::spawn(async move {
            assert_advertise_link(
                &client,
                &server_id,
                COMPONENT_ID,
                &format!("link{i}"),
                "wasi",
                "http",
                vec!["incoming-handler".to_string()],
                vec![format!("host_{i}")],
                vec![],
            )
            .await
        }));
    }

    for task in tasks {
        task.await??;
    }

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify all links work
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    let base_url = format!("http://localhost:{http_port}");

    for i in 1..=10 {
        let response = http_client
            .get(&base_url)
            .header("Host", format!("host{i}.local"))
            .send()
            .await
            .with_context(|| format!("Failed to request host{i}"))?;

        assert_eq!(
            response.status().as_u16(),
            200,
            "Expected 200 for host{i}.local"
        );
    }

    // Remove all links concurrently
    let mut tasks = vec![];
    for i in 1..=10 {
        let client = ctl_client.clone();
        let server_id = http_server_id.clone();
        tasks.push(tokio::spawn(async move {
            assert_remove_link(&client, &server_id, "wasi", "http", &format!("link{i}")).await
        }));
    }

    for task in tasks {
        task.await??;
    }

    // Verify all links are removed with retry logic (concurrent deletions may take time)
    for i in 1..=10 {
        let mut attempts = 0;
        let max_attempts = 20; // 2 seconds total

        loop {
            let response = http_client
                .get(&base_url)
                .header("Host", format!("host{i}.local"))
                .send()
                .await
                .with_context(|| format!("Failed to request host{i} after delete"))?;

            if response.status().as_u16() == 404 {
                break; // Link successfully removed
            }

            attempts += 1;
            if attempts >= max_attempts {
                panic!("Expected 404 for host{i}.local after deletion, got {} after {attempts} attempts", response.status().as_u16());
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    Ok(())
}

/// Test rapid add/delete sequences to stress the async behavior
#[instrument(skip_all)]
#[tokio::test]
async fn test_rapid_link_changes() -> anyhow::Result<()> {
    let (nats_server, nats_url, nats_client) = start_nats(None, true)
        .await
        .context("failed to start NATS")?;

    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(
        nats_client.expect("failed to build nats client"),
    )
    .lattice(LATTICE.to_string())
    .build();

    let host = WasmCloudTestHost::start(&nats_url, LATTICE)
        .await
        .context("failed to start test host")?;

    let http_port = free_port().await?;
    let http_server_id = "http-server".to_string();

    assert_config_put(
        &ctl_client,
        &http_server_id,
        [
            (
                "default_address".to_string(),
                format!("{}:{http_port}", Ipv4Addr::LOCALHOST),
            ),
            ("routing_mode".to_string(), "host".to_string()),
        ],
    )
    .await?;

    assert_config_put(
        &ctl_client,
        "rapid_host",
        [("host".to_string(), "rapid.local".to_string())],
    )
    .await?;

    let host_key = host.host_key();
    assert_start_provider(StartProviderArgs {
        client: &ctl_client,
        host_id: &host_key.public_key(),
        provider_id: &http_server_id,
        provider_ref: BUILTIN_HTTP_SERVER,
        config: vec![http_server_id.clone()],
    })
    .await?;

    assert_scale_component(
        &ctl_client,
        host.host_key().public_key(),
        format!("file://{}", hello_world_component()),
        COMPONENT_ID,
        None,
        5,
        Vec::new(),
        Duration::from_secs(10),
    )
    .await?;

    // Perform rapid add/delete without waiting
    for _ in 0..10 {
        assert_advertise_link(
            &ctl_client,
            &http_server_id,
            COMPONENT_ID,
            "rapid",
            "wasi",
            "http",
            vec!["incoming-handler".to_string()],
            vec!["rapid_host".to_string()],
            vec![],
        )
        .await?;

        assert_remove_link(&ctl_client, &http_server_id, "wasi", "http", "rapid").await?;
    }

    // Final add
    assert_advertise_link(
        &ctl_client,
        &http_server_id,
        COMPONENT_ID,
        "rapid",
        "wasi",
        "http",
        vec!["incoming-handler".to_string()],
        vec!["rapid_host".to_string()],
        vec![],
    )
    .await?;

    // Wait for propagation
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify final state
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    let response = http_client
        .get(format!("http://localhost:{http_port}"))
        .header("Host", "rapid.local")
        .send()
        .await?;

    assert_eq!(
        response.status().as_u16(),
        200,
        "Link should work after rapid changes"
    );

    Ok(())
}

/// Test updating link configuration (delete old config, add new config)
#[instrument(skip_all)]
#[tokio::test]
async fn test_link_config_update() -> anyhow::Result<()> {
    let (nats_server, nats_url, nats_client) = start_nats(None, true)
        .await
        .context("failed to start NATS")?;

    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(
        nats_client.expect("failed to build nats client"),
    )
    .lattice(LATTICE.to_string())
    .build();

    let host = WasmCloudTestHost::start(&nats_url, LATTICE)
        .await
        .context("failed to start test host")?;

    let http_port = free_port().await?;
    let http_server_id = "http-server".to_string();

    assert_config_put(
        &ctl_client,
        &http_server_id,
        [
            (
                "default_address".to_string(),
                format!("{}:{http_port}", Ipv4Addr::LOCALHOST),
            ),
            ("routing_mode".to_string(), "host".to_string()),
        ],
    )
    .await?;

    // Create two different configs
    assert_config_put(
        &ctl_client,
        "config_v1",
        [("host".to_string(), "v1.local".to_string())],
    )
    .await?;

    assert_config_put(
        &ctl_client,
        "config_v2",
        [("host".to_string(), "v2.local".to_string())],
    )
    .await?;

    let host_key = host.host_key();
    assert_start_provider(StartProviderArgs {
        client: &ctl_client,
        host_id: &host_key.public_key(),
        provider_id: &http_server_id,
        provider_ref: BUILTIN_HTTP_SERVER,
        config: vec![http_server_id.clone()],
    })
    .await?;

    assert_scale_component(
        &ctl_client,
        host.host_key().public_key(),
        format!("file://{}", hello_world_component()),
        COMPONENT_ID,
        None,
        5,
        Vec::new(),
        Duration::from_secs(10),
    )
    .await?;

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    let base_url = format!("http://localhost:{http_port}");

    // Add link with config v1
    assert_advertise_link(
        &ctl_client,
        &http_server_id,
        COMPONENT_ID,
        "update_test",
        "wasi",
        "http",
        vec!["incoming-handler".to_string()],
        vec!["config_v1".to_string()],
        vec![],
    )
    .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify v1 works
    let response = http_client
        .get(&base_url)
        .header("Host", "v1.local")
        .send()
        .await?;
    assert_eq!(response.status().as_u16(), 200);

    // v2 should not work yet
    let response = http_client
        .get(&base_url)
        .header("Host", "v2.local")
        .send()
        .await?;
    assert_eq!(response.status().as_u16(), 404);

    // Update to config v2 (delete and re-add with new config)
    assert_remove_link(&ctl_client, &http_server_id, "wasi", "http", "update_test").await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_advertise_link(
        &ctl_client,
        &http_server_id,
        COMPONENT_ID,
        "update_test",
        "wasi",
        "http",
        vec!["incoming-handler".to_string()],
        vec!["config_v2".to_string()],
        vec![],
    )
    .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Now v2 should work
    let response = http_client
        .get(&base_url)
        .header("Host", "v2.local")
        .send()
        .await?;
    assert_eq!(
        response.status().as_u16(),
        200,
        "Config update should enable v2.local"
    );

    // v1 should not work anymore
    let response = http_client
        .get(&base_url)
        .header("Host", "v1.local")
        .send()
        .await?;
    assert_eq!(
        response.status().as_u16(),
        404,
        "Config update should disable v1.local"
    );

    Ok(())
}

/// Test that link operations work correctly when provider is stopped and restarted
#[instrument(skip_all)]
#[tokio::test]
async fn test_link_survives_provider_restart() -> anyhow::Result<()> {
    let (nats_server, nats_url, nats_client) = start_nats(None, true)
        .await
        .context("failed to start NATS")?;

    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(
        nats_client.expect("failed to build nats client"),
    )
    .lattice(LATTICE.to_string())
    .build();

    let host = WasmCloudTestHost::start(&nats_url, LATTICE)
        .await
        .context("failed to start test host")?;

    let http_port = free_port().await?;
    let http_server_id = "http-server".to_string();

    assert_config_put(
        &ctl_client,
        &http_server_id,
        [
            (
                "default_address".to_string(),
                format!("{}:{http_port}", Ipv4Addr::LOCALHOST),
            ),
            ("routing_mode".to_string(), "host".to_string()),
        ],
    )
    .await?;

    assert_config_put(
        &ctl_client,
        "restart_host",
        [("host".to_string(), "restart.local".to_string())],
    )
    .await?;

    let host_key = host.host_key();

    // Start provider
    assert_start_provider(StartProviderArgs {
        client: &ctl_client,
        host_id: &host_key.public_key(),
        provider_id: &http_server_id,
        provider_ref: BUILTIN_HTTP_SERVER,
        config: vec![http_server_id.clone()],
    })
    .await?;

    assert_scale_component(
        &ctl_client,
        host.host_key().public_key(),
        format!("file://{}", hello_world_component()),
        COMPONENT_ID,
        None,
        5,
        Vec::new(),
        Duration::from_secs(10),
    )
    .await?;

    // Add link
    assert_advertise_link(
        &ctl_client,
        &http_server_id,
        COMPONENT_ID,
        "restart_test",
        "wasi",
        "http",
        vec!["incoming-handler".to_string()],
        vec!["restart_host".to_string()],
        vec![],
    )
    .await?;

    tokio::time::sleep(Duration::from_millis(200)).await;

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    let base_url = format!("http://localhost:{http_port}");

    // Verify link works
    let response = http_client
        .get(&base_url)
        .header("Host", "restart.local")
        .send()
        .await?;
    assert_eq!(response.status().as_u16(), 200);

    // Stop provider
    assert_stop_provider(wasmcloud_test_util::provider::StopProviderArgs {
        client: &ctl_client,
        host_id: &host_key.public_key(),
        provider_id: &http_server_id,
    })
    .await?;

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Restart provider
    assert_start_provider(StartProviderArgs {
        client: &ctl_client,
        host_id: &host_key.public_key(),
        provider_id: &http_server_id,
        provider_ref: BUILTIN_HTTP_SERVER,
        config: vec![http_server_id.clone()],
    })
    .await?;

    // Wait for provider to start and links to be restored
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Link should be automatically restored from NATS store
    let response = timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(resp) = http_client
                .get(&base_url)
                .header("Host", "restart.local")
                .send()
                .await
            {
                if resp.status().as_u16() == 200 {
                    return Ok::<_, anyhow::Error>(resp);
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .context("Link should be restored after provider restart")??;

    assert_eq!(
        response.status().as_u16(),
        200,
        "Link should work after provider restart"
    );

    Ok(())
}
