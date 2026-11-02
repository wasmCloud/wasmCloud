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
//! The rest drop the backend's connection under it:
//! - every read made right after a redis restart succeeds (the backend
//!   reconnects and re-sends it);
//! - every write (`set`, `set_many`, `delete`, `delete_many`, `increment`,
//!   `put_if_absent`) on a just-dropped connection surfaces the error instead
//!   and is applied at most once;
//! - a list-keys cursor from before a reconnect is refused;
//! - while redis is stopped, a call fails within the configured redial
//!   schedule, and the backend recovers once redis is back.
//!
//! The backend reaches redis through a small TCP proxy the test owns, so the
//! url stays the same across the restart without pinning a host port (a port
//! picked free and handed to Docker can be taken by anything in between).
//!
//! Requires Docker (redis); marked `#[ignore]`, so it runs only under
//! `cargo test --include-ignored`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use redis::AsyncCommands;
use testcontainers::{
    ContainerAsync, GenericImage,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use wash_runtime::plugin::multiplex::BackendProvider;
use wash_runtime::plugin::wasi_keyvalue::RedisProvider;

// The flat backend ignores the bucket identifier for key naming (the connection
// is the keyspace), so any value works here.
const BUCKET: &str = "ignored";

#[tokio::test]
#[ignore = "requires Docker (redis); run with `cargo test --include-ignored`"]
async fn redis_backend_interops_with_a_raw_keyspace() -> Result<()> {
    let redis = start_redis().await?;
    let url = format!(
        "redis://127.0.0.1:{}",
        redis.get_host_port_ipv4(6379).await?
    );

    // A raw redis client, standing in for "someone else" managing the keyspace.
    let raw_client = redis::Client::open(url.as_str())?;
    let mut raw = raw_client.get_multiplexed_async_connection().await?;

    // The multiplexed backend pointed at the same redis (no prefix => flat).
    let backend: wash_runtime::plugin::wasi_keyvalue::KvId = RedisProvider
        .instantiate(
            &HashMap::from([
                ("backend".to_string(), "redis".to_string()),
                ("url".to_string(), url.clone()),
            ]),
            None,
        )
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
        .instantiate(
            &HashMap::from([
                ("backend".to_string(), "redis".to_string()),
                ("url".to_string(), url.clone()),
                ("prefix".to_string(), "app1:".to_string()),
            ]),
            None,
        )
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

/// A TCP proxy on a port this test holds for its whole life, forwarding to
/// whatever host port Docker gave the redis container this time.
///
/// [`RedisProxy::cut`] drops every connection it carries, as a server restart
/// does. While nothing listens upstream, an accepted connection is closed at
/// once, so a client sees the server as down.
struct RedisProxy {
    url: String,
    upstream: Arc<AtomicU16>,
    generation: watch::Sender<u64>,
    /// Connections the proxy is carrying right now.
    live: Arc<AtomicUsize>,
}

impl RedisProxy {
    async fn start(upstream_port: u16) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("redis://{}", listener.local_addr()?);
        let upstream = Arc::new(AtomicU16::new(upstream_port));
        let live = Arc::new(AtomicUsize::new(0));
        let (generation, _) = watch::channel(0u64);
        let (target, cuts, carried) = (upstream.clone(), generation.clone(), live.clone());
        tokio::spawn(async move {
            while let Ok((mut client, _)) = listener.accept().await {
                let port = target.load(Ordering::SeqCst);
                let mut cut = cuts.subscribe();
                let carried = carried.clone();
                carried.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    if let Ok(mut server) = TcpStream::connect(("127.0.0.1", port)).await {
                        tokio::select! {
                            _ = tokio::io::copy_bidirectional(&mut client, &mut server) => {}
                            _ = cut.changed() => {}
                        }
                    }
                    // Both sockets are dropped by now.
                    drop(client);
                    carried.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        Ok(Self {
            url,
            upstream,
            generation,
            live,
        })
    }

    /// Drop every connection the proxy carries, and wait until each is
    /// closed, so nothing sent afterwards on one of them reaches redis.
    async fn cut(&self) {
        self.generation.send_modify(|g| *g += 1);
        while self.live.load(Ordering::SeqCst) > 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn point_at(&self, port: u16) {
        self.upstream.store(port, Ordering::SeqCst);
    }
}

/// Start redis and wait until it answers on its mapped host port. The log
/// line alone is not enough: Docker's port forward can still refuse for a
/// moment after the server inside says it is ready.
async fn start_redis() -> Result<ContainerAsync<GenericImage>> {
    let redis = GenericImage::new("redis", "7-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start redis: {e}"))?;
    wait_until_ready(&redis).await?;
    Ok(redis)
}

/// Wait until a fresh client gets an answer to `PING` on the host port Docker
/// maps to the container's 6379, and return that port. The port is looked up
/// again on every try: right after a restart, Docker can still report the
/// mapping it is replacing.
async fn wait_until_ready(redis: &ContainerAsync<GenericImage>) -> Result<u16> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(port) = redis.get_host_port_ipv4(6379).await
            && ping(port).await
        {
            return Ok(port);
        }
        anyhow::ensure!(
            tokio::time::Instant::now() < deadline,
            "redis did not answer within 30s"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn ping(port: u16) -> bool {
    let Ok(client) = redis::Client::open(format!("redis://127.0.0.1:{port}")) else {
        return false;
    };
    let config = redis::AsyncConnectionConfig::new()
        .set_connection_timeout(Duration::from_secs(1))
        .set_response_timeout(Duration::from_secs(1));
    match client
        .get_multiplexed_async_connection_with_config(&config)
        .await
    {
        Ok(mut raw) => redis::cmd("PING")
            .query_async::<String>(&mut raw)
            .await
            .is_ok(),
        Err(_) => false,
    }
}

/// Restart the redis container behind `proxy`, dropping every connection
/// through it, and wait until the restarted server answers a fresh client.
async fn restart_redis(redis: &ContainerAsync<GenericImage>, proxy: &RedisProxy) -> Result<()> {
    redis.stop().await?;
    proxy.cut().await;
    redis.start().await?;
    proxy.point_at(wait_until_ready(redis).await?);
    Ok(())
}

async fn backend_behind(proxy: &RedisProxy) -> Result<wash_runtime::plugin::wasi_keyvalue::KvId> {
    backend_at(&proxy.url).await
}

async fn backend_at(url: &str) -> Result<wash_runtime::plugin::wasi_keyvalue::KvId> {
    backend_with(url, &[]).await
}

async fn backend_with(
    url: &str,
    settings: &[(&str, &str)],
) -> Result<wash_runtime::plugin::wasi_keyvalue::KvId> {
    let mut config = HashMap::from([
        ("backend".to_string(), "redis".to_string()),
        ("url".to_string(), url.to_string()),
    ]);
    config.extend(settings.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    RedisProvider
        .instantiate(&config, None)
        .await
        .context("failed to instantiate redis backend")
}

/// A redis restart (or failover) drops the backend's connection. The backend
/// is pooled and outlives workloads, so it must replace the connection itself,
/// and a read that finds the connection dead is sent again on the
/// replacement: the first read after each restart succeeds.
#[tokio::test]
#[ignore = "requires Docker (redis); run with `cargo test --include-ignored`"]
async fn redis_backend_reconnects_after_a_server_restart() -> Result<()> {
    let redis = start_redis().await?;
    let proxy = RedisProxy::start(redis.get_host_port_ipv4(6379).await?).await?;
    let backend = backend_behind(&proxy).await?;
    let fail = |op: &str, e| anyhow::anyhow!("{op} right after a restart failed: {e:?}");
    let seed = |op: &str, e| anyhow::anyhow!("seeding with {op} failed: {e:?}");

    backend
        .set(BUCKET, "k", b"1".to_vec())
        .await
        .map_err(|e| seed("set", e))?;
    backend
        .set_many(
            BUCKET,
            vec![
                ("a".to_string(), b"A".to_vec()),
                ("b".to_string(), b"B".to_vec()),
            ],
        )
        .await
        .map_err(|e| seed("set_many", e))?;

    restart_redis(&redis, &proxy).await?;
    let got = backend.get(BUCKET, "k").await.map_err(|e| fail("get", e))?;
    assert_eq!(got.as_deref(), Some(b"1".as_slice()));

    restart_redis(&redis, &proxy).await?;
    assert!(
        backend
            .exists(BUCKET, "k")
            .await
            .map_err(|e| fail("exists", e))?
    );

    restart_redis(&redis, &proxy).await?;
    let many = backend
        .get_many(BUCKET, vec!["a".to_string(), "b".to_string()])
        .await
        .map_err(|e| fail("get_many", e))?;
    assert_eq!(
        many,
        vec![
            Some(("a".to_string(), b"A".to_vec())),
            Some(("b".to_string(), b"B".to_vec()))
        ]
    );

    restart_redis(&redis, &proxy).await?;
    let mut keys = backend
        .list_keys(BUCKET, None)
        .await
        .map_err(|e| fail("list_keys", e))?
        .keys;
    keys.sort();
    assert_eq!(keys, ["a", "b", "k"]);

    // A url's DB is part of how stores are kept apart. The replacement
    // connection authenticates and selects the same DB the url names, so a
    // read after a restart sees that DB and no other.
    let db3_url = format!("{}/3", proxy.url);
    let db3 = backend_at(&db3_url).await?;
    let mut raw3 = redis::Client::open(db3_url.as_str())?
        .get_multiplexed_async_connection()
        .await?;
    raw3.set::<_, _, ()>("only-in-3", "3").await?;
    restart_redis(&redis, &proxy).await?;
    let in3 = db3
        .get(BUCKET, "only-in-3")
        .await
        .map_err(|e| fail("get in db 3", e))?;
    assert_eq!(in3.as_deref(), Some(b"3".as_slice()));
    let in0 = backend
        .exists(BUCKET, "only-in-3")
        .await
        .map_err(|e| fail("exists in db 0", e))?;
    assert!(
        !in0,
        "a key in DB 3 must not show in DB 0 after a reconnect"
    );

    Ok(())
}

/// A write is never sent twice: if the connection dropped under it, it may
/// already have run. The call that finds the connection dead surfaces the
/// error, the server state shows it was applied at most once, and the next
/// call succeeds on the replacement connection. A read in the same position
/// is retried.
#[tokio::test]
#[ignore = "requires Docker (redis); run with `cargo test --include-ignored`"]
async fn redis_backend_does_not_retry_a_write() -> Result<()> {
    let redis = start_redis().await?;
    let proxy = RedisProxy::start(redis.get_host_port_ipv4(6379).await?).await?;
    let backend = backend_behind(&proxy).await?;

    // Even the writes that leave the same state however often they run are
    // sent once: a replay could overwrite a concurrent write if the
    // first attempt landed and only its reply was lost. Each write that finds
    // the connection dead surfaces the error, was not applied (the connection
    // died before it was sent), and succeeds when the caller tries again.
    // The connection is cut at the proxy rather than by restarting redis, so
    // each write follows the cut at once: a write after a few quiet seconds
    // is checked with a `PING` first and would succeed.
    let raw_client = redis::Client::open(proxy.url.as_str())?;
    let check = |op: &str, r: Result<(), wash_runtime::plugin::wasi_keyvalue::StoreError>| {
        r.err()
            .map(|_| ())
            .with_context(|| format!("{op} on the dropped connection is not retried"))
    };

    proxy.cut().await;
    let mut raw = raw_client.get_multiplexed_async_connection().await?;
    check("set", backend.set(BUCKET, "w", b"1".to_vec()).await)?;
    assert_eq!(raw.get::<_, Option<Vec<u8>>>("w").await?, None);
    backend
        .set(BUCKET, "w", b"1".to_vec())
        .await
        .map_err(|e| anyhow::anyhow!("set after the failed one: {e:?}"))?;

    proxy.cut().await;
    let mut raw = raw_client.get_multiplexed_async_connection().await?;
    let pairs = vec![
        ("w1".to_string(), b"1".to_vec()),
        ("w2".to_string(), b"2".to_vec()),
    ];
    check("set_many", backend.set_many(BUCKET, pairs.clone()).await)?;
    assert_eq!(raw.exists::<_, u64>(&["w1", "w2"]).await?, 0);
    backend
        .set_many(BUCKET, pairs)
        .await
        .map_err(|e| anyhow::anyhow!("set_many after the failed one: {e:?}"))?;

    proxy.cut().await;
    let mut raw = raw_client.get_multiplexed_async_connection().await?;
    check("delete", backend.delete(BUCKET, "w").await)?;
    assert_eq!(raw.exists::<_, u64>("w").await?, 1);
    backend
        .delete(BUCKET, "w")
        .await
        .map_err(|e| anyhow::anyhow!("delete after the failed one: {e:?}"))?;

    proxy.cut().await;
    let mut raw = raw_client.get_multiplexed_async_connection().await?;
    let keys = vec!["w1".to_string(), "w2".to_string()];
    check(
        "delete_many",
        backend.delete_many(BUCKET, keys.clone()).await,
    )?;
    assert_eq!(raw.exists::<_, u64>(&["w1", "w2"]).await?, 2);
    backend
        .delete_many(BUCKET, keys)
        .await
        .map_err(|e| anyhow::anyhow!("delete_many after the failed one: {e:?}"))?;
    assert_eq!(raw.exists::<_, u64>(&["w1", "w2", "w"]).await?, 0);

    // A read on the same kind of dropped connection is retried and succeeds.
    proxy.cut().await;
    backend
        .get(BUCKET, "w")
        .await
        .map_err(|e| anyhow::anyhow!("a read after a restart is retried: {e:?}"))?;

    // increment (INCRBY)
    proxy.cut().await;
    let mut raw = raw_client.get_multiplexed_async_connection().await?;
    raw.set::<_, _, ()>("counter", "5").await?;
    backend
        .increment(BUCKET, "counter", 1)
        .await
        .expect_err("an increment on the dropped connection is not retried");
    let counter: i64 = raw.get("counter").await?;
    assert_eq!(
        counter, 5,
        "the failed increment must not have been applied"
    );
    let after = backend
        .increment(BUCKET, "counter", 1)
        .await
        .map_err(|e| anyhow::anyhow!("increment after the failed one: {e:?}"))?;
    assert_eq!(after, 6, "applied exactly once");

    // put_if_absent (SET NX)
    proxy.cut().await;
    let mut raw = raw_client.get_multiplexed_async_connection().await?;
    backend
        .put_if_absent(BUCKET, "fresh", b"x".to_vec())
        .await
        .expect_err("a put-if-absent on the dropped connection is not retried");
    let fresh: Option<Vec<u8>> = raw.get("fresh").await?;
    assert_eq!(fresh, None);
    assert!(
        backend
            .put_if_absent(BUCKET, "fresh", b"x".to_vec())
            .await
            .map_err(|e| anyhow::anyhow!("put_if_absent after the failed one: {e:?}"))?,
        "the key was absent, so this write happens"
    );

    Ok(())
}

/// A later SCAN page (nonzero cursor) is not replayed on a new connection: a
/// restarted server may lay out its table differently, so the cursor could
/// skip keys there. The page that crosses the reconnect fails, and so does
/// any later use of a cursor from before it, however the reconnect came
/// about.
#[tokio::test]
#[ignore = "requires Docker (redis); run with `cargo test --include-ignored`"]
async fn redis_backend_refuses_a_list_keys_cursor_from_before_a_reconnect() -> Result<()> {
    let redis = start_redis().await?;
    let proxy = RedisProxy::start(redis.get_host_port_ipv4(6379).await?).await?;
    let backend = backend_behind(&proxy).await?;
    let mut raw = redis::Client::open(proxy.url.as_str())?
        .get_multiplexed_async_connection()
        .await?;
    let many: Vec<(String, String)> = (0..5_000)
        .map(|i| (format!("page-{i}"), "v".to_string()))
        .collect();
    raw.mset::<_, _, ()>(&many).await?;
    // Paging with the backend's cursors visits every key.
    let mut seen = std::collections::HashSet::new();
    let mut cursor = None;
    loop {
        let page = backend
            .list_keys(BUCKET, cursor)
            .await
            .map_err(|e| anyhow::anyhow!("list_keys page: {e:?}"))?;
        seen.extend(page.keys);
        cursor = page.cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert!(
        (0..5_000).all(|i| seen.contains(&format!("page-{i}"))),
        "a full listing must visit every key"
    );
    let mut cursor = None;
    loop {
        let page = backend
            .list_keys(BUCKET, cursor)
            .await
            .map_err(|e| anyhow::anyhow!("list_keys page: {e:?}"))?;
        cursor = page.cursor;
        if !page.keys.is_empty() || cursor.is_none() {
            break;
        }
    }
    let cursor = cursor.context("5000 keys take more than one SCAN page")?;
    proxy.cut().await;
    backend
        .list_keys(BUCKET, Some(cursor))
        .await
        .expect_err("a later SCAN page is not retried on a new connection");
    // Nor is the cursor accepted once another call has reconnected.
    backend
        .exists(BUCKET, "page-0")
        .await
        .map_err(|e| anyhow::anyhow!("exists after the failed page: {e:?}"))?;
    let err = backend
        .list_keys(BUCKET, Some(cursor))
        .await
        .expect_err("a cursor from before the reconnect is refused");
    assert!(format!("{err:?}").contains("first page"), "{err:?}");

    // The same holds when another call, not the page request, is what finds
    // the connection dead and reconnects.
    let page = backend
        .list_keys(BUCKET, None)
        .await
        .map_err(|e| anyhow::anyhow!("list_keys from the start: {e:?}"))?;
    let cursor = page.cursor.context("a fresh listing pages too")?;
    proxy.cut().await;
    backend
        .get(BUCKET, "page-1")
        .await
        .map_err(|e| anyhow::anyhow!("get between pages: {e:?}"))?;
    let err = backend
        .list_keys(BUCKET, Some(cursor))
        .await
        .expect_err("a cursor from before the reconnect is refused");
    assert!(format!("{err:?}").contains("first page"), "{err:?}");

    // A listing started after the reconnect lands on the same redis cursors
    // as the stale one (same keys, same table), yet the stale listing still
    // cannot continue, and the fresh one can.
    let fresh = backend
        .list_keys(BUCKET, None)
        .await
        .map_err(|e| anyhow::anyhow!("list_keys after the reconnect: {e:?}"))?
        .cursor
        .context("a fresh listing pages too")?;
    backend
        .list_keys(BUCKET, Some(cursor))
        .await
        .expect_err("the stale listing stays refused");
    backend
        .list_keys(BUCKET, Some(fresh))
        .await
        .map_err(|e| anyhow::anyhow!("the fresh listing continues: {e:?}"))?;

    Ok(())
}

/// While the server is down, a call fails within the configured redial
/// schedule rather than after a backoff of its own, and once the server is
/// back the next call succeeds: a failed redial is not kept.
#[tokio::test]
#[ignore = "requires Docker (redis); run with `cargo test --include-ignored`"]
async fn redis_backend_redials_within_its_settings_and_recovers() -> Result<()> {
    let redis = start_redis().await?;
    let proxy = RedisProxy::start(redis.get_host_port_ipv4(6379).await?).await?;
    let backend = backend_with(
        &proxy.url,
        &[
            ("backend-reconnect-num-retries", "3"),
            ("backend-reconnect-max-delay-ms", "200"),
        ],
    )
    .await?;

    redis.stop().await?;
    proxy.cut().await;
    // Eight calls at once all find the connection dead and queue for the
    // redial. They share its outcome, so together they fail after one redial
    // schedule: four attempts with pauses of 25-50, 50-100 and 100-200ms,
    // under 400ms. Eight schedules in turn would take at least 1.4s, and a
    // backoff that ignored the maximum would take seconds.
    let started = std::time::Instant::now();
    let calls = (0..8).map(|_| backend.get(BUCKET, "k"));
    for result in futures::future::join_all(calls).await {
        result.expect_err("nothing serves redis while it is stopped");
    }
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(1), "failing took {elapsed:?}");

    redis.start().await?;
    proxy.point_at(wait_until_ready(&redis).await?);
    backend
        .set(BUCKET, "k", b"back".to_vec())
        .await
        .map_err(|e| anyhow::anyhow!("set once redis is back: {e:?}"))?;

    Ok(())
}
