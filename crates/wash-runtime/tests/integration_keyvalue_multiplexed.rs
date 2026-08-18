#![cfg(feature = "wasm_component_model_implements")]
//! End-to-end test for the multiplexed keyvalue backends (redis + NATS
//! JetStream) routed through `MultiplexedKeyValue`'s provider/registry path.
//!
//! Builds a registry from two named host interfaces, one `redis`, one `nats`,
//! each with its own connection `url`.
//! Drives *real* backends against containers, asserting each named import routes to
//! the correct server and that the two are isolated.
//!
//! Requires Docker (redis + NATS); marked `#[ignore]`, so it runs only under
//! `cargo test --include-ignored` (CI's Linux leg) and not a plain `cargo test`

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, Result};
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};

use wash_runtime::plugin::wasi_keyvalue::{
    BucketPolicy, MultiplexedAsyncKeyValue, MultiplexedKeyValue, NatsProvider, RedisProvider,
};
use wash_runtime::wit::WitInterface;

fn kv_iface(name: &str, backend: &str, url: &str) -> WitInterface {
    WitInterface {
        namespace: "wasi".to_string(),
        package: "keyvalue".to_string(),
        interfaces: [
            "store".to_string(),
            "atomics".to_string(),
            "batch".to_string(),
        ]
        .into_iter()
        .collect(),
        version: None,
        config: HashMap::from([
            ("backend".to_string(), backend.to_string()),
            ("url".to_string(), url.to_string()),
        ]),
        name: Some(name.to_string()),
    }
}

#[tokio::test]
#[ignore = "requires Docker (redis + NATS); run with `cargo test --include-ignored`"]
async fn multiplexed_routes_to_redis_and_nats() -> Result<()> {
    // --- redis container ---
    let redis = GenericImage::new("redis", "7-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start redis: {e}"))?;
    let redis_port = redis.get_host_port_ipv4(6379).await?;
    let redis_url = format!("redis://127.0.0.1:{redis_port}");

    // --- NATS container (JetStream enabled for KV) ---
    let nats = GenericImage::new("nats", "2.12.8-alpine")
        .with_exposed_port(4222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .with_cmd(["-js"])
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start nats: {e}"))?;
    let nats_port = nats.get_host_port_ipv4(4222).await?;
    let nats_url = format!("nats://127.0.0.1:{nats_port}");

    // The NATS backend opens existing KV stores, so create the bucket first.
    let nats_client = async_nats::connect(&nats_url)
        .await
        .context("connect to nats")?;
    async_nats::jetstream::new(nats_client)
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: "shared".to_string(),
            ..Default::default()
        })
        .await
        .context("create nats kv bucket")?;

    // --- build the routing registry from two named host interfaces ---
    let plugin = MultiplexedKeyValue::new()
        .with_provider(Arc::new(RedisProvider))
        .with_provider(Arc::new(NatsProvider::default()));
    let interfaces = HashSet::from([
        kv_iface("redis-kv", "redis", &redis_url),
        kv_iface("nats-kv", "nats", &nats_url),
    ]);
    let registry = plugin.build_registry(&interfaces).await?;
    let redis_be = registry.get("redis-kv").expect("redis-kv routed").clone();
    let nats_be = registry.get("nats-kv").expect("nats-kv routed").clone();

    // --- each named import lands on its own server ---
    redis_be.open("bucket").await.map_err(err)?;
    redis_be
        .set("bucket", "k", b"from-redis".to_vec())
        .await
        .map_err(err)?;
    nats_be.open("shared").await.map_err(err)?;
    nats_be
        .set("shared", "k", b"from-nats".to_vec())
        .await
        .map_err(err)?;

    assert_eq!(
        redis_be.get("bucket", "k").await.map_err(err)?,
        Some(b"from-redis".to_vec())
    );
    assert_eq!(
        nats_be.get("shared", "k").await.map_err(err)?,
        Some(b"from-nats".to_vec())
    );
    // Isolation: the redis backend never sees the NATS-only key (different server).
    assert_eq!(
        redis_be.get("bucket", "nats-only").await.map_err(err)?,
        None
    );

    // --- exercise the rest of the surface against the real redis backend ---
    redis_be
        .set_many(
            "bucket",
            vec![("a".into(), b"1".to_vec()), ("b".into(), b"2".to_vec())],
        )
        .await
        .map_err(err)?;
    assert_eq!(
        redis_be
            .get_many("bucket", vec!["a".into(), "b".into(), "missing".into()])
            .await
            .map_err(err)?,
        vec![
            Some(("a".to_string(), b"1".to_vec())),
            Some(("b".to_string(), b"2".to_vec())),
            None,
        ]
    );
    assert!(redis_be.exists("bucket", "a").await.map_err(err)?);
    redis_be.delete("bucket", "a").await.map_err(err)?;
    assert!(!redis_be.exists("bucket", "a").await.map_err(err)?);
    assert_eq!(
        redis_be.increment("bucket", "ctr", 5).await.map_err(err)?,
        5
    );
    assert_eq!(
        redis_be.increment("bucket", "ctr", 3).await.map_err(err)?,
        8
    );

    // --- and the real NATS backend ---
    assert_eq!(nats_be.increment("shared", "ctr", 7).await.map_err(err)?, 7);
    let mut keys = nats_be.list_keys("shared", None).await.map_err(err)?.keys;
    keys.sort();
    assert_eq!(keys, vec!["ctr".to_string(), "k".to_string()]);

    Ok(())
}

/// The bucket policy on a named NATS interface: `create` decides whether an
/// identifier may bring a bucket into existence, `bucket_prefix` decides which
/// physical bucket it lands in, and `bucket` pins every identifier to one
/// store.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn nats_bucket_policy_governs_creation_and_naming() -> Result<()> {
    let nats = GenericImage::new("nats", "2.12.8-alpine")
        .with_exposed_port(4222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .with_cmd(["-js"])
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start nats: {e}"))?;
    let nats_port = nats.get_host_port_ipv4(4222).await?;
    let nats_url = format!("nats://127.0.0.1:{nats_port}");

    // A host that withholds creation: its interfaces open only what exists,
    // and one asking for `create: missing` does not get it.
    let strict = kv_iface("strict", "nats", &nats_url);
    let mut escalating = kv_iface("escalating", "nats", &nats_url);
    escalating
        .config
        .insert("create".to_string(), "missing".to_string());

    let withholding = MultiplexedKeyValue::new()
        .with_provider(Arc::new(NatsProvider::default()))
        .build_registry(&HashSet::from([strict, escalating]))
        .await?;
    let strict_be = withholding.get("strict").expect("strict routed").clone();
    let escalating_be = withholding
        .get("escalating")
        .expect("escalating routed")
        .clone();

    for (name, backend) in [("strict", &strict_be), ("escalating", &escalating_be)] {
        let e = backend
            .open("counters")
            .await
            .expect_err("an absent bucket must not open on a host that withholds creation");
        assert!(
            matches!(
                e,
                wash_runtime::plugin::wasi_keyvalue::StoreError::NoSuchStore
            ),
            "{name}: expected no-such-store, got {e:?}"
        );
    }

    // A host that allows creation, with a prefix and a pin.
    let mut creating = kv_iface("creating", "nats", &nats_url);
    creating
        .config
        .insert("bucket_prefix".to_string(), "team-a_".to_string());

    let mut pinned = kv_iface("pinned", "nats", &nats_url);
    pinned
        .config
        .insert("bucket".to_string(), "PINNED".to_string());

    let allowing = MultiplexedKeyValue::new()
        .with_provider(Arc::new(NatsProvider::with_defaults(
            BucketPolicy::create_missing(),
        )))
        .build_registry(&HashSet::from([creating, pinned]))
        .await?;
    let creating_be = allowing.get("creating").expect("creating routed").clone();
    let pinned_be = allowing.get("pinned").expect("pinned routed").clone();

    // `create = missing` creates it, under the prefixed physical name.
    creating_be.open("counters").await.map_err(err)?;
    creating_be
        .set("counters", "k", b"v".to_vec())
        .await
        .map_err(err)?;

    let js = async_nats::jetstream::new(async_nats::connect(&nats_url).await?);
    js.get_key_value("team-a_counters")
        .await
        .context("the prefixed bucket must exist")?;
    js.get_key_value("counters")
        .await
        .expect_err("the unprefixed name must not have been created");

    // Now that the physical bucket exists, the withholding host's interface
    // reaches it by the name it was configured with — the prefix is what
    // separates them, so an unprefixed `counters` is still absent.
    strict_be
        .open("team-a_counters")
        .await
        .map_err(err)
        .context("an existing bucket must open under create = never")?;
    assert_eq!(
        strict_be
            .get("team-a_counters", "k")
            .await
            .map_err(err)?
            .as_deref(),
        Some(b"v".as_slice())
    );

    // A pinned interface ignores the identifier entirely: both opens land in
    // `PINNED`.
    pinned_be.open("one").await.map_err(err)?;
    pinned_be
        .set("one", "k", b"pinned".to_vec())
        .await
        .map_err(err)?;
    pinned_be.open("two").await.map_err(err)?;
    assert_eq!(
        pinned_be.get("two", "k").await.map_err(err)?.as_deref(),
        Some(b"pinned".as_slice()),
        "a pinned policy must resolve every identifier to one bucket"
    );

    Ok(())
}

/// `wasi:keyvalue` and `wasmcloud:keyvalue` resolve NATS buckets identically:
/// both plugins register the same providers, so an embedder's bucket policy —
/// a `wash host`'s `--keyvalue-nats-*` flags, a `wash dev` project's
/// `dev.wasi_keyvalue_nats` — reaches a named import of either package, and an
/// interface still overrides it key by key.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn embedder_defaults_reach_both_keyvalue_packages() -> Result<()> {
    let nats = GenericImage::new("nats", "2.12.8-alpine")
        .with_exposed_port(4222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .with_cmd(["-js"])
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start nats: {e}"))?;
    let nats_port = nats.get_host_port_ipv4(4222).await?;
    let nats_url = format!("nats://127.0.0.1:{nats_port}");

    // What an embedder configured: create missing buckets, under a prefix.
    let defaults = BucketPolicy {
        prefix: "host_".to_string(),
        create: wash_runtime::plugin::wasi_keyvalue::CreatePolicy::Missing,
        ..BucketPolicy::default()
    };

    let wasi_kv = MultiplexedKeyValue::new()
        .with_provider(Arc::new(NatsProvider::with_defaults(defaults.clone())));
    let wasmcloud_kv = MultiplexedAsyncKeyValue::new()
        .with_provider(Arc::new(NatsProvider::with_defaults(defaults.clone())));

    // Neither interface configures a policy of its own, so both inherit.
    let wasi_be = wasi_kv
        .build_registry(&HashSet::from([kv_iface("kv", "nats", &nats_url)]))
        .await?
        .get("kv")
        .expect("wasi:keyvalue routed")
        .clone();
    let wasmcloud_be = wasmcloud_kv
        .build_registry(&HashSet::from([wasmcloud_kv_iface("kv", &nats_url)]))
        .await?
        .get("kv")
        .expect("wasmcloud:keyvalue routed")
        .clone();

    // The inherited `create = missing` applies to both...
    wasi_be.open("counters").await.map_err(err)?;
    wasi_be
        .set("counters", "k", b"v".to_vec())
        .await
        .map_err(err)?;
    wasmcloud_be.open("counters").await.map_err(err)?;

    // ...and so does the inherited prefix, which is why both land in the one
    // physical bucket.
    assert_eq!(
        wasmcloud_be
            .get("counters", "k")
            .await
            .map_err(err)?
            .as_deref(),
        Some(b"v".as_slice()),
        "both packages must resolve the identifier to the same physical bucket"
    );
    let js = async_nats::jetstream::new(async_nats::connect(&nats_url).await?);
    js.get_key_value("host_counters")
        .await
        .context("the prefixed bucket must exist")?;

    // An interface that sets a key overrides the embedder for itself alone.
    let mut own = wasmcloud_kv_iface("own", &nats_url);
    own.config
        .insert("bucket_prefix".to_string(), "iface_".to_string());
    let own_be = wasmcloud_kv
        .build_registry(&HashSet::from([own]))
        .await?
        .get("own")
        .expect("own routed")
        .clone();
    own_be.open("counters").await.map_err(err)?;
    assert_eq!(
        own_be.get("counters", "k").await.map_err(err)?,
        None,
        "the overridden prefix must select a different bucket"
    );
    js.get_key_value("iface_counters")
        .await
        .context("the overriding interface's bucket must exist")?;

    Ok(())
}

/// A named `wasmcloud:keyvalue` host interface — the async package's spelling
/// of [`kv_iface`].
fn wasmcloud_kv_iface(name: &str, url: &str) -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "keyvalue".to_string(),
        interfaces: [
            "store".to_string(),
            "atomics".to_string(),
            "cas".to_string(),
            "batch".to_string(),
        ]
        .into_iter()
        .collect(),
        version: None,
        config: HashMap::from([
            ("backend".to_string(), "nats".to_string()),
            ("url".to_string(), url.to_string()),
        ]),
        name: Some(name.to_string()),
    }
}

/// The `KvBackend` ops return a WIT `store::Error` which isn't `std::error::Error`;
/// stringify it for `?`/`anyhow`.
fn err(e: impl std::fmt::Debug) -> anyhow::Error {
    anyhow::anyhow!("keyvalue backend error: {e:?}")
}
