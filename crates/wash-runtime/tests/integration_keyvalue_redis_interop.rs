#![cfg(feature = "wasm_component_model_implements")]
//! Interop test for the multiplexed redis [`KvBackend`]: it speaks a **flat
//! redis keyspace** (like the v1 wasmCloud redis provider), so it can read and
//! write keys created/managed by any other redis client and not a private
//! per-bucket layout.
//!
//! Drives a real redis container directly through the backend (no host/guest):
//!   1. a raw redis client `SET`s a plain key; the backend `get`s it back;
//!   2. the backend `set`s a key; a raw redis client `GET`s it back;
//!   3. a raw client seeds a counter; the backend `increment`s it (signed),
//!      and the raw client sees the new value — proving `INCRBY` semantics.
//!
//! Requires Docker (redis); marked `#[ignore]`, so it runs only under
//! `cargo test --include-ignored`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;

use anyhow::{Context, Result};
use redis::AsyncCommands;
use testcontainers::{
    GenericImage,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};

use wash_runtime::plugin::multiplex::BackendProvider;
use wash_runtime::plugin::wasi_keyvalue::RedisProvider;

// The flat backend ignores the bucket identifier for key naming (the connection
// is the keyspace), so any value works here.
const BUCKET: &str = "ignored";

#[tokio::test]
#[ignore = "requires Docker (redis); run with `cargo test --include-ignored`"]
async fn redis_backend_interops_with_a_raw_keyspace() -> Result<()> {
    let redis = GenericImage::new("redis", "7-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start redis: {e}"))?;
    let url = format!(
        "redis://127.0.0.1:{}",
        redis.get_host_port_ipv4(6379).await?
    );

    // A raw redis client, standing in for "someone else" managing the keyspace.
    let raw_client = redis::Client::open(url.as_str())?;
    let mut raw = raw_client.get_multiplexed_async_connection().await?;

    // The multiplexed backend pointed at the same redis (no prefix => flat).
    let backend: wash_runtime::plugin::wasi_keyvalue::KvId = RedisProvider
        .instantiate(&HashMap::from([
            ("backend".to_string(), "redis".to_string()),
            ("url".to_string(), url.clone()),
        ]))
        .await
        .context("failed to instantiate redis backend")?;

    // 1. Raw client writes a plain key; the backend reads it back.
    raw.set::<_, _, ()>("external-key", b"hello from raw redis".to_vec())
        .await?;
    let got = backend
        .get(BUCKET, "external-key")
        .await
        .map_err(|e| anyhow::anyhow!("backend.get failed: {e:?}"))?;
    assert_eq!(
        got.as_deref(),
        Some(b"hello from raw redis".as_slice()),
        "backend must read a key created by a raw redis client"
    );

    // 2. Backend writes a key; the raw client reads it back (plain, no wrapping).
    backend
        .set(BUCKET, "from-backend", b"hello from backend".to_vec())
        .await
        .map_err(|e| anyhow::anyhow!("backend.set failed: {e:?}"))?;
    let raw_value: Option<Vec<u8>> = raw.get("from-backend").await?;
    assert_eq!(
        raw_value.as_deref(),
        Some(b"hello from backend".as_slice()),
        "a raw redis client must see the backend's write as a plain key"
    );

    // 3. Signed increment interops with a raw INCRBY counter.
    raw.set::<_, _, ()>("counter", "10").await?;
    let incremented = backend
        .increment(BUCKET, "counter", 5)
        .await
        .map_err(|e| anyhow::anyhow!("backend.increment failed: {e:?}"))?;
    assert_eq!(
        incremented, 15,
        "increment must build on the raw counter value"
    );
    let raw_counter: i64 = raw.get("counter").await?;
    assert_eq!(
        raw_counter, 15,
        "a raw redis client must see the incremented counter"
    );

    // 4. The optional `prefix` namespaces keys within the same DB. A prefixed
    // backend maps logical key `k` to redis key `app1:k`, and isolates from the
    // flat keys written above.
    let prefixed: wash_runtime::plugin::wasi_keyvalue::KvId = RedisProvider
        .instantiate(&HashMap::from([
            ("backend".to_string(), "redis".to_string()),
            ("url".to_string(), url.clone()),
            ("prefix".to_string(), "app1:".to_string()),
        ]))
        .await
        .context("failed to instantiate prefixed redis backend")?;

    // A prefixed write lands under the prefix in the raw keyspace...
    prefixed
        .set(BUCKET, "k", b"v".to_vec())
        .await
        .map_err(|e| anyhow::anyhow!("prefixed set failed: {e:?}"))?;
    let raw_prefixed: Option<Vec<u8>> = raw.get("app1:k").await?;
    assert_eq!(
        raw_prefixed.as_deref(),
        Some(b"v".as_slice()),
        "a prefixed backend must write to `{{prefix}}{{key}}` in the raw keyspace"
    );

    // ...and a raw key under the prefix is read back by its logical name.
    raw.set::<_, _, ()>("app1:external", b"x".to_vec()).await?;
    let via_prefixed = prefixed
        .get(BUCKET, "external")
        .await
        .map_err(|e| anyhow::anyhow!("prefixed get failed: {e:?}"))?;
    assert_eq!(
        via_prefixed.as_deref(),
        Some(b"x".as_slice()),
        "a prefixed backend must read `{{prefix}}{{key}}` by its logical key"
    );

    // Isolation: the unprefixed backend does not see the prefixed key, and the
    // prefixed `list-keys` returns logical (stripped) names, not the flat keys.
    let flat_view = backend
        .get(BUCKET, "external")
        .await
        .map_err(|e| anyhow::anyhow!("unprefixed get failed: {e:?}"))?;
    assert!(
        flat_view.is_none(),
        "the unprefixed backend must not see a prefixed key"
    );
    let mut listed = prefixed
        .list_keys(BUCKET, None)
        .await
        .map_err(|e| anyhow::anyhow!("prefixed list_keys failed: {e:?}"))?
        .keys;
    listed.sort();
    assert_eq!(
        listed,
        vec!["external".to_string(), "k".to_string()],
        "prefixed list-keys must return logical keys (prefix stripped), isolated from flat keys"
    );

    Ok(())
}
