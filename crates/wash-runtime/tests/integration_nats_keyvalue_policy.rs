#![cfg(feature = "wasm_component_model_implements")]
//! End-to-end bucket policy: a **real guest** opening a store over HTTP, on a
//! host backed by a **real NATS JetStream**.
//!
//! The unit and backend tests exercise the policy through Rust APIs. These go
//! the whole way — guest `wasi:keyvalue/store.open` (and the async
//! `wasmcloud:keyvalue` equivalent) through the host bindings, the plugin, the
//! policy, and JetStream — because that is the path an operator's flags
//! actually govern:
//!
//! * `create = never` must reach the guest as an error, not a created bucket;
//! * `create = missing` must create the *prefixed* bucket and nothing else;
//! * a bucket that already exists must open under `never` on a later host,
//!   which is the upgrade path for a deployment that pre-creates its buckets;
//! * `wasmcloud:keyvalue` must behave identically, since it shares the
//!   providers.
//!
//! Requires Docker (NATS with JetStream); marked `#[ignore]`, so it runs under
//! `cargo test --include-ignored` (CI's Linux leg) and not a plain `cargo test`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::time::timeout;

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, Ingress},
    },
    plugin::wasi_keyvalue::{
        BucketPolicy, CreatePolicy, MultiplexedAsyncKeyValue, NatsKeyValue, NatsProvider,
    },
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

mod common;
use common::http_incoming_handler_interface;

/// Opens `wasi:keyvalue` bucket "counter" and returns the incremented count.
const KEYVALUE_COUNTER_WASM: &[u8] = include_bytes!("wasm/keyvalue_counter.wasm");
/// Opens async `wasmcloud:keyvalue` bucket "default-kv" and echoes a value.
const KEYVALUE_DEFAULT_P3_WASM: &[u8] = include_bytes!("wasm/keyvalue_default_p3.wasm");

/// The prefix under test: every bucket these hosts touch must land under it.
const PREFIX: &str = "e2e-";

async fn start_jetstream() -> Result<(ContainerAsync<GenericImage>, String)> {
    let container = GenericImage::new("nats", "2.12.8-alpine")
        .with_exposed_port(4222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .with_cmd(["-js"])
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start nats: {e}"))?;
    let port = container.get_host_port_ipv4(4222).await?;
    Ok((container, format!("nats://127.0.0.1:{port}")))
}

fn wasi_kv_iface() -> WitInterface {
    WitInterface {
        namespace: "wasi".to_string(),
        package: "keyvalue".to_string(),
        interfaces: ["store".to_string(), "atomics".to_string()]
            .into_iter()
            .collect(),
        version: Some(semver::Version::parse("0.2.0-draft").unwrap()),
        config: HashMap::new(),
        name: None,
    }
}

/// An unnamed async `wasmcloud:keyvalue` interface pointed at NATS: the
/// workload's default route, so the guest's plain import lands on it.
fn wasmcloud_kv_iface(url: &str) -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "keyvalue".to_string(),
        interfaces: ["store".to_string(), "atomics".to_string()]
            .into_iter()
            .collect(),
        version: Some(semver::Version::parse("0.2.0").unwrap()),
        config: HashMap::from([
            ("backend".to_string(), "nats".to_string()),
            ("url".to_string(), url.to_string()),
        ]),
        name: None,
    }
}

fn workload(
    name: &str,
    wasm: &'static [u8],
    host_interfaces: Vec<WitInterface>,
) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: name.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: format!("{name}.wasm"),
                digest: None,
                bytes: bytes::Bytes::from_static(wasm),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
                max_concurrency: 1,
            }],
            host_interfaces,
            volumes: vec![],
        },
    }
}

/// One request to a host's DevRouter, returning the status and body.
async fn request(addr: std::net::SocketAddr, host_header: &str) -> Result<(u16, String)> {
    let response = timeout(
        Duration::from_secs(15),
        reqwest::Client::new()
            .get(format!("http://{addr}/"))
            .header("HOST", host_header)
            .send(),
    )
    .await
    .context("request timed out")??;
    let status = response.status().as_u16();
    Ok((status, response.text().await?))
}

/// A host serving `wasi:keyvalue` from NATS under `policy`.
async fn start_wasi_kv_host(
    url: &str,
    policy: BucketPolicy,
    host_header: &str,
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let client = async_nats::connect(url).await?;
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();
    let host = HostBuilder::new()
        .with_engine(Engine::builder().build()?)
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(NatsKeyValue::with_bucket_policy(&client, policy)))?
        .build()?
        .start()
        .await
        .context("failed to start host")?;

    host.workload_start(workload(
        "keyvalue-counter",
        KEYVALUE_COUNTER_WASM,
        vec![
            http_incoming_handler_interface(host_header, None),
            wasi_kv_iface(),
        ],
    ))
    .await
    .context("failed to start keyvalue-counter workload")?;

    Ok((addr, host))
}

/// The whole policy, end to end, through a guest's `wasi:keyvalue` import.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn guest_open_honors_bucket_policy_over_jetstream() -> Result<()> {
    let (_container, url) = start_jetstream().await?;
    let js = async_nats::jetstream::new(async_nats::connect(&url).await?);

    // --- create = never: the guest's open fails, and nothing is created ---
    {
        let (addr, host) = start_wasi_kv_host(&url, BucketPolicy::default(), "kv-never").await?;
        let (status, body) = request(addr, "kv-never").await?;
        assert_eq!(
            status, 500,
            "a host that does not create buckets must surface the failure to the guest, got body: {body}"
        );
        // The failure must be the policy refusing the open, not some unrelated
        // fault dressed up as a 500.
        assert!(
            body.contains("open") && body.contains("NoSuchStore"),
            "expected a no-such-store on open, got: {body}"
        );
        js.get_key_value("counter")
            .await
            .expect_err("no bucket may be created under create = never");
        drop(host);
    }

    // --- create = missing + prefix: the prefixed bucket is created and used ---
    let creating = BucketPolicy {
        prefix: PREFIX.to_string(),
        create: CreatePolicy::Missing,
        ..BucketPolicy::default()
    };
    {
        let (addr, host) = start_wasi_kv_host(&url, creating.clone(), "kv-missing").await?;

        let (status, body) = request(addr, "kv-missing").await?;
        assert_eq!(status, 200, "expected a counter, got: {body}");
        assert_eq!(body, "1");
        let (_, body) = request(addr, "kv-missing").await?;
        assert_eq!(body, "2", "the counter must persist in JetStream");

        js.get_key_value("e2e-counter")
            .await
            .context("the prefixed bucket must exist")?;
        js.get_key_value("counter")
            .await
            .expect_err("the unprefixed name must not have been created");
        drop(host);
    }

    // --- a later host that creates nothing still opens the existing bucket,
    // and continues the same counter: the upgrade path for a deployment whose
    // buckets are pre-created ---
    {
        let strict = BucketPolicy {
            prefix: PREFIX.to_string(),
            ..BucketPolicy::default()
        };
        let (addr, host) = start_wasi_kv_host(&url, strict, "kv-existing").await?;
        let (status, body) = request(addr, "kv-existing").await?;
        assert_eq!(
            status, 200,
            "an existing bucket must open under create = never, got: {body}"
        );
        assert_eq!(body, "3", "the counter must continue in the same bucket");
        drop(host);
    }

    Ok(())
}

/// The path a CLI actually takes: the embedder's policy handed to
/// [`HostBuilder::with_multiplexed_plugins_with`], rather than a provider
/// constructed by hand.
///
/// `wash dev` and `wash host` register the multiplexed set this way, so this
/// is what proves their `--keyvalue-nats-*` flags and `dev.wasi_keyvalue_nats`
/// block reach an `(implements ..)` import at all.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn multiplexed_plugin_set_inherits_embedder_policy() -> Result<()> {
    let (_container, url) = start_jetstream().await?;
    let js = async_nats::jetstream::new(async_nats::connect(&url).await?);

    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();
    let host = HostBuilder::new()
        .with_engine(Engine::builder().build()?)
        .with_http_handler(Arc::new(ingress))
        .with_multiplexed_plugins_with(
            &wash_runtime::plugin::MultiplexedDefaults::default().with_keyvalue_nats_bucket(
                BucketPolicy {
                    prefix: PREFIX.to_string(),
                    create: CreatePolicy::Missing,
                    ..BucketPolicy::default()
                },
            ),
        )?
        .build()?
        .start()
        .await
        .context("failed to start host")?;

    // The interface names its backend and URL; everything about *which* bucket
    // and whether it may be created comes from the embedder.
    host.workload_start(workload(
        "keyvalue-default-p3",
        KEYVALUE_DEFAULT_P3_WASM,
        vec![
            http_incoming_handler_interface("builder-defaults", None),
            wasmcloud_kv_iface(&url),
        ],
    ))
    .await
    .context("failed to start keyvalue-default-p3 workload")?;

    let (status, body) = request(addr, "builder-defaults").await?;
    assert_eq!(status, 200, "expected the guest's value, got: {body}");
    js.get_key_value("e2e-default-kv")
        .await
        .context("the embedder's prefix must have reached the multiplexed plugin set")?;
    js.get_key_value("default-kv")
        .await
        .expect_err("the unprefixed name must not have been created");
    drop(host);

    Ok(())
}

/// The same policy, through a real async `wasmcloud:keyvalue` guest: the two
/// packages share the providers, so an operator's policy must govern both.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn async_wasmcloud_keyvalue_guest_honors_bucket_policy() -> Result<()> {
    let (_container, url) = start_jetstream().await?;
    let js = async_nats::jetstream::new(async_nats::connect(&url).await?);

    let start = |policy: BucketPolicy, host_header: &'static str| {
        let url = url.clone();
        async move {
            let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
            let addr = ingress.addr();
            let host = HostBuilder::new()
                .with_engine(Engine::builder().build()?)
                .with_http_handler(Arc::new(ingress))
                .with_plugin(Arc::new(
                    MultiplexedAsyncKeyValue::new()
                        .with_provider(Arc::new(NatsProvider::with_defaults(policy))),
                ))?
                .build()?
                .start()
                .await
                .context("failed to start host")?;
            host.workload_start(workload(
                "keyvalue-default-p3",
                KEYVALUE_DEFAULT_P3_WASM,
                vec![
                    http_incoming_handler_interface(host_header, None),
                    wasmcloud_kv_iface(&url),
                ],
            ))
            .await
            .context("failed to start keyvalue-default-p3 workload")?;
            anyhow::Ok((addr, host))
        }
    };

    // A host that withholds creation refuses the guest's open.
    let (addr, host) = start(BucketPolicy::default(), "wasmcloud-kv-never").await?;
    let (status, body) = request(addr, "wasmcloud-kv-never").await?;
    assert_eq!(
        status, 500,
        "create = never must reach an async guest as an error, got body: {body}"
    );
    assert!(
        body.contains("open") && body.contains("NoSuchStore"),
        "expected a no-such-store on open, got: {body}"
    );
    js.get_key_value("default-kv")
        .await
        .expect_err("no bucket may be created under create = never");
    drop(host);

    // One that allows it creates the prefixed bucket, exactly as the
    // `wasi:keyvalue` path does.
    let (addr, host) = start(
        BucketPolicy {
            prefix: PREFIX.to_string(),
            create: CreatePolicy::Missing,
            ..BucketPolicy::default()
        },
        "wasmcloud-kv-missing",
    )
    .await?;
    let (status, body) = request(addr, "wasmcloud-kv-missing").await?;
    assert_eq!(status, 200, "expected the guest's value, got: {body}");
    assert_eq!(body, "woof from a plain p3 keyvalue guest");
    js.get_key_value("e2e-default-kv")
        .await
        .context("the prefixed bucket must exist")?;
    drop(host);

    Ok(())
}
