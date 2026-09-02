//! A workload with several components that each import `wasi:config/store`.
//!
//! Every component of a workload reads the same workload-scoped interface
//! config layered with its own `LocalResources.config`, so a two-component
//! workload has to deliver two different views. The caller reports its own
//! view and the linked callee's in one response, which is what makes a
//! component reading someone else's config (or nothing at all) visible.

use anyhow::{Context, Result};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, Ingress},
    },
    plugin::wasi_config::DynamicConfig,
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

const CALLER_WASM: &[u8] = include_bytes!("wasm/config_caller.wasm");
const CALLEE_WASM: &[u8] = include_bytes!("wasm/config_callee.wasm");

/// Each component's own `LocalResources.config` (`who`) over the shared
/// workload-scoped entry.
const EXPECTED: &str = "caller[shared=yes;who=caller] callee[shared=yes;who=callee]";

fn config(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

/// A component carrying its own `LocalResources.config`. `warm` parks its
/// instances in a pool between calls instead of building a store per request.
fn component(name: &str, bytes: &'static [u8], own: &[(&str, &str)], warm: bool) -> Component {
    let (pool_size, max_invocations) = if warm { (2, 10_000) } else { (-1, -1) };
    Component {
        name: name.to_string(),
        digest: None,
        bytes: bytes::Bytes::from_static(bytes),
        local_resources: LocalResources {
            config: config(own),
            ..Default::default()
        },
        pool_size,
        max_invocations,
        ..Default::default()
    }
}

/// Runs the two-component workload with `host_interfaces` and returns the
/// caller's response body.
async fn run(host_interfaces: Vec<WitInterface>) -> Result<String> {
    run_with(host_interfaces, false, 1)
        .await
        .map(|mut bodies| bodies.remove(0))
}

/// Runs the workload and issues `requests` concurrent requests.
async fn run_with(
    host_interfaces: Vec<WitInterface>,
    warm: bool,
    requests: usize,
) -> Result<Vec<String>> {
    let engine = Engine::builder().build()?;
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();

    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(DynamicConfig::default()))?
        .build()?
        .start()
        .await
        .context("failed to start host")?;

    let started = host
        .workload_start(WorkloadStartRequest {
            workload_id: uuid::Uuid::new_v4().to_string(),
            workload: Workload {
                namespace: "test".to_string(),
                name: "multi-config".to_string(),
                annotations: HashMap::new(),
                service: None,
                components: vec![
                    component("caller", CALLER_WASM, &[("who", "caller")], warm),
                    component("callee", CALLEE_WASM, &[("who", "callee")], warm),
                ],
                host_interfaces,
                volumes: vec![],
            },
        })
        .await
        .context("failed to start workload")?;
    anyhow::ensure!(
        started.workload_status.message.contains("started"),
        "workload did not start: {:?}",
        started.workload_status
    );

    let client = reqwest::Client::new();
    let mut tasks = Vec::with_capacity(requests);
    for _ in 0..requests {
        let client = client.clone();
        let url = format!("http://{addr}/");
        tasks.push(tokio::spawn(async move {
            let response = timeout(
                Duration::from_secs(30),
                client.get(url).header("HOST", "test").send(),
            )
            .await
            .context("request timed out")?
            .context("request failed")?;
            let status = response.status();
            let body = response.text().await.context("failed to read body")?;
            anyhow::ensure!(status.is_success(), "request failed: {status} — {body}");
            Ok::<_, anyhow::Error>(body)
        }));
    }

    let mut bodies = Vec::with_capacity(requests);
    for task in tasks {
        bodies.push(task.await.context("request task panicked")??);
    }
    Ok(bodies)
}

fn http_interface() -> WitInterface {
    WitInterface {
        namespace: "wasi".to_string(),
        package: "http".to_string(),
        interfaces: ["incoming-handler".to_string()].into_iter().collect(),
        version: None,
        config: config(&[("host", "test")]),
        name: None,
    }
}

/// Both components read the same workload-scoped `wasi:config` entry, each
/// layered with its own `LocalResources.config`.
#[tokio::test]
async fn each_component_reads_its_own_config() -> Result<()> {
    let mut wasi_config = WitInterface::from("wasi:config/store@0.2.0-rc.1");
    wasi_config.config = config(&[("shared", "yes")]);

    let body = run(vec![http_interface(), wasi_config]).await?;
    assert_eq!(body, EXPECTED);
    Ok(())
}

/// The same workload with the `wasi:config` entry written as the package
/// alone — the form a manifest takes when it declares config without naming
/// the interface.
#[tokio::test]
async fn a_package_only_entry_still_delivers_config() -> Result<()> {
    let mut wasi_config = WitInterface::from("wasi:config");
    wasi_config.config = config(&[("shared", "yes")]);

    let body = run(vec![http_interface(), wasi_config]).await?;
    assert_eq!(body, EXPECTED);
    Ok(())
}

/// A versionless entry, as a hand-written manifest tends to spell it.
#[tokio::test]
async fn a_versionless_entry_delivers_config() -> Result<()> {
    let mut wasi_config = WitInterface::from("wasi:config/store");
    wasi_config.config = config(&[("shared", "yes")]);

    let body = run(vec![http_interface(), wasi_config]).await?;
    assert_eq!(body, EXPECTED);
    Ok(())
}

/// A workload that spreads one binding across two entries: both components read
/// one store holding both entries' keys, on every start.
#[tokio::test]
async fn config_spread_across_entries_reaches_every_component() -> Result<()> {
    let mut versioned = WitInterface::from("wasi:config/store@0.2.0-rc.1");
    versioned.config = config(&[("shared", "yes")]);
    let mut package_only = WitInterface::from("wasi:config");
    package_only.config = config(&[("extra", "1")]);

    // Repeated because the order under test comes from a `HashSet`: one run
    // proves nothing.
    for _ in 0..8 {
        let body = run(vec![
            http_interface(),
            versioned.clone(),
            package_only.clone(),
        ])
        .await?;
        assert_eq!(
            body,
            "caller[extra=1;shared=yes;who=caller] callee[extra=1;shared=yes;who=callee]"
        );
    }
    Ok(())
}

/// A name asks for one backend of a package. `wasi:config` serves a single
/// store, so a named entry beside the unnamed one is refused rather than
/// delivering one of the two under both names.
#[tokio::test]
async fn a_named_entry_beside_an_unnamed_one_is_refused() -> Result<()> {
    let mut unnamed = WitInterface::from("wasi:config/store@0.2.0-rc.1");
    unnamed.config = config(&[("shared", "yes")]);
    let mut named = WitInterface::from("wasi:config/store@0.2.0-rc.1");
    named.name = Some("extra".to_string());
    named.config = config(&[("other", "no")]);

    let err = match run(vec![http_interface(), unnamed, named]).await {
        Ok(body) => panic!("two bindings cannot share one store, got: {body}"),
        Err(e) => format!("{e:#}"),
    };
    assert!(
        err.contains("does not support named instances") && err.contains("extra"),
        "the refusal must name the binding that has nowhere to go, got: {err}"
    );
    Ok(())
}

/// Concurrent requests over warm, pooled instances keep each component's view
/// its own: the caller and the callee it links to share one store.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_requests_keep_each_view_its_own() -> Result<()> {
    let mut wasi_config = WitInterface::from("wasi:config/store@0.2.0-rc.1");
    wasi_config.config = config(&[("shared", "yes")]);

    let bodies = run_with(vec![http_interface(), wasi_config], true, 100).await?;
    assert_eq!(bodies.len(), 100);
    for body in bodies {
        assert_eq!(body, EXPECTED);
    }
    Ok(())
}
