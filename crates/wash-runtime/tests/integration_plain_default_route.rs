#![cfg(feature = "wasm_component_model_implements")]
//! A plain (unlabeled) `wasi:keyvalue` / `wasi:blobstore` import served by the
//! SYNC multiplexer through the workload's default (`""`) route, exercised
//! through real instantiated components.
//!
//! Every host here registers BOTH the standalone plugin (which serves plain
//! imports today) and the multiplexer with a filesystem backend rooted at a
//! temp dir, so the temp dir is the witness: it fills only when the plain
//! import was served by the multiplexer.
//!
//! * no route → the standalone serves the plain import, exactly as before;
//! * a declared unnamed binding → the multiplexer serves it, beside labels;
//! * an operator base (the entry's own `config`) naming a `backend` → the
//!   multiplexer serves it;
//! * a plain entry naming its own `backend` → the multiplexer serves it, for
//!   the workload that named it only.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::timeout;

use wash_runtime::{
    engine::Engine,
    host::{
        Host, HostApi, HostBuilder,
        http::{DynamicRouter, Ingress},
    },
    plugin::{
        PluginBindingSet, PluginBindings,
        bindings::UNNAMED_BINDING,
        wasi_blobstore::{
            FilesystemProvider as BlobFilesystemProvider, InMemoryBlobstore,
            InMemoryProvider as BlobInMemoryProvider, MultiplexedBlobstore,
        },
        wasi_keyvalue::{
            FilesystemProvider as KvFilesystemProvider, InMemoryKeyValue,
            InMemoryProvider as KvInMemoryProvider, MultiplexedKeyValue,
        },
    },
    types::{Component, LocalResources, Workload, WorkloadStartRequest, WorkloadState},
    wit::{Config, WitInterface},
};

mod common;
use common::http_incoming_handler_interface;

const KEYVALUE_COUNTER_WASM: &[u8] = include_bytes!("wasm/keyvalue_counter.wasm");
const HTTP_BLOBSTORE_WASM: &[u8] = include_bytes!("wasm/http_blobstore.wasm");

/// The bucket and key `keyvalue-counter` increments, as the filesystem backend
/// lays them out under its root.
const COUNTER_FILE: [&str; 2] = ["counter", "n"];
/// The container and object `http-blobstore` writes.
const BLOB_FILE: [&str; 2] = ["my-container-real", "hi.txt"];

const KEYVALUE_ID: &str = "wasi-keyvalue-multiplexed";
const BLOBSTORE_ID: &str = "wasi-blobstore-multiplexed";

fn config(pairs: &[(&str, &str)]) -> Config {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "wash-plain-route-{tag}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn kv_entry(name: Option<&str>, config: Config) -> WitInterface {
    WitInterface {
        namespace: "wasi".to_string(),
        package: "keyvalue".to_string(),
        interfaces: ["store".to_string(), "atomics".to_string()]
            .into_iter()
            .collect(),
        version: Some(semver::Version::parse("0.2.0-draft").unwrap()),
        config,
        name: name.map(String::from),
    }
}

fn blob_entry(config: Config) -> WitInterface {
    WitInterface {
        namespace: "wasi".to_string(),
        package: "blobstore".to_string(),
        interfaces: [
            "blobstore".to_string(),
            "container".to_string(),
            "types".to_string(),
        ]
        .into_iter()
        .collect(),
        version: Some(semver::Version::parse("0.2.0-draft").unwrap()),
        config,
        name: None,
    }
}

/// A host with the standalone in-memory keyvalue plugin AND the multiplexer
/// (in-memory + filesystem backends).
async fn kv_host(bindings: PluginBindings) -> Result<(Arc<Host>, SocketAddr)> {
    let engine = Engine::builder().build()?;
    let ingress = Ingress::new(DynamicRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();
    let multiplexed = MultiplexedKeyValue::new()
        .with_provider(Arc::new(KvInMemoryProvider))
        .with_provider(Arc::new(KvFilesystemProvider));
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(InMemoryKeyValue::new()))?
        .with_plugin(Arc::new(multiplexed))?
        .with_plugin_bindings(bindings)
        .build()?;
    Ok((host.start().await.context("failed to start host")?, addr))
}

/// A host with the standalone in-memory blobstore plugin AND the multiplexer
/// (in-memory + filesystem backends).
async fn blob_host(bindings: PluginBindings) -> Result<(Arc<Host>, SocketAddr)> {
    let engine = Engine::builder().build()?;
    let ingress = Ingress::new(DynamicRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();
    let multiplexed = MultiplexedBlobstore::new()
        .with_provider(Arc::new(BlobInMemoryProvider))
        .with_provider(Arc::new(BlobFilesystemProvider));
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(InMemoryBlobstore::new(None)))?
        .with_plugin(Arc::new(multiplexed))?
        .with_plugin_bindings(bindings)
        .build()?;
    Ok((host.start().await.context("failed to start host")?, addr))
}

/// Start `wasm` behind HTTP host `http_host` with `entries`, and require it to
/// be running (`workload_start` reports a bind failure in the status).
async fn start(
    host: &Arc<Host>,
    http_host: &str,
    wasm: &'static [u8],
    entries: Vec<WitInterface>,
) -> Result<()> {
    let mut host_interfaces = vec![http_incoming_handler_interface(http_host, None)];
    host_interfaces.extend(entries);
    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: http_host.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: format!("{http_host}.wasm"),
                digest: None,
                bytes: bytes::Bytes::from_static(wasm),
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
    let status = host
        .workload_start(req)
        .await
        .context("workload_start call failed")?
        .workload_status;
    assert_eq!(
        status.workload_state,
        WorkloadState::Running,
        "workload {http_host} should bind: {status:?}"
    );
    Ok(())
}

async fn request(addr: SocketAddr, http_host: &str, body: Option<&str>) -> Result<String> {
    let client = reqwest::Client::new();
    let builder = match body {
        Some(body) => client
            .post(format!("http://{addr}/"))
            .body(body.to_string()),
        None => client.get(format!("http://{addr}/")),
    };
    let response = timeout(
        Duration::from_secs(10),
        builder.header("HOST", http_host).send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;
    let status = response.status();
    let text = response.text().await?;
    assert!(status.is_success(), "expected 200, got {status}: {text}");
    Ok(text)
}

fn file(root: &Path, parts: [&str; 2]) -> PathBuf {
    root.join(parts[0]).join(parts[1])
}

fn is_empty_dir(dir: &Path) -> bool {
    std::fs::read_dir(dir).unwrap().next().is_none()
}

/// No route anywhere: the plain import stays with the standalone plugin, the
/// way it bound before default routes existed — even beside a labeled entry,
/// which still binds the multiplexer on the same linker.
#[tokio::test]
async fn plain_keyvalue_without_a_route_stays_with_the_standalone() -> Result<()> {
    let root = temp_root("kv-none");
    let declared = PluginBindings::new().with_plugin(
        PluginBindingSet::new(KEYVALUE_ID)
            .with_binding("cache", config(&[("backend", "in-memory")])),
    );
    let (host, addr) = kv_host(declared).await?;
    start(
        &host,
        "kv-none",
        KEYVALUE_COUNTER_WASM,
        vec![
            kv_entry(None, Config::new()),
            kv_entry(Some("cache"), Config::new()),
        ],
    )
    .await?;

    for expected in ["1", "2"] {
        assert_eq!(request(addr, "kv-none", None).await?, expected);
    }
    assert!(
        is_empty_dir(&root),
        "no route was declared, so the multiplexer must not have served the plain import"
    );
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

/// An unnamed binding the operator declared on the multiplexer is the plain
/// import's route: the counter lands in that binding's backend, and a labeled
/// entry beside it still binds on the same linker without a clash.
#[tokio::test]
async fn plain_keyvalue_follows_a_declared_unnamed_binding() -> Result<()> {
    let root = temp_root("kv-declared");
    let root_str = root.to_string_lossy().to_string();
    let declared = PluginBindings::new().with_plugin(
        PluginBindingSet::new(KEYVALUE_ID)
            .with_binding(
                UNNAMED_BINDING,
                config(&[("backend", "filesystem"), ("root", &root_str)]),
            )
            .with_binding("cache", config(&[("backend", "in-memory")])),
    );
    let (host, addr) = kv_host(declared).await?;
    start(
        &host,
        "kv-declared",
        KEYVALUE_COUNTER_WASM,
        vec![
            kv_entry(None, Config::new()),
            kv_entry(Some("cache"), Config::new()),
        ],
    )
    .await?;

    for expected in ["1", "2"] {
        assert_eq!(request(addr, "kv-declared", None).await?, expected);
    }
    let counter = file(&root, COUNTER_FILE);
    assert!(
        counter.exists(),
        "the plain import must have been served by the declared unnamed binding"
    );
    assert_eq!(std::fs::read_to_string(&counter)?.trim(), "2");
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

/// A plain entry naming its own `backend` is a route: the entry's backend
/// serves it, over an operator base that supplies the host-owned `root` but no
/// backend of its own. (Before default routes, the standalone served it and the
/// `backend` was never read.)
#[tokio::test]
async fn a_plain_entry_naming_a_backend_routes_to_the_multiplexer() -> Result<()> {
    let root = temp_root("kv-entry");
    let declared = PluginBindings::new().with_plugin(
        PluginBindingSet::new(KEYVALUE_ID).with_base(config(&[("root", &root.to_string_lossy())])),
    );
    let (host, addr) = kv_host(declared).await?;
    start(
        &host,
        "kv-entry",
        KEYVALUE_COUNTER_WASM,
        vec![kv_entry(None, config(&[("backend", "filesystem")]))],
    )
    .await?;

    for expected in ["1", "2"] {
        assert_eq!(request(addr, "kv-entry", None).await?, expected);
    }
    assert!(
        file(&root, COUNTER_FILE).exists(),
        "the entry's own backend must serve the plain import"
    );
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

/// An operator base — what `host.plugins` calls the entry's own `config`, the
/// unnamed binding's layer — naming a `backend` is a route for every plain
/// import on the host, beside a label that inherits the same base.
#[tokio::test]
async fn an_operator_base_naming_a_backend_routes_plain_imports() -> Result<()> {
    let root = temp_root("kv-base");
    let declared = PluginBindings::new().with_plugin(
        PluginBindingSet::new(KEYVALUE_ID)
            .with_base(config(&[
                ("backend", "filesystem"),
                ("root", &root.to_string_lossy()),
            ]))
            .with_binding("cache", config(&[("backend", "in-memory")])),
    );
    let (host, addr) = kv_host(declared).await?;
    start(
        &host,
        "kv-base",
        KEYVALUE_COUNTER_WASM,
        vec![
            kv_entry(None, Config::new()),
            kv_entry(Some("cache"), Config::new()),
        ],
    )
    .await?;

    for expected in ["1", "2"] {
        assert_eq!(request(addr, "kv-base", None).await?, expected);
    }
    assert_eq!(
        std::fs::read_to_string(file(&root, COUNTER_FILE))?.trim(),
        "2",
        "the base's backend must serve the plain import"
    );
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

/// A route a workload wrote is that workload's alone: a second workload on the
/// same host with a plain entry naming no backend keeps the standalone, and the
/// first workload's counter is not shared with it.
#[tokio::test]
async fn a_workload_route_is_not_another_workloads_default() -> Result<()> {
    let root = temp_root("kv-per-workload");
    let declared = PluginBindings::new().with_plugin(
        PluginBindingSet::new(KEYVALUE_ID).with_base(config(&[("root", &root.to_string_lossy())])),
    );
    let (host, addr) = kv_host(declared).await?;
    start(
        &host,
        "kv-routed",
        KEYVALUE_COUNTER_WASM,
        vec![kv_entry(None, config(&[("backend", "filesystem")]))],
    )
    .await?;
    start(
        &host,
        "kv-plain",
        KEYVALUE_COUNTER_WASM,
        vec![kv_entry(None, Config::new())],
    )
    .await?;

    for expected in ["1", "2"] {
        assert_eq!(request(addr, "kv-routed", None).await?, expected);
    }
    assert_eq!(
        request(addr, "kv-plain", None).await?,
        "1",
        "the unrouted workload must keep its own standalone store"
    );
    assert_eq!(
        std::fs::read_to_string(file(&root, COUNTER_FILE))?.trim(),
        "2",
        "only the routed workload's increments reach the multiplexer's backend"
    );
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}

/// The blobstore half: without a route the standalone serves the plain
/// import; with a declared unnamed binding the multiplexer's filesystem
/// backend does (there is no standalone S3 plugin, so this is the only way a
/// plain `wasi:blobstore` import can reach S3).
#[tokio::test]
async fn plain_blobstore_follows_a_declared_unnamed_binding() -> Result<()> {
    for routed in [false, true] {
        let root = temp_root("blob");
        let mut set = PluginBindingSet::new(BLOBSTORE_ID);
        if routed {
            set = set.with_binding(
                UNNAMED_BINDING,
                config(&[("backend", "filesystem"), ("root", &root.to_string_lossy())]),
            );
        }
        let (host, addr) = blob_host(PluginBindings::new().with_plugin(set)).await?;
        start(
            &host,
            "blob",
            HTTP_BLOBSTORE_WASM,
            vec![blob_entry(Config::new())],
        )
        .await?;

        let body = "hello, plain blobstore";
        assert_eq!(request(addr, "blob", Some(body)).await?.trim(), body);
        assert_eq!(
            file(&root, BLOB_FILE).exists(),
            routed,
            "the multiplexer must serve the plain import {}",
            if routed {
                "through the declared unnamed binding"
            } else {
                "only when a route is declared"
            },
        );
        let _ = std::fs::remove_dir_all(&root);
    }
    Ok(())
}
