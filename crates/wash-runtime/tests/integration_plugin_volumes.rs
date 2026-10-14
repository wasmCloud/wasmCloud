//! Integration test: a host component plugin's `volumes` — host directories
//! preopened into the plugin's own store.
//!
//! `volume-plugin` reads and writes files with plain `std::fs`. A plugin store
//! has no filesystem of its own, so everything the plugin can reach is what its
//! volumes preopened, with the permissions they were declared with. The plugin
//! exports a bespoke `acme:volume/files` capability reporting each outcome as a
//! string, driven end to end over HTTP through the `volume-plugin-caller`
//! workload.

#![cfg(feature = "host-component-plugins")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::timeout;

use wash_runtime::engine::Engine;
use wash_runtime::host::http::{DevRouter, Ingress};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::plugin::PluginVolume;
use wash_runtime::plugin::component_host::ComponentHostPlugin;
use wash_runtime::types::LocalResources;
use wash_runtime::wit::WitInterface;

mod common;
use common::{component_workload_request, http_incoming_handler_interface};

const VOLUME_PLUGIN_WASM: &[u8] = include_bytes!("wasm/volume_plugin.wasm");
const CALLER_WASM: &[u8] = include_bytes!("wasm/volume_plugin_caller.wasm");
const PLUGIN_ID: &str = "volume-plugin";

fn files_interface() -> WitInterface {
    WitInterface {
        namespace: "acme".to_string(),
        package: "volume".to_string(),
        interfaces: ["files".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.1.0").unwrap()),
        config: HashMap::new(),
        name: None,
    }
}

fn volume(host_path: &Path, mount_path: &str, read_only: bool) -> PluginVolume {
    PluginVolume {
        host_path: host_path.to_path_buf(),
        mount_path: mount_path.to_string(),
        read_only,
    }
}

/// Starts a host with `volume-plugin` registered with `volumes`, plus the
/// caller workload wired up to drive it.
async fn start_host_with_volumes(
    volumes: Vec<PluginVolume>,
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let ingress = Ingress::builder(DevRouter::default(), "127.0.0.1:0".parse()?)
        .build()
        .await?;
    let addr = ingress.addr();

    let builder = HostBuilder::new()
        .with_engine(engine.clone())
        .with_http_handler(Arc::new(ingress));
    let native_plugins = builder.native_plugins();
    let host_ref = builder.host_ref();

    let plugin = ComponentHostPlugin::builder()
        .id(PLUGIN_ID)
        .wasm(VOLUME_PLUGIN_WASM)
        .engine(engine)
        .native_plugins(native_plugins)
        .volumes(volumes.into())
        .maybe_host_ref(Some(host_ref))
        .build()
        .await
        .context("volume-plugin should link cleanly")?;

    let host = builder.with_plugin(Arc::new(plugin))?.build()?;
    let host = host.start().await.context("failed to start host")?;

    host.workload_start(component_workload_request(
        "volume-plugin-caller",
        "caller",
        CALLER_WASM,
        LocalResources::default(),
        vec![
            http_incoming_handler_interface("caller", None),
            files_interface(),
        ],
    ))
    .await?;

    Ok((addr, host))
}

async fn get(client: &reqwest::Client, addr: &std::net::SocketAddr, path: &str) -> Result<String> {
    let resp = timeout(
        Duration::from_secs(15),
        client
            .get(format!("http://{addr}{path}"))
            .header("HOST", "caller")
            .send(),
    )
    .await
    .context("request timed out")??;
    Ok(resp.text().await?)
}

/// A file in a volume is readable at the volume's mount path.
#[tokio::test]
async fn test_plugin_reads_a_file_from_its_volume() -> Result<()> {
    let models = tempfile::tempdir()?;
    std::fs::write(models.path().join("hello.txt"), "hello from the host")?;
    let (addr, _host) =
        start_host_with_volumes(vec![volume(models.path(), "/models", true)]).await?;
    let client = reqwest::Client::new();

    let outcome = get(&client, &addr, "/read?path=/models/hello.txt").await?;
    assert_eq!(outcome, "ok:hello from the host");

    Ok(())
}

/// A read-only volume refuses writes, and nothing reaches the host directory.
#[tokio::test]
async fn test_plugin_read_only_volume_refuses_writes() -> Result<()> {
    let models = tempfile::tempdir()?;
    let (addr, _host) =
        start_host_with_volumes(vec![volume(models.path(), "/models", true)]).await?;
    let client = reqwest::Client::new();

    let outcome = get(&client, &addr, "/write?path=/models/new.txt&contents=x").await?;
    assert!(
        outcome.starts_with("error"),
        "a write to a read-only volume must fail, got {outcome:?}"
    );
    assert!(!models.path().join("new.txt").exists());

    Ok(())
}

/// A read-write volume's writes land in the host directory.
#[tokio::test]
async fn test_plugin_read_write_volume_writes_reach_the_host() -> Result<()> {
    let cache = tempfile::tempdir()?;
    let (addr, _host) =
        start_host_with_volumes(vec![volume(cache.path(), "/cache", false)]).await?;
    let client = reqwest::Client::new();

    let outcome = get(
        &client,
        &addr,
        "/write?path=/cache/out.txt&contents=written",
    )
    .await?;
    assert_eq!(outcome, "ok");
    assert_eq!(
        std::fs::read_to_string(cache.path().join("out.txt"))?,
        "written"
    );

    Ok(())
}

/// Nothing outside the volumes is reachable: not another host path, and not
/// one reached by climbing out of a volume.
#[tokio::test]
async fn test_plugin_cannot_reach_outside_its_volumes() -> Result<()> {
    let root = tempfile::tempdir()?;
    let models = root.path().join("models");
    std::fs::create_dir(&models)?;
    std::fs::write(root.path().join("secret.txt"), "not for plugins")?;
    let (addr, _host) = start_host_with_volumes(vec![volume(&models, "/models", true)]).await?;
    let client = reqwest::Client::new();

    for path in ["/models/../secret.txt", "/secret.txt", "/etc/passwd"] {
        let outcome = get(&client, &addr, &format!("/read?path={path}")).await?;
        assert!(
            outcome.starts_with("error"),
            "{path} must be unreachable, got {outcome:?}"
        );
    }

    Ok(())
}

/// A plugin declared without volumes has no filesystem at all — what every
/// plugin had before volumes existed.
#[tokio::test]
async fn test_plugin_without_volumes_has_no_filesystem() -> Result<()> {
    let (addr, _host) = start_host_with_volumes(vec![]).await?;
    let client = reqwest::Client::new();

    let outcome = get(&client, &addr, "/read?path=/models/hello.txt").await?;
    assert!(
        outcome.starts_with("error"),
        "a plugin without volumes must reach no files, got {outcome:?}"
    );

    Ok(())
}
