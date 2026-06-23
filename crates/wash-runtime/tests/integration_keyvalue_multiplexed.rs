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

use wash_runtime::plugin::wasi_keyvalue::{MultiplexedKeyValue, NatsProvider, RedisProvider};
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
        .with_provider(Arc::new(NatsProvider));
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

/// The `KvBackend` ops return a WIT `store::Error` which isn't `std::error::Error`;
/// stringify it for `?`/`anyhow`.
fn err(e: impl std::fmt::Debug) -> anyhow::Error {
    anyhow::anyhow!("keyvalue backend error: {e:?}")
}
