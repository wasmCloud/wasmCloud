#![cfg(feature = "wasm_component_model_implements")]
//! Coexistence of a standalone keyvalue plugin and a multiplexed
//! `(implements ..)` keyvalue plugin on one linker, exercised through a *real*
//! instantiated component.
//!
//! The component (`keyvalue-counter`) imports `wasi:keyvalue` once, unnamed,
//! and increments a counter per request. The host registers BOTH:
//!   * [`InMemoryKeyValue`] — the standalone plugin, whose default-instance
//!     `add_to_linker` serves the component's unnamed import; and
//!   * [`MultiplexedKeyValue`] — the multiplexed plugin, whose named-imports
//!     `add_to_linker` serves `(implements ..)` named imports.
//!
//! The workload declares the unnamed interface (routed to the standalone) plus
//! two *named* interfaces, `cache` and `sessions` (routed to the multiplexed
//! plugin). Both plugins therefore run their `add_to_linker` on the same linker
//! at instantiate time — the exact path that could collide if the default and
//! named instance registrations conflicted. `workload_start` succeeding proves
//! they coexist; the HTTP round-trip proves the unnamed import still routes to
//! (and persists in) the standalone backend.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::timeout;

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, HttpServer},
    },
    plugin::wasi_keyvalue::{InMemoryKeyValue, InMemoryProvider, MultiplexedKeyValue},
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

mod common;
use common::http_incoming_handler_interface;

const KEYVALUE_COUNTER_WASM: &[u8] = include_bytes!("wasm/keyvalue_counter.wasm");

const KV_VERSION: &str = "0.2.0-draft";

fn kv_interface(name: Option<&str>, backend: Option<&str>) -> WitInterface {
    let mut config = HashMap::new();
    if let Some(backend) = backend {
        config.insert("backend".to_string(), backend.to_string());
    }
    WitInterface {
        namespace: "wasi".to_string(),
        package: "keyvalue".to_string(),
        interfaces: ["store".to_string(), "atomics".to_string()]
            .into_iter()
            .collect(),
        version: Some(semver::Version::parse(KV_VERSION).unwrap()),
        config,
        name: name.map(String::from),
    }
}

#[tokio::test]
async fn standalone_and_multiplexed_keyvalue_coexist() -> Result<()> {
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = http_server.addr();

    // Both plugins on one host: the standalone serves the unnamed import, the
    // multiplexed serves the named ones. The multiplexed plugin defaults named
    // interfaces to an isolated in-memory backend via `InMemoryProvider`.
    let multiplexed = MultiplexedKeyValue::new().with_provider(Arc::new(InMemoryProvider));
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_server))
        .with_plugin(Arc::new(InMemoryKeyValue::new()))?
        .with_plugin(Arc::new(multiplexed))?
        .build()?;
    let host = host.start().await.context("failed to start host")?;

    // Unnamed kv (-> standalone) + two named kv interfaces (-> multiplexed). The
    // named entries force the multiplexed plugin to bind and run its named
    // `add_to_linker` alongside the standalone's default-instance one.
    let host_interfaces = vec![
        http_incoming_handler_interface("kv-coexist", None),
        kv_interface(None, None),
        kv_interface(Some("cache"), Some("in-memory")),
        kv_interface(Some("sessions"), Some("in-memory")),
    ];

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "keyvalue-coexist".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "keyvalue-counter.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(KEYVALUE_COUNTER_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces,
            volumes: vec![],
        },
    };

    // Instantiation runs BOTH plugins' `add_to_linker` on the same linker.
    // Success here is the core coexistence assertion: the default-instance and
    // named-instance registrations don't conflict.
    host.workload_start(req)
        .await
        .context("standalone + multiplexed keyvalue plugins should bind together")?;

    // The unnamed import must route to the standalone backend and persist there
    // across requests (the counter increments 1 -> 2), unaffected by the
    // multiplexed plugin sharing the linker.
    let client = reqwest::Client::new();
    for expected in ["1", "2"] {
        let response = timeout(
            Duration::from_secs(10),
            client
                .get(format!("http://{addr}/"))
                .header("HOST", "kv-coexist")
                .send(),
        )
        .await
        .context("request timed out")?
        .context("request failed")?;

        let status = response.status();
        let body = response.text().await?;
        assert!(status.is_success(), "expected 200, got {status}: {body}");
        assert_eq!(
            body, expected,
            "unnamed keyvalue import should route to the standalone backend and persist"
        );
    }

    Ok(())
}
