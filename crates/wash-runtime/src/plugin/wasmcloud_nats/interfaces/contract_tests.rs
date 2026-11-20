use super::*;
use crate::engine::ctx::{Ctx, extract_active_ctx};
use crate::plugin::{
    HostPlugin,
    wasmcloud_nats::{PLUGIN_NATS_ID, WasmcloudNats, config::NatsConfig, test_server::NatsServer},
};
use futures::{FutureExt as _, StreamExt as _};
use std::collections::HashMap;
use wasmtime::component::Resource;

/// Runs a sync-signature host method under the store lock. Those methods
/// never await, so a future that is not ready at once is a bug, not a wait.
macro_rules! sync_call {
    ($store_accessor:expr, |$active:ident| $call:expr) => {
        $store_accessor.with(|mut access| {
            let mut $active = extract_active_ctx(access.get());
            $call.now_or_never().expect("sync host method")
        })
    };
}

fn borrow<T: 'static>(resource: &Resource<T>) -> Resource<T> {
    Resource::new_borrow(resource.rep())
}

pub(super) fn message(subject: &str, body: &[u8]) -> types::NatsMessage {
    types::NatsMessage {
        subject: subject.into(),
        body: body.into(),
        reply_to: None,
        headers: None,
    }
}

pub(super) fn expect_denied<T>(result: Result<T, types::NatsError>) -> types::Denial {
    match result {
        Err(types::NatsError::Denied(denial)) => denial,
        Err(other) => panic!("expected a denial, got {other:?}"),
        Ok(_) => panic!("expected a denial, got Ok"),
    }
}

pub(super) fn expect_not_found<T: std::fmt::Debug>(result: Result<T, types::NatsError>) {
    assert!(
        matches!(result, Err(types::NatsError::NotFound(_))),
        "expected not-found, got {result:?}"
    );
}

/// Lifts both layers of a `run_concurrent` result: the store faulting, then
/// the body's own error.
pub(super) fn finished(result: wasmtime::Result<wasmtime::Result<()>>) -> anyhow::Result<()> {
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) | Err(e) => Err(anyhow::anyhow!("{e:#}")),
    }
}

/// Per-test names, so every test can share one `NATS_URL` server.
pub(super) struct Scope {
    pub(super) subjects: String,
    pub(super) stream: String,
    pub(super) bucket: String,
}

impl Scope {
    fn new() -> Self {
        let id = uuid::Uuid::new_v4().simple().to_string();
        Self {
            subjects: format!("contract.{id}"),
            stream: format!("CONTRACT-{id}"),
            bucket: format!("CONTRACT-{id}"),
        }
    }

    pub(super) fn subject(&self, leaf: &str) -> String {
        format!("{}.{leaf}", self.subjects)
    }
}

pub(super) struct Fixture {
    server: NatsServer,
    pub(super) plugin: Arc<WasmcloudNats>,
    config: NatsConfig,
    pub(super) conn: Arc<ConnHandle>,
    pub(super) scope: Scope,
}

/// Deletes the scope's stream and bucket from a `NATS_URL` server, failed
/// assertions included. Drop cannot await and the test's runtime may be
/// unwinding, so the deletes run on a thread with a runtime of their own.
impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.server.is_external() {
            return;
        }
        let url = self.server.url.clone();
        let stream = self.scope.stream.clone();
        let bucket = self.scope.bucket.clone();
        let _ = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(10), async {
                    let client = async_nats::connect(&url).await?;
                    let jetstream = async_nats::jetstream::new(client);
                    let _ = jetstream.delete_stream(&stream).await;
                    let _ = jetstream.delete_key_value(&bucket).await;
                    anyhow::Ok(())
                })
                .await?
            })
        })
        .join();
    }
}

pub(super) async fn fixture() -> anyhow::Result<Fixture> {
    #[cfg(feature = "wasi-tls")]
    crate::init_crypto();
    let server = NatsServer::start().await?;
    let scope = Scope::new();
    let config = NatsConfig::from_map(&crate::wit::Config::from([
        ("servers", server.url.clone()),
        ("subject-allow", format!("{}.>", scope.subjects)),
        ("stream-allow", scope.stream.clone()),
        ("stream-delete-allow", scope.stream.clone()),
        ("bucket-allow", scope.bucket.clone()),
        ("request-timeout-ms", "1000".into()),
    ]))?;
    let plugin = Arc::new(WasmcloudNats::new());
    let conn = acquire(&plugin, "workload", "", &config).await?;
    Ok(Fixture {
        server,
        plugin,
        config,
        conn,
        scope,
    })
}

/// Retries until the server accepts the plugin's connection.
async fn acquire(
    plugin: &WasmcloudNats,
    workload_id: &str,
    binding: &str,
    config: &NatsConfig,
) -> anyhow::Result<Arc<ConnHandle>> {
    let conn = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match plugin
                .connections
                .acquire(workload_id, binding, config, vec![], true)
                .await
            {
                Ok(conn) => break conn,
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    })
    .await?;
    Ok(conn)
}

/// A deny-all binding with no time for a round trip, so a resource method
/// wrongly routed through its id is refused or runs out of time.
async fn deny_all(fixture: &Fixture) -> anyhow::Result<Arc<ConnHandle>> {
    let mut denied = NatsConfig::from_map(&crate::wit::Config::from([(
        "servers",
        fixture.server.url.clone(),
    )]))?;
    denied.request_timeout = Some(Duration::ZERO);
    acquire(&fixture.plugin, "other", "other", &denied).await
}

pub(super) fn new_store(
    workload_id: &str,
    plugin: Option<&Arc<WasmcloudNats>>,
) -> anyhow::Result<wasmtime::Store<SharedCtx>> {
    let mut ctx = Ctx::builder(workload_id, "component");
    if let Some(plugin) = plugin {
        let host_plugin: Arc<dyn HostPlugin + Send + Sync> = plugin.clone();
        ctx = ctx.with_plugins(HashMap::from([(PLUGIN_NATS_ID, host_plugin)]));
    }
    let mut config = wasmtime::Config::new();
    config
        .wasm_component_model_async(true)
        .concurrency_support(true);
    let engine = wasmtime::Engine::new(&config).map_err(|e| anyhow::anyhow!("{e:#}"))?;
    Ok(wasmtime::Store::new(&engine, SharedCtx::new(ctx.build())))
}

#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn a_labeled_bucket_reports_revisions_and_typed_outcomes() -> anyhow::Result<()> {
    use labeled_kv::{HostBucket as BucketDrop, HostBucketWithStore as Bucket};
    let fx = fixture().await?;
    let other = deny_all(&fx).await?;
    fx.conn
        .jetstream
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: fx.scope.bucket.clone(),
            history: 10,
            ..Default::default()
        })
        .await?;
    let mut store = new_store("workload", Some(&fx.plugin))?;
    let bucket_name = fx.scope.bucket.clone();
    let ran = tokio::time::timeout(
        Duration::from_secs(60),
        store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let accessor = &host_accessor;
            let bucket = <SharedCtx as kv::HostWithStore<_>>::open(accessor, bucket_name.clone())
                .await?
                .unwrap();
            let revision = <SharedCtx as Bucket<_>>::put(
                accessor,
                other.clone(),
                borrow(&bucket),
                "item".into(),
                b"one".to_vec(),
            )
            .await?
            .unwrap();
            let entry = <SharedCtx as Bucket<_>>::get(
                accessor,
                other.clone(),
                borrow(&bucket),
                "item".into(),
            )
            .await?
            .unwrap();
            assert_eq!(entry.key, "item");
            assert_eq!((entry.revision, entry.value), (revision, b"one".to_vec()));
            assert_eq!(entry.operation, kv::KvOperation::Put);
            assert!(matches!(
                <SharedCtx as Bucket<_>>::create(
                    accessor,
                    other.clone(),
                    borrow(&bucket),
                    "item".into(),
                    vec![]
                )
                .await?,
                Err(types::NatsError::Jetstream(_))
            ));
            match <SharedCtx as Bucket<_>>::update(
                accessor,
                other.clone(),
                borrow(&bucket),
                "item".into(),
                vec![],
                revision + 10,
            )
            .await?
            {
                Err(types::NatsError::RevisionMismatch(actual)) => assert_eq!(actual, revision),
                unexpected => panic!("expected revision-mismatch({revision}), got {unexpected:?}"),
            }
            let updated = <SharedCtx as Bucket<_>>::update(
                accessor,
                other.clone(),
                borrow(&bucket),
                "item".into(),
                b"two".to_vec(),
                revision,
            )
            .await?
            .unwrap();
            assert!(updated > revision);
            assert_eq!(
                <SharedCtx as Bucket<_>>::history(
                    accessor,
                    other.clone(),
                    borrow(&bucket),
                    "item".into()
                )
                .await?
                .unwrap()
                .len(),
                2
            );
            let status = <SharedCtx as Bucket<_>>::status(accessor, other.clone(), borrow(&bucket))
                .await?
                .unwrap();
            assert_eq!(status.bucket, bucket_name);
            assert_eq!(status.history, 10);
            assert_eq!(status.values, 2);
            let page = <SharedCtx as Bucket<_>>::keys(
                accessor,
                other.clone(),
                borrow(&bucket),
                ">".into(),
            )
            .await?
            .unwrap();
            assert_eq!(page.keys, ["item"]);
            assert!(!page.truncated);
            assert!(matches!(
                <SharedCtx as Bucket<_>>::keys(
                    accessor,
                    other.clone(),
                    borrow(&bucket),
                    "bad..filter".into()
                )
                .await?,
                Err(types::NatsError::Unexpected(_))
            ));
            assert!(matches!(
                <SharedCtx as Bucket<_>>::create_with_ttl(
                    accessor,
                    other.clone(),
                    borrow(&bucket),
                    "lease".into(),
                    vec![],
                    1
                )
                .await?,
                Err(types::NatsError::Jetstream(_))
            ));
            <SharedCtx as Bucket<_>>::delete_by_revision(
                accessor,
                other.clone(),
                borrow(&bucket),
                "item".into(),
                updated,
            )
            .await?
            .unwrap();
            assert!(matches!(
                <SharedCtx as Bucket<_>>::get(
                    accessor,
                    other.clone(),
                    borrow(&bucket),
                    "item".into()
                )
                .await?,
                Err(types::NatsError::KeyNotFound)
            ));
            <SharedCtx as Bucket<_>>::create(
                accessor,
                other.clone(),
                borrow(&bucket),
                "delete".into(),
                vec![1],
            )
            .await?
            .unwrap();
            <SharedCtx as Bucket<_>>::delete(
                accessor,
                other.clone(),
                borrow(&bucket),
                "delete".into(),
            )
            .await?
            .unwrap();
            <SharedCtx as Bucket<_>>::purge(
                accessor,
                other.clone(),
                borrow(&bucket),
                "delete".into(),
            )
            .await?
            .unwrap();
            sync_call!(store_accessor, |active| BucketDrop::drop(
                &mut active,
                other.clone(),
                bucket
            ))?;
            Ok::<_, wasmtime::Error>(())
        }),
    )
    .await?;
    finished(ran)?;
    fx.plugin.connections.shutdown().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn a_pull_consumer_settles_each_message_once() -> anyhow::Result<()> {
    use labeled_js::{
        HostMessageHandle as Message, HostMessageHandleWithStore as Settle,
        HostPullConsumer as PullDrop, HostPullConsumerWithStore as Pull,
    };
    let fx = fixture().await?;
    let other = deny_all(&fx).await?;
    let work = fx.scope.subject("work");
    let stream = fx
        .conn
        .jetstream
        .create_stream(async_nats::jetstream::stream::Config {
            name: fx.scope.stream.clone(),
            subjects: vec![format!("{}.>", fx.scope.subjects)],
            ..Default::default()
        })
        .await?;
    stream
        .create_consumer(async_nats::jetstream::consumer::pull::Config {
            durable_name: Some("worker".into()),
            filter_subject: work.clone(),
            ..Default::default()
        })
        .await?;
    let mut store = new_store("workload", Some(&fx.plugin))?;
    let conn = &fx.conn;
    let scope = &fx.scope;
    let ran = tokio::time::timeout(
        Duration::from_secs(60),
        store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let accessor = &host_accessor;
            for number in 0..5 {
                let ack = <SharedCtx as js::HostWithStore<_>>::publish(
                    accessor,
                    message(&work, &[number]),
                )
                .await?
                .unwrap();
                assert_eq!(ack.stream_name, scope.stream);
            }
            let consumer = <SharedCtx as js::HostWithStore<_>>::open_pull_consumer(
                accessor,
                scope.stream.clone(),
                "worker".into(),
            )
            .await?
            .unwrap();
            let info = <SharedCtx as Pull<_>>::info(accessor, other.clone(), borrow(&consumer))
                .await?
                .unwrap();
            assert_eq!(info.name, "worker");
            assert_eq!(info.stream_name, scope.stream);
            assert_eq!(info.filter_subject, work);
            assert_eq!(info.num_pending, 5);
            let batch =
                <SharedCtx as Pull<_>>::fetch(accessor, other.clone(), borrow(&consumer), 5, 1000)
                    .await?
                    .unwrap();
            assert_eq!(batch.messages.len(), 5);
            assert_eq!(batch.stop, js::FetchStop::BatchFilled);
            assert!(conn.fetch_budget.outstanding() > 0);
            for (i, handle) in batch.messages.into_iter().enumerate() {
                let msg = sync_call!(store_accessor, |active| Message::message(
                    &mut active,
                    other.clone(),
                    borrow(&handle)
                ))?;
                assert_eq!(msg.body, [i as u8]);
                assert_eq!(msg.subject, work);
                let sequence = sync_call!(store_accessor, |active| Message::sequence(
                    &mut active,
                    other.clone(),
                    borrow(&handle)
                ))?;
                assert!(sequence > 0);
                let stored_at =
                    sync_call!(store_accessor, |active| Message::stored_at_unix_nanos(
                        &mut active,
                        other.clone(),
                        borrow(&handle)
                    ))?;
                assert!(stored_at > 0);
                let delivered = sync_call!(store_accessor, |active| Message::delivery_count(
                    &mut active,
                    other.clone(),
                    borrow(&handle)
                ))?;
                assert_eq!(delivered, 1);
                <SharedCtx as Settle<_>>::in_progress(accessor, other.clone(), borrow(&handle))
                    .await?
                    .unwrap();
                match i {
                    0 => {
                        <SharedCtx as Settle<_>>::ack_sync(accessor, other.clone(), borrow(&handle))
                            .await?
                            .unwrap()
                    }
                    1 => <SharedCtx as Settle<_>>::ack(accessor, other.clone(), borrow(&handle))
                        .await?
                        .unwrap(),
                    2 => <SharedCtx as Settle<_>>::term(accessor, other.clone(), borrow(&handle))
                        .await?
                        .unwrap(),
                    _ => <SharedCtx as Settle<_>>::nak(
                        accessor,
                        other.clone(),
                        borrow(&handle),
                        Some(1000),
                    )
                    .await?
                    .unwrap(),
                }
                assert!(matches!(
                    <SharedCtx as Settle<_>>::ack(accessor, other.clone(), borrow(&handle)).await?,
                    Err(types::NatsError::AlreadySettled)
                ));
                sync_call!(store_accessor, |active| Message::drop(
                    &mut active,
                    other.clone(),
                    handle
                ))?;
            }
            assert_eq!(conn.fetch_budget.outstanding(), 0);
            assert!(matches!(
                <SharedCtx as Pull<_>>::fetch_with_limits(
                    accessor,
                    other.clone(),
                    borrow(&consumer),
                    0,
                    0,
                    1,
                )
                .await?,
                Err(types::NatsError::Unexpected(_))
            ));
            sync_call!(store_accessor, |active| PullDrop::drop(
                &mut active,
                other.clone(),
                consumer
            ))?;
            Ok::<_, wasmtime::Error>(())
        }),
    )
    .await?;
    finished(ran)?;
    fx.plugin.connections.shutdown().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn a_dropped_handle_is_naked_and_redelivered() -> anyhow::Result<()> {
    use labeled_js::{
        HostMessageHandle as Message, HostMessageHandleWithStore as Settle,
        HostPullConsumer as PullDrop, HostPullConsumerWithStore as Pull,
    };
    let fx = fixture().await?;
    let work = fx.scope.subject("work");
    let stream = fx
        .conn
        .jetstream
        .create_stream(async_nats::jetstream::stream::Config {
            name: fx.scope.stream.clone(),
            subjects: vec![format!("{}.>", fx.scope.subjects)],
            ..Default::default()
        })
        .await?;
    stream
        .create_consumer(async_nats::jetstream::consumer::pull::Config {
            durable_name: Some("worker".into()),
            filter_subject: work.clone(),
            ..Default::default()
        })
        .await?;
    let mut store = new_store("workload", Some(&fx.plugin))?;
    let conn = &fx.conn;
    let scope = &fx.scope;
    let ran = tokio::time::timeout(
        Duration::from_secs(60),
        store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let accessor = &host_accessor;
            <SharedCtx as js::HostWithStore<_>>::publish(accessor, message(&work, b"once"))
                .await?
                .unwrap();
            let consumer = <SharedCtx as js::HostWithStore<_>>::open_pull_consumer(
                accessor,
                scope.stream.clone(),
                "worker".into(),
            )
            .await?
            .unwrap();
            let first =
                <SharedCtx as Pull<_>>::fetch(accessor, conn.clone(), borrow(&consumer), 1, 1000)
                    .await?
                    .unwrap();
            assert_eq!(first.messages.len(), 1);
            let handle = first.messages.into_iter().next().unwrap();
            let sequence = sync_call!(store_accessor, |active| Message::sequence(
                &mut active,
                conn.clone(),
                borrow(&handle)
            ))?;
            let delivered = sync_call!(store_accessor, |active| Message::delivery_count(
                &mut active,
                conn.clone(),
                borrow(&handle)
            ))?;
            assert_eq!(delivered, 1);
            assert!(conn.fetch_budget.outstanding() > 0);
            sync_call!(store_accessor, |active| Message::drop(
                &mut active,
                conn.clone(),
                handle
            ))?;
            assert_eq!(conn.fetch_budget.outstanding(), 0);

            let again =
                <SharedCtx as Pull<_>>::fetch(accessor, conn.clone(), borrow(&consumer), 1, 2000)
                    .await?
                    .unwrap();
            assert_eq!(again.messages.len(), 1);
            let handle = again.messages.into_iter().next().unwrap();
            let redelivered_sequence = sync_call!(store_accessor, |active| Message::sequence(
                &mut active,
                conn.clone(),
                borrow(&handle)
            ))?;
            assert_eq!(redelivered_sequence, sequence);
            let delivered = sync_call!(store_accessor, |active| Message::delivery_count(
                &mut active,
                conn.clone(),
                borrow(&handle)
            ))?;
            assert_eq!(delivered, 2);
            <SharedCtx as Settle<_>>::term(accessor, conn.clone(), borrow(&handle))
                .await?
                .unwrap();
            sync_call!(store_accessor, |active| Message::drop(
                &mut active,
                conn.clone(),
                handle
            ))?;
            assert_eq!(conn.fetch_budget.outstanding(), 0);
            sync_call!(store_accessor, |active| PullDrop::drop(
                &mut active,
                conn.clone(),
                consumer
            ))?;
            Ok::<_, wasmtime::Error>(())
        }),
    )
    .await?;
    finished(ran)?;
    fx.plugin.connections.shutdown().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn core_publish_and_request_map_every_refusal() -> anyhow::Result<()> {
    let fx = fixture().await?;
    let client = fx.server.client().await?;
    let echo = fx.scope.subject("echo");
    let silent = fx.scope.subject("silent");
    let mut echo_sub = client.subscribe(echo.clone()).await?;
    let responder = client.clone();
    tokio::spawn(async move {
        while let Some(msg) = echo_sub.next().await {
            let Some(reply) = msg.reply else {
                continue;
            };
            let mut headers = async_nats::HeaderMap::new();
            headers.insert("X-Echo", "yes");
            let _ = responder
                .publish_with_headers(reply, headers, msg.payload)
                .await;
        }
    });
    let _silent_sub = client.subscribe(silent.clone()).await?;
    client.flush().await?;
    let mut store = new_store("workload", Some(&fx.plugin))?;
    let conn = &fx.conn;
    let scope = &fx.scope;
    let ran = tokio::time::timeout(
        Duration::from_secs(60),
        store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let accessor = &host_accessor;
            <SharedCtx as core::HostWithStore<_>>::publish(
                accessor,
                message(&scope.subject("ok"), b"x"),
            )
            .await?
            .unwrap();

            let ungranted = format!("other.{}", uuid::Uuid::new_v4().simple());
            let denial = expect_denied(
                <SharedCtx as core::HostWithStore<_>>::publish(accessor, message(&ungranted, b"x"))
                    .await?,
            );
            assert_eq!(denial.reason, types::DenialReason::NotGranted);
            assert!(matches!(denial.target, types::DeniedResource::Subject));
            assert_eq!(denial.name, ungranted);
            assert_eq!(conn.publishes.totals(), (1, 1));

            let denial = expect_denied(
                <SharedCtx as core::HostWithStore<_>>::publish(
                    accessor,
                    types::NatsMessage {
                        reply_to: Some("$SYS.x".into()),
                        ..message(&scope.subject("ok"), b"x")
                    },
                )
                .await?,
            );
            assert_eq!(denial.reason, types::DenialReason::Reserved);
            assert!(matches!(denial.target, types::DeniedResource::Subject));
            assert_eq!(denial.name, "$SYS.x");
            assert_eq!(conn.publishes.totals(), (1, 2));

            assert!(matches!(
                <SharedCtx as core::HostWithStore<_>>::publish(
                    accessor,
                    types::NatsMessage {
                        headers: Some(vec![types::HeaderEntry {
                            name: "bad header".into(),
                            value: "v".into(),
                        }]),
                        ..message(&scope.subject("ok"), b"x")
                    },
                )
                .await?,
                Err(types::NatsError::InvalidHeader(_))
            ));

            let max_payload = conn.max_payload();
            match <SharedCtx as core::HostWithStore<_>>::publish(
                accessor,
                message(&scope.subject("ok"), &vec![0; max_payload as usize + 1]),
            )
            .await?
            {
                Err(types::NatsError::MaxPayloadExceeded(limit)) => assert_eq!(limit, max_payload),
                unexpected => panic!("expected max-payload-exceeded, got {unexpected:?}"),
            }

            assert!(matches!(
                <SharedCtx as core::HostWithStore<_>>::request(
                    accessor,
                    message(&scope.subject("nobody"), b"ping"),
                    500,
                )
                .await?,
                Err(types::NatsError::NoResponders)
            ));

            let response = <SharedCtx as core::HostWithStore<_>>::request(
                accessor,
                message(&echo, b"ping"),
                1000,
            )
            .await?
            .unwrap();
            assert_eq!(response.body, b"ping");
            assert!(response.reply_to.is_none());
            let echoed = response.headers.unwrap_or_default();
            assert!(
                echoed
                    .iter()
                    .any(|h| h.name.eq_ignore_ascii_case("x-echo") && h.value == "yes"),
                "response headers: {echoed:?}"
            );

            assert!(matches!(
                <SharedCtx as core::HostWithStore<_>>::request(
                    accessor,
                    message(&silent, b"ping"),
                    100,
                )
                .await?,
                Err(types::NatsError::Timeout(_))
            ));

            conn.grant_reply("_INBOX.abc");
            <SharedCtx as core::HostWithStore<_>>::publish(
                accessor,
                message("_INBOX.abc", b"reply"),
            )
            .await?
            .unwrap();
            let denial = expect_denied(
                <SharedCtx as core::HostWithStore<_>>::publish(
                    accessor,
                    message("_INBOX.abc", b"reply"),
                )
                .await?,
            );
            assert_eq!(denial.reason, types::DenialReason::NotGranted);
            assert_eq!(denial.name, "_INBOX.abc");
            Ok::<_, wasmtime::Error>(())
        }),
    )
    .await?;
    finished(ran)?;
    fx.plugin.connections.shutdown().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn an_unlabeled_call_with_no_unnamed_binding_is_disconnected() -> anyhow::Result<()> {
    let fx = fixture().await?;
    acquire(&fx.plugin, "labeled-only", "north", &fx.config).await?;
    let bucket = fx.scope.bucket.clone();

    let mut store = new_store("labeled-only", Some(&fx.plugin))?;
    let ran = tokio::time::timeout(
        Duration::from_secs(60),
        store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let accessor = &host_accessor;
            assert!(matches!(
                <SharedCtx as kv::HostWithStore<_>>::open(accessor, bucket.clone()).await?,
                Err(types::NatsError::Disconnected)
            ));
            assert!(matches!(
                <SharedCtx as core::HostWithStore<_>>::publish(
                    accessor,
                    message(&fx.scope.subject("ok"), b"x"),
                )
                .await?,
                Err(types::NatsError::Disconnected)
            ));
            Ok::<_, wasmtime::Error>(())
        }),
    )
    .await?;
    finished(ran)?;

    let mut store = new_store("labeled-only", None)?;
    let ran = tokio::time::timeout(
        Duration::from_secs(60),
        store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let accessor = &host_accessor;
            match <SharedCtx as kv::HostWithStore<_>>::open(accessor, bucket.clone()).await? {
                Err(types::NatsError::Unexpected(msg)) => assert_eq!(
                    msg,
                    format!("nats plugin not available: plugin {PLUGIN_NATS_ID} not found")
                ),
                unexpected => panic!("expected unexpected(..), got {unexpected:?}"),
            }
            Ok::<_, wasmtime::Error>(())
        }),
    )
    .await?;
    finished(ran)?;
    fx.plugin.connections.shutdown().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn labels_route_grants() -> anyhow::Result<()> {
    use labeled_kv::HostBucket as BucketDrop;
    let fx = fixture().await?;
    let other = deny_all(&fx).await?;
    fx.conn
        .jetstream
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: fx.scope.bucket.clone(),
            ..Default::default()
        })
        .await?;
    let mut store = new_store("workload", Some(&fx.plugin))?;
    let conn = &fx.conn;
    let scope = &fx.scope;
    let ran = tokio::time::timeout(
        Duration::from_secs(60),
        store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let accessor = &host_accessor;
            let denial = expect_denied(
                <SharedCtx as labeled_kv::HostWithStore<_>>::open(
                    accessor,
                    other.clone(),
                    scope.bucket.clone(),
                )
                .await?,
            );
            assert_eq!(denial.reason, types::DenialReason::NotGranted);
            assert!(matches!(denial.target, types::DeniedResource::Bucket));
            assert_eq!(denial.name, scope.bucket);
            let bucket = <SharedCtx as labeled_kv::HostWithStore<_>>::open(
                accessor,
                conn.clone(),
                scope.bucket.clone(),
            )
            .await?
            .unwrap();
            sync_call!(store_accessor, |active| BucketDrop::drop(
                &mut active,
                conn.clone(),
                bucket
            ))?;

            let denial = expect_denied(
                <SharedCtx as labeled_js::HostWithStore<_>>::open_pull_consumer(
                    accessor,
                    other.clone(),
                    scope.stream.clone(),
                    "worker".into(),
                )
                .await?,
            );
            assert_eq!(denial.reason, types::DenialReason::NotGranted);
            assert!(matches!(denial.target, types::DeniedResource::Stream));
            assert_eq!(denial.name, scope.stream);

            let subject = scope.subject("ok");
            let denial = expect_denied(
                <SharedCtx as labeled_core::HostWithStore<_>>::publish(
                    accessor,
                    other.clone(),
                    message(&subject, b"x"),
                )
                .await?,
            );
            assert_eq!(denial.reason, types::DenialReason::NotGranted);
            assert!(matches!(denial.target, types::DeniedResource::Subject));
            assert_eq!(denial.name, subject);
            <SharedCtx as labeled_core::HostWithStore<_>>::publish(
                accessor,
                conn.clone(),
                message(&subject, b"x"),
            )
            .await?
            .unwrap();
            assert_eq!(other.publishes.totals(), (0, 1));
            assert_eq!(conn.publishes.totals(), (1, 0));
            Ok::<_, wasmtime::Error>(())
        }),
    )
    .await?;
    finished(ran)?;
    fx.plugin.connections.shutdown().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS)"]
async fn a_bucket_writes_to_the_server_it_was_opened_on() -> anyhow::Result<()> {
    use labeled_kv::{HostBucket as BucketDrop, HostBucketWithStore as Bucket};
    #[cfg(feature = "wasi-tls")]
    crate::init_crypto();
    let north = NatsServer::container().await?;
    let south = NatsServer::container().await?;
    let scope = Scope::new();
    let config = |url: &str| {
        NatsConfig::from_map(&crate::wit::Config::from([
            ("servers", url.to_string()),
            ("bucket-allow", scope.bucket.clone()),
        ]))
    };
    let plugin = Arc::new(WasmcloudNats::new());
    let conn = acquire(&plugin, "workload", "", &config(&north.url)?).await?;
    let other = acquire(&plugin, "workload", "south", &config(&south.url)?).await?;
    let opened_on = conn
        .jetstream
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: scope.bucket.clone(),
            ..Default::default()
        })
        .await?;
    let mut store = new_store("workload", Some(&plugin))?;
    let bucket_name = scope.bucket.clone();
    let label = other.clone();
    let ran = tokio::time::timeout(
        Duration::from_secs(60),
        store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let accessor = &host_accessor;
            let bucket = <SharedCtx as kv::HostWithStore<_>>::open(accessor, bucket_name)
                .await?
                .unwrap();
            <SharedCtx as Bucket<_>>::put(
                accessor,
                label.clone(),
                borrow(&bucket),
                "item".into(),
                b"one".to_vec(),
            )
            .await?
            .unwrap();
            sync_call!(store_accessor, |active| BucketDrop::drop(
                &mut active,
                label.clone(),
                bucket
            ))?;
            Ok::<_, wasmtime::Error>(())
        }),
    )
    .await?;
    finished(ran)?;
    assert_eq!(
        opened_on.get("item").await?.as_deref(),
        Some(b"one".as_slice())
    );
    assert!(
        other
            .jetstream
            .get_key_value(scope.bucket.clone())
            .await
            .is_err()
    );
    plugin.connections.shutdown().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn kv_admin_creates_reads_and_deletes_buckets_under_their_grants() -> anyhow::Result<()> {
    use kv_admin::CreateBucketError as CreateErr;
    use labeled_kv::{HostBucket as BucketDrop, HostBucketWithStore as Bucket};
    use labeled_kv_admin::HostWithStore as Admin;
    let fx = fixture().await?;
    let name = fx.scope.bucket.clone();
    let admin_config = NatsConfig::from_map(&crate::wit::Config::from([
        ("servers", fx.server.url.clone()),
        ("bucket-allow", name.clone()),
        ("bucket-create-allow", name.clone()),
        ("bucket-delete-allow", name.clone()),
        ("bucket-max-bytes", "1048576".into()),
        ("bucket-storage-allow", "file,memory".into()),
    ]))?;
    let admin = acquire(&fx.plugin, "workload", "admin", &admin_config).await?;
    // The same grant without memory storage, which is opt-in.
    let file_only = acquire(
        &fx.plugin,
        "workload",
        "file-only",
        &NatsConfig::from_map(&crate::wit::Config::from([
            ("servers", fx.server.url.clone()),
            ("bucket-allow", name.clone()),
            ("bucket-create-allow", name.clone()),
            ("bucket-max-bytes", "1048576".into()),
        ]))?,
    )
    .await?;
    // `bucket-create-allow` without `bucket-allow` grants nothing.
    let create_only = acquire(
        &fx.plugin,
        "workload",
        "create-only",
        &NatsConfig::from_map(&crate::wit::Config::from([
            ("servers", fx.server.url.clone()),
            ("bucket-create-allow", name.clone()),
            ("bucket-max-bytes", "1048576".into()),
        ]))?,
    )
    .await?;
    // The same grant after the operator raised, then lowered, the ceiling.
    let ceiling = |id: &'static str, bytes: &'static str| {
        let config = NatsConfig::from_map(&crate::wit::Config::from([
            ("servers", fx.server.url.clone()),
            ("bucket-allow", name.clone()),
            ("bucket-create-allow", name.clone()),
            ("bucket-max-bytes", bytes.into()),
            ("bucket-storage-allow", "memory".into()),
        ]));
        let plugin = fx.plugin.clone();
        async move { acquire(&plugin, "workload", id, &config?).await }
    };
    let raised = ceiling("raised", "2097152").await?;
    let lowered = ceiling("lowered", "524288").await?;
    // `bucket-allow` alone: may read the config, may not create or delete.
    let reader = fx.conn.clone();
    let config = |max_bytes| kv_admin::BucketConfig {
        bucket: name.clone(),
        history: 3,
        max_age_ms: 0,
        max_bytes,
        max_value_size: 0,
        storage: kv_admin::StorageType::Memory,
        limit_marker_ttl_ms: Some(1_500),
    };
    let mut store = new_store("workload", Some(&fx.plugin))?;
    let ran = tokio::time::timeout(
        Duration::from_secs(60),
        store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let accessor = &host_accessor;
            let create_err = |r: Result<_, CreateErr>| match r {
                Err(CreateErr::Nats(e)) => Err(e),
                Err(CreateErr::ConfigConflict(c)) => panic!("unexpected conflict: {c:?}"),
                Ok(handle) => Ok(handle),
            };

            for conn in [reader.clone(), create_only.clone()] {
                let denial = expect_denied(create_err(
                    <SharedCtx as Admin<_>>::create_bucket(accessor, conn, config(0)).await?,
                ));
                assert!(matches!(denial.reason, types::DenialReason::NotGranted));
                assert!(matches!(denial.target, types::DeniedResource::Bucket));
                assert_eq!(denial.name, name);
            }

            assert!(matches!(
                <SharedCtx as Admin<_>>::create_bucket(accessor, admin.clone(), config(2 << 20))
                    .await?,
                Err(CreateErr::Nats(types::NatsError::LimitExceeded(_)))
            ));
            match create_err(
                <SharedCtx as Admin<_>>::create_bucket(accessor, file_only.clone(), config(0))
                    .await?,
            ) {
                Err(types::NatsError::LimitExceeded(m)) => {
                    assert!(m.contains("bucket-storage-allow"), "{m}")
                }
                other => panic!("expected memory storage to be refused, got {other:?}"),
            }

            let bucket = <SharedCtx as Admin<_>>::create_bucket(accessor, admin.clone(), config(0))
                .await?
                .unwrap();
            // Same config again opens the existing bucket.
            let again = <SharedCtx as Admin<_>>::create_bucket(accessor, admin.clone(), config(0))
                .await?
                .unwrap();
            // A different one is refused with what the bucket really has.
            match <SharedCtx as Admin<_>>::create_bucket(accessor, admin.clone(), config(4096))
                .await?
            {
                Err(CreateErr::ConfigConflict(existing)) => {
                    assert_eq!(existing.bucket, name);
                    assert_eq!(existing.max_bytes, 1 << 20);
                    assert_eq!(existing.history, 3);
                    assert_eq!(existing.limit_marker_ttl_ms, Some(2_000));
                }
                other => panic!("expected a config conflict, got {other:?}"),
            }

            // `max-bytes: 0` left the size to the host: a raised ceiling still
            // opens the 1 MiB bucket, a lowered one it no longer fits under
            // does not.
            let reopened =
                <SharedCtx as Admin<_>>::create_bucket(accessor, raised.clone(), config(0))
                    .await?
                    .unwrap();
            match <SharedCtx as Admin<_>>::create_bucket(accessor, lowered.clone(), config(0))
                .await?
            {
                Err(CreateErr::ConfigConflict(existing)) => {
                    assert_eq!(existing.max_bytes, 1 << 20)
                }
                other => panic!("expected a config conflict, got {other:?}"),
            }
            // Asking for its exact size is over the lowered ceiling, but the
            // bucket exists, so that is a conflict too, not a limit.
            match <SharedCtx as Admin<_>>::create_bucket(accessor, lowered.clone(), config(1 << 20))
                .await?
            {
                Err(CreateErr::ConfigConflict(existing)) => {
                    assert_eq!(existing.max_bytes, 1 << 20)
                }
                other => panic!("expected a config conflict, got {other:?}"),
            }

            let read =
                <SharedCtx as Admin<_>>::get_bucket_config(accessor, reader.clone(), name.clone())
                    .await?
                    .unwrap();
            assert_eq!(read.bucket, name);
            assert_eq!(read.history, 3);
            assert_eq!(read.max_age_ms, 0);
            // 0 took the host's `bucket-max-bytes`.
            assert_eq!(read.max_bytes, 1 << 20);
            assert_eq!(read.max_value_size, 0);
            assert!(matches!(read.storage, kv_admin::StorageType::Memory));
            // Rounded up to whole seconds.
            assert_eq!(read.limit_marker_ttl_ms, Some(2_000));

            // The marker TTL is what lets per-key TTL work on the new bucket.
            <SharedCtx as Bucket<_>>::create_with_ttl(
                accessor,
                admin.clone(),
                borrow(&bucket),
                "lease".into(),
                b"held".to_vec(),
                1_000,
            )
            .await?
            .unwrap();

            expect_denied(
                <SharedCtx as Admin<_>>::delete_bucket(accessor, reader.clone(), name.clone())
                    .await?,
            );
            <SharedCtx as Admin<_>>::delete_bucket(accessor, admin.clone(), name.clone())
                .await?
                .unwrap();
            expect_not_found(
                <SharedCtx as Admin<_>>::delete_bucket(accessor, admin.clone(), name.clone())
                    .await?,
            );
            expect_not_found(
                <SharedCtx as Admin<_>>::get_bucket_config(accessor, reader.clone(), name.clone())
                    .await?,
            );
            for handle in [bucket, again, reopened] {
                sync_call!(store_accessor, |active| BucketDrop::drop(
                    &mut active,
                    admin.clone(),
                    handle
                ))?;
            }
            Ok::<_, wasmtime::Error>(())
        }),
    )
    .await?;
    finished(ran)?;
    fx.plugin.connections.shutdown().await;
    Ok(())
}

/// No size ceiling, file storage, and every limit a guest can set, read back
/// from the server; then the inputs only the server refuses.
#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn kv_admin_applies_every_requested_limit() -> anyhow::Result<()> {
    use kv_admin::CreateBucketError as CreateErr;
    use labeled_kv::HostBucket as BucketDrop;
    use labeled_kv_admin::HostWithStore as Admin;
    let fx = fixture().await?;
    let file = format!("{}-file", fx.scope.bucket);
    let replicated = format!("{}-r3", fx.scope.bucket);
    let open = |extra: &[(&'static str, String)]| {
        let mut map = crate::wit::Config::from([
            ("servers", fx.server.url.clone()),
            ("bucket-allow", ">".to_string()),
            (
                "bucket-create-allow",
                format!("{file},{replicated},not.valid"),
            ),
            ("bucket-delete-allow", ">".to_string()),
            ("bucket-max-bytes", "unlimited".to_string()),
        ]);
        for (k, v) in extra {
            map.insert(k.to_string(), v.clone());
        }
        NatsConfig::from_map(&map)
    };
    let unbounded = acquire(&fx.plugin, "workload", "unbounded", &open(&[])?).await?;
    let three = acquire(
        &fx.plugin,
        "workload",
        "three",
        &open(&[("bucket-replicas", "3".into())])?,
    )
    .await?;
    assert!(fx.conn.policy.bucket_replicas().is_none());
    let request = |bucket: &str| kv_admin::BucketConfig {
        bucket: bucket.to_string(),
        history: 0,
        max_age_ms: 90_000,
        max_bytes: 0,
        max_value_size: 1024,
        storage: kv_admin::StorageType::File,
        limit_marker_ttl_ms: None,
    };
    let js = async_nats::jetstream::new(fx.server.client().await?);
    let mut store = new_store("workload", Some(&fx.plugin))?;
    let ran = tokio::time::timeout(
        Duration::from_secs(60),
        store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let accessor = &host_accessor;

            let handle =
                <SharedCtx as Admin<_>>::create_bucket(accessor, unbounded.clone(), request(&file))
                    .await?
                    .unwrap();
            let read = <SharedCtx as Admin<_>>::get_bucket_config(
                accessor,
                unbounded.clone(),
                file.clone(),
            )
            .await?
            .unwrap();
            assert_eq!(read.history, 1, "history 0 means 1");
            assert_eq!(read.max_age_ms, 90_000);
            assert_eq!(read.max_bytes, 0, "no ceiling and 0 requested is unlimited");
            assert_eq!(read.max_value_size, 1024);
            assert!(matches!(read.storage, kv_admin::StorageType::File));
            assert_eq!(read.limit_marker_ttl_ms, None);
            sync_call!(store_accessor, |active| BucketDrop::drop(
                &mut active,
                unbounded.clone(),
                handle
            ))?;
            <SharedCtx as Admin<_>>::delete_bucket(accessor, unbounded.clone(), file.clone())
                .await?
                .unwrap();

            // Granted, but not a name a bucket can have.
            match <SharedCtx as Admin<_>>::create_bucket(
                accessor,
                unbounded.clone(),
                request("not.valid"),
            )
            .await?
            {
                Err(CreateErr::Nats(types::NatsError::Jetstream(m))) => {
                    assert!(m.contains("not.valid"), "{m}")
                }
                other => panic!("expected an invalid-name refusal, got {other:?}"),
            }

            // The host's replica count reaches the server: a single node
            // refuses it, a cluster keeps three copies.
            match <SharedCtx as Admin<_>>::create_bucket(
                accessor,
                three.clone(),
                request(&replicated),
            )
            .await?
            {
                Err(CreateErr::Nats(types::NatsError::Jetstream(m))) => {
                    assert!(m.to_lowercase().contains("replicas"), "{m}")
                }
                Ok(handle) => {
                    let stream = js.get_key_value(&replicated).await?.stream;
                    assert_eq!(stream.cached_info().config.num_replicas, 3);
                    sync_call!(store_accessor, |active| BucketDrop::drop(
                        &mut active,
                        three.clone(),
                        handle
                    ))?;
                    let _ = js.delete_key_value(&replicated).await;
                }
                other => panic!("expected the replica count to reach the server, got {other:?}"),
            }
            Ok::<_, wasmtime::Error>(())
        }),
    )
    .await?;
    finished(ran)?;
    fx.plugin.connections.shutdown().await;
    Ok(())
}

/// Events naming one bucket, as `(level, message)`.
#[derive(Clone, Default)]
struct BucketEvents {
    bucket: String,
    seen: Arc<std::sync::Mutex<Vec<(tracing::Level, String)>>>,
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for BucketEvents {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        #[derive(Default)]
        struct Fields {
            bucket: Option<String>,
            message: String,
        }
        impl tracing::field::Visit for Fields {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                if field.name() == "bucket" {
                    self.bucket = Some(value.to_string());
                }
            }
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                match field.name() {
                    "message" => self.message = format!("{value:?}"),
                    "bucket" => self.bucket = Some(format!("{value:?}").trim_matches('"').into()),
                    _ => {}
                }
            }
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        if fields.bucket.as_deref() == Some(self.bucket.as_str()) {
            self.seen
                .lock()
                .unwrap()
                .push((*event.metadata().level(), fields.message));
        }
    }
}

/// Only a create that makes the bucket logs it as created; ensuring a live
/// bucket, or being refused one, does not. The write between the creates
/// keeps the second off the server's clock.
#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn kv_admin_logs_a_bucket_as_created_only_when_it_is() -> anyhow::Result<()> {
    use kv_admin::CreateBucketError as CreateErr;
    use labeled_kv::{HostBucket as BucketDrop, HostBucketWithStore as Bucket};
    use labeled_kv_admin::HostWithStore as Admin;
    use tracing::Level;
    use tracing_subscriber::layer::SubscriberExt as _;
    const CHILD: &str = "WASH_TEST_KV_ADMIN_CREATE_LOG";

    // Callsite interest is process-global. Capture events without parallel tests
    // registering the same callsites under a different default subscriber.
    if std::env::var_os(CHILD).is_none() {
        let (_, module) = module_path!().split_once("::").unwrap();
        let test = format!("{module}::kv_admin_logs_a_bucket_as_created_only_when_it_is");
        let output = std::process::Command::new(std::env::current_exe()?)
            .args(["--exact", &test, "--include-ignored", "--nocapture"])
            .env(CHILD, "1")
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "kv-admin create log capture failed:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return Ok(());
    }

    let fx = fixture().await?;
    let name = fx.scope.bucket.clone();
    let admin = acquire(
        &fx.plugin,
        "workload",
        "admin",
        &NatsConfig::from_map(&crate::wit::Config::from([
            ("servers", fx.server.url.clone()),
            ("bucket-allow", name.clone()),
            ("bucket-create-allow", name.clone()),
            ("bucket-delete-allow", name.clone()),
            ("bucket-max-bytes", "1048576".into()),
            ("bucket-storage-allow", "memory".into()),
        ]))?,
    )
    .await?;
    let config = |max_bytes| kv_admin::BucketConfig {
        bucket: name.clone(),
        history: 1,
        max_age_ms: 0,
        max_bytes,
        max_value_size: 0,
        storage: kv_admin::StorageType::Memory,
        limit_marker_ttl_ms: None,
    };
    let events = BucketEvents {
        bucket: name.clone(),
        ..Default::default()
    };
    let _guard =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(events.clone()));
    let mut store = new_store("workload", Some(&fx.plugin))?;
    let ran = tokio::time::timeout(
        Duration::from_secs(60),
        store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let accessor = &host_accessor;
            let created =
                <SharedCtx as Admin<_>>::create_bucket(accessor, admin.clone(), config(0))
                    .await?
                    .unwrap();
            <SharedCtx as Bucket<_>>::put(
                accessor,
                admin.clone(),
                borrow(&created),
                "k".into(),
                b"v".to_vec(),
            )
            .await?
            .unwrap();
            let ensured =
                <SharedCtx as Admin<_>>::create_bucket(accessor, admin.clone(), config(0))
                    .await?
                    .unwrap();
            assert!(matches!(
                <SharedCtx as Admin<_>>::create_bucket(accessor, admin.clone(), config(4096))
                    .await?,
                Err(CreateErr::ConfigConflict(_))
            ));
            for handle in [created, ensured] {
                sync_call!(store_accessor, |active| BucketDrop::drop(
                    &mut active,
                    admin.clone(),
                    handle
                ))?;
            }
            <SharedCtx as Admin<_>>::delete_bucket(accessor, admin.clone(), name.clone())
                .await?
                .unwrap();
            Ok::<_, wasmtime::Error>(())
        }),
    )
    .await?;
    finished(ran)?;
    assert_eq!(
        *events.seen.lock().unwrap(),
        [
            (Level::INFO, "wasmcloud:nats created KV bucket".to_string()),
            (
                Level::DEBUG,
                "wasmcloud:nats opened existing KV bucket".to_string()
            ),
            (Level::INFO, "wasmcloud:nats deleted KV bucket".to_string()),
        ]
    );
    fx.plugin.connections.shutdown().await;
    Ok(())
}

/// Credentials that may create a bucket but not read stream info still
/// create it, and ensure it again.
#[tokio::test]
#[ignore = "requires Docker (NATS)"]
async fn kv_admin_creates_without_stream_info_permission() -> anyhow::Result<()> {
    use labeled_kv::HostBucket as BucketDrop;
    use labeled_kv_admin::HostWithStore as Admin;
    let name = format!("CONTRACT-{}", uuid::Uuid::new_v4().simple());
    let server = NatsServer::with_config(&format!(
        r#"
listen: 0.0.0.0:4222
jetstream {{ store_dir: "/tmp/js" }}
authorization {{
  users: [{{
    user: creator, password: creator,
    permissions: {{
      publish: {{ allow: ["$JS.API.INFO", "$JS.API.STREAM.CREATE.KV_{name}"], deny: ["$JS.API.STREAM.INFO.>"] }},
      subscribe: {{ allow: [">"] }}
    }}
  }}]
}}
"#
    ))
    .await?;
    let plugin = Arc::new(WasmcloudNats::new());
    let creator = acquire(
        &plugin,
        "workload",
        "creator",
        &NatsConfig::from_map(&crate::wit::Config::from([
            ("servers", server.url.clone()),
            ("username", "creator".into()),
            ("password", "creator".into()),
            ("bucket-allow", name.clone()),
            ("bucket-create-allow", name.clone()),
            ("bucket-max-bytes", "1048576".into()),
            ("bucket-storage-allow", "memory".into()),
            ("request-timeout-ms", "1000".into()),
        ]))?,
    )
    .await?;
    let config = kv_admin::BucketConfig {
        bucket: name.clone(),
        history: 1,
        max_age_ms: 0,
        max_bytes: 0,
        max_value_size: 0,
        storage: kv_admin::StorageType::Memory,
        limit_marker_ttl_ms: None,
    };
    let mut store = new_store("workload", Some(&plugin))?;
    let ran = tokio::time::timeout(
        Duration::from_secs(60),
        store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let accessor = &host_accessor;
            for _ in 0..2 {
                let handle = <SharedCtx as Admin<_>>::create_bucket(
                    accessor,
                    creator.clone(),
                    config.clone(),
                )
                .await?
                .unwrap_or_else(|e| panic!("create without stream info failed: {e:?}"));
                sync_call!(store_accessor, |active| BucketDrop::drop(
                    &mut active,
                    creator.clone(),
                    handle
                ))?;
            }
            Ok::<_, wasmtime::Error>(())
        }),
    )
    .await?;
    finished(ran)?;
    plugin.connections.shutdown().await;
    Ok(())
}
