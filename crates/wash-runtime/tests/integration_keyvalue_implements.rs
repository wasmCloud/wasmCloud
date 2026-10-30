#![cfg(feature = "wasm_component_model_implements")]
//! End-to-end `(implements ..)` named-import routing through a *real* guest.
//!
//! The `keyvalue-implements` fixture imports `wasi:keyvalue/store` **twice**
//! under the component-model labels `team-a` and `team-b` (the WIT `implements`
//! clause — `import team-a: wasi:keyvalue/store@…;`). On each request it opens a
//! bucket through each label, writes a distinct value to the same key, and reads
//! both back; it answers `isolated` only if neither write is visible through the
//! other label.
//!
//! The host binds a single [`MultiplexedKeyValue`] plugin. The workload declares
//! the two named interfaces `team-a` and `team-b`, each routed to an isolated
//! in-memory backend. The plugin's named-imports `add_to_linker` resolves each
//! label to its own backend, so the two stores must stay disjoint — proving the
//! implements id threads through `open` *and* the bucket resource methods
//! (`set`/`get`) to the correct backend.
//!
//! Unlike `integration_keyvalue_coexistence.rs` (where the named side is
//! declared host-side because the guest imported unnamed), here the *guest
//! itself* emits the `(implements ..)` imports.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::timeout;

use wash_runtime::{
    engine::Engine,
    host::{
        Host, HostApi, HostBuilder,
        http::{DevRouter, Ingress},
    },
    plugin::{
        PluginBindingSet, PluginBindings,
        wasi_keyvalue::{FilesystemProvider, InMemoryProvider, MultiplexedKeyValue},
    },
    types::{Component, LocalResources, Workload, WorkloadStartRequest, WorkloadState},
    wit::WitInterface,
};

mod common;
use common::http_incoming_handler_interface;

const KEYVALUE_IMPLEMENTS_WASM: &[u8] = include_bytes!("wasm/keyvalue_implements.wasm");

const KV_VERSION: &str = "0.2.0-draft";

/// A named `wasi:keyvalue/store` interface routed to an in-memory backend. The
/// `name` becomes the implements label the guest imports under (`team-a` /
/// `team-b`); `resolve` matches it against the component's import label.
fn named_store(name: &str) -> WitInterface {
    named_store_with(name, &[("backend", "in-memory")])
}

fn named_store_with(name: &str, config: &[(&str, &str)]) -> WitInterface {
    let config = config
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    WitInterface {
        namespace: "wasi".to_string(),
        package: "keyvalue".to_string(),
        interfaces: ["store".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse(KV_VERSION).unwrap()),
        config,
        name: Some(name.to_string()),
    }
}

/// A host serving the in-memory and filesystem backends under `bindings`, and
/// the address its HTTP ingress listens on.
async fn start_host(bindings: PluginBindings) -> Result<(Arc<Host>, SocketAddr)> {
    let engine = Engine::builder().build()?;
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();
    let multiplexed = MultiplexedKeyValue::new()
        .with_provider(Arc::new(InMemoryProvider))
        .with_provider(Arc::new(FilesystemProvider));
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(multiplexed))?
        .with_plugin_bindings(bindings)
        .build()?;
    let host = host.start().await.context("failed to start host")?;
    Ok((host, addr))
}

/// Start the fixture with `team-a` and `team-b` bound as `stores` describe.
/// `workload_start` returns Ok even on resolution failure, encoding the error
/// in the status, so the status is what callers assert on.
async fn start_workload(
    host: &Arc<Host>,
    stores: [WitInterface; 2],
) -> Result<wash_runtime::types::WorkloadStatus> {
    let mut host_interfaces = vec![http_incoming_handler_interface("kv-implements", None)];
    host_interfaces.extend(stores);
    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "keyvalue-implements".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "keyvalue-implements.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(KEYVALUE_IMPLEMENTS_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
                max_concurrency: 1,
                ..Default::default()
            }],
            host_interfaces,
            volumes: vec![],
        },
    };
    let resp = host
        .workload_start(req)
        .await
        .context("workload_start call failed")?;
    Ok(resp.workload_status)
}

/// The fixture's answer: `isolated` when neither label's write is visible
/// through the other.
async fn request_isolation(addr: SocketAddr) -> Result<String> {
    let response = timeout(
        Duration::from_secs(10),
        reqwest::Client::new()
            .get(format!("http://{addr}/"))
            .header("HOST", "kv-implements")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;
    let status = response.status();
    let body = response.text().await?;
    assert!(status.is_success(), "expected 200, got {status}: {body}");
    Ok(body)
}

fn is_empty_dir(dir: &Path) -> bool {
    std::fs::read_dir(dir).unwrap().next().is_none()
}

#[tokio::test]
async fn implements_imports_route_to_isolated_backends() -> Result<()> {
    // One multiplexed plugin serves both labeled imports; each named interface
    // defaults to its own isolated in-memory backend via `InMemoryProvider`.
    let (host, addr) = start_host(PluginBindings::new()).await?;

    // Success proves both labeled imports of the same external interface bound
    // on one linker.
    let status = start_workload(&host, [named_store("team-a"), named_store("team-b")]).await?;
    assert_eq!(
        status.workload_state,
        WorkloadState::Running,
        "workload should resolve: {}",
        status.message
    );

    // The guest writes `from-a` via team-a and `from-b` via team-b to the same
    // key, then reads both back. `isolated` means neither write leaked across the
    // label boundary — i.e. the implements id routed `open` and the bucket
    // resource methods to the correct backend.
    assert_eq!(
        request_isolation(addr).await?,
        "isolated",
        "each implements import must route to its own backend (no cross-label leak)"
    );

    Ok(())
}

#[tokio::test]
async fn deny_refuses_a_workload_that_roots_its_own_filesystem_backend() -> Result<()> {
    // Nothing declared, so the plugin resolves under the host default, `deny`.
    let (host, _) = start_host(PluginBindings::new()).await?;
    let outside = tempfile::tempdir()?;
    let root = outside.path().to_string_lossy().into_owned();

    let status = start_workload(
        &host,
        [
            named_store_with("team-a", &[("backend", "filesystem"), ("root", &root)]),
            named_store("team-b"),
        ],
    )
    .await?;
    assert_ne!(status.workload_state, WorkloadState::Running);
    assert!(
        status.message.contains("`root`"),
        "the refusal should name the key: {}",
        status.message
    );
    assert!(is_empty_dir(outside.path()), "nothing may be written there");

    Ok(())
}

#[tokio::test]
async fn an_operator_root_serves_a_workload_that_only_picks_the_backend() -> Result<()> {
    let team_a = tempfile::tempdir()?;
    let team_b = tempfile::tempdir()?;
    let root = |dir: &tempfile::TempDir| {
        HashMap::from([(
            "root".to_string(),
            dir.path().to_string_lossy().into_owned(),
        )])
    };
    let (host, addr) = start_host(
        PluginBindings::new().with_plugin(
            PluginBindingSet::new("wasi-keyvalue-multiplexed")
                .with_binding("team-a", root(&team_a))
                .with_binding("team-b", root(&team_b)),
        ),
    )
    .await?;

    let filesystem = [("backend", "filesystem")];
    let status = start_workload(
        &host,
        [
            named_store_with("team-a", &filesystem),
            named_store_with("team-b", &filesystem),
        ],
    )
    .await?;
    assert_eq!(
        status.workload_state,
        WorkloadState::Running,
        "workload should resolve: {}",
        status.message
    );

    assert_eq!(request_isolation(addr).await?, "isolated");
    assert!(!is_empty_dir(team_a.path()), "team-a writes under its root");
    assert!(!is_empty_dir(team_b.path()), "team-b writes under its root");

    Ok(())
}
