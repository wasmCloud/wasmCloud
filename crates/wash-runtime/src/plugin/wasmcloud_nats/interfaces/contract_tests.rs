use super::*;
use crate::engine::ctx::{Ctx, extract_active_ctx};
use crate::plugin::{
    HostPlugin,
    wasmcloud_nats::{PLUGIN_NATS_ID, WasmcloudNats, config::NatsConfig},
};
use std::collections::HashMap;
use testcontainers::{
    GenericImage, ImageExt as _,
    core::{IntoContainerPort as _, WaitFor},
    runners::AsyncRunner as _,
};
use wasmtime::component::Resource;

fn borrow<T: 'static>(resource: &Resource<T>) -> Resource<T> {
    Resource::new_borrow(resource.rep())
}

fn message(subject: &str, body: &[u8]) -> types::NatsMessage {
    types::NatsMessage {
        subject: subject.into(),
        body: body.into(),
        reply_to: None,
        headers: None,
    }
}

#[tokio::test]
#[ignore = "requires Docker (NATS)"]
async fn named_resources_keep_their_connection_and_settlement_contract() {
    #[cfg(feature = "wasi-tls")]
    crate::init_crypto();
    use labeled_js::{
        HostMessageHandle as Message, HostMessageHandleWithStore as Settle,
        HostPullConsumer as PullDrop, HostPullConsumerWithStore as Pull,
    };
    use labeled_kv::{HostBucket as BucketDrop, HostBucketWithStore as Bucket};
    let server = GenericImage::new("nats", "2.12.8-alpine")
        .with_exposed_port(4222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .with_cmd(["-js"])
        .start()
        .await
        .unwrap();
    let url = format!(
        "nats://127.0.0.1:{}",
        server.get_host_port_ipv4(4222).await.unwrap()
    );
    let plugin = Arc::new(WasmcloudNats::new());
    let config = NatsConfig::from_map(&HashMap::from([
        ("servers".into(), url),
        ("subject-allow".into(), "contract.>".into()),
        ("stream-allow".into(), "CONTRACT".into()),
        ("stream-delete-allow".into(), "CONTRACT".into()),
        ("bucket-allow".into(), "CONTRACT".into()),
        ("request-timeout-ms".into(), "1000".into()),
    ]))
    .unwrap();
    let conn = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(conn) = plugin
                .connections
                .acquire("workload", "", &config, vec![], true)
                .await
            {
                break conn;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let mut denied = NatsConfig::from_map(&HashMap::from([(
        "servers".into(),
        config.servers.join(","),
    )]))
    .unwrap();
    denied.request_timeout = Some(Duration::from_secs(1));
    let other = plugin
        .connections
        .acquire("other", "other", &denied, vec![], true)
        .await
        .unwrap();
    conn.jetstream
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: "CONTRACT".into(),
            history: 10,
            ..Default::default()
        })
        .await
        .unwrap();
    let stream = conn
        .jetstream
        .create_stream(async_nats::jetstream::stream::Config {
            name: "CONTRACT".into(),
            subjects: vec!["contract.>".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    stream
        .create_consumer(async_nats::jetstream::consumer::pull::Config {
            durable_name: Some("worker".into()),
            filter_subject: "contract.work".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let host_plugin: Arc<dyn HostPlugin + Send + Sync> = plugin.clone();
    let ctx = Ctx::builder("workload", "component")
        .with_plugins(HashMap::from([(PLUGIN_NATS_ID, host_plugin)]))
        .build();
    let mut config = wasmtime::Config::new();
    config
        .wasm_component_model_async(true)
        .concurrency_support(true);
    let engine = wasmtime::Engine::new(&config).unwrap();
    let mut store = wasmtime::Store::new(&engine, SharedCtx::new(ctx));
    tokio::time::timeout(
        Duration::from_secs(30),
        store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let accessor = &host_accessor;
            let bucket = <SharedCtx as kv::HostWithStore<_>>::open(accessor, "CONTRACT".into())
                .await
                .unwrap()
                .unwrap();
            let revision = <SharedCtx as Bucket<_>>::put(
                accessor,
                other.clone(),
                borrow(&bucket),
                "item".into(),
                b"one".to_vec(),
            )
            .await
            .unwrap()
            .unwrap();
            let value = <SharedCtx as Bucket<_>>::get(
                accessor,
                other.clone(),
                borrow(&bucket),
                "item".into(),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!((value.revision, value.value), (revision, b"one".to_vec()));
            assert!(matches!(
                <SharedCtx as Bucket<_>>::create(
                    accessor,
                    other.clone(),
                    borrow(&bucket),
                    "item".into(),
                    vec![]
                )
                .await
                .unwrap(),
                Err(types::NatsError::Jetstream(_))
            ));
            assert!(matches!(
                <SharedCtx as Bucket<_>>::update(
                    accessor,
                    other.clone(),
                    borrow(&bucket),
                    "item".into(),
                    vec![],
                    revision + 10
                )
                .await
                .unwrap(),
                Err(types::NatsError::RevisionMismatch(_))
            ));
            let updated = <SharedCtx as Bucket<_>>::update(
                accessor,
                other.clone(),
                borrow(&bucket),
                "item".into(),
                b"two".to_vec(),
                revision,
            )
            .await
            .unwrap()
            .unwrap();
            assert!(updated > revision);
            assert_eq!(
                <SharedCtx as Bucket<_>>::history(
                    accessor,
                    other.clone(),
                    borrow(&bucket),
                    "item".into()
                )
                .await
                .unwrap()
                .unwrap()
                .len(),
                2
            );
            assert!(
                <SharedCtx as Bucket<_>>::status(accessor, other.clone(), borrow(&bucket))
                    .await
                    .unwrap()
                    .is_ok()
            );
            assert!(
                <SharedCtx as Bucket<_>>::keys(
                    accessor,
                    other.clone(),
                    borrow(&bucket),
                    ">".into()
                )
                .await
                .unwrap()
                .is_ok()
            );
            assert!(
                <SharedCtx as Bucket<_>>::keys(
                    accessor,
                    other.clone(),
                    borrow(&bucket),
                    "bad..filter".into()
                )
                .await
                .unwrap()
                .is_err()
            );
            assert!(
                <SharedCtx as Bucket<_>>::create_with_ttl(
                    accessor,
                    other.clone(),
                    borrow(&bucket),
                    "lease".into(),
                    vec![],
                    1
                )
                .await
                .unwrap()
                .is_err()
            );
            <SharedCtx as Bucket<_>>::delete_by_revision(
                accessor,
                other.clone(),
                borrow(&bucket),
                "item".into(),
                updated,
            )
            .await
            .unwrap()
            .unwrap();
            assert!(matches!(
                <SharedCtx as Bucket<_>>::get(
                    accessor,
                    other.clone(),
                    borrow(&bucket),
                    "item".into()
                )
                .await
                .unwrap(),
                Err(types::NatsError::KeyNotFound)
            ));
            <SharedCtx as Bucket<_>>::create(
                accessor,
                other.clone(),
                borrow(&bucket),
                "delete".into(),
                vec![1],
            )
            .await
            .unwrap()
            .unwrap();
            <SharedCtx as Bucket<_>>::delete(
                accessor,
                other.clone(),
                borrow(&bucket),
                "delete".into(),
            )
            .await
            .unwrap()
            .unwrap();
            <SharedCtx as Bucket<_>>::purge(
                accessor,
                other.clone(),
                borrow(&bucket),
                "delete".into(),
            )
            .await
            .unwrap()
            .unwrap();
            store_accessor.with(|mut access| {
                let mut active = extract_active_ctx(access.get());
                futures::executor::block_on(BucketDrop::drop(&mut active, other.clone(), bucket))
                    .unwrap();
            });

            for number in 0..5 {
                <SharedCtx as js::HostWithStore<_>>::publish(
                    accessor,
                    message("contract.work", &[number]),
                )
                .await
                .unwrap()
                .unwrap();
            }
            let consumer = <SharedCtx as js::HostWithStore<_>>::open_pull_consumer(
                accessor,
                "CONTRACT".into(),
                "worker".into(),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(
                <SharedCtx as Pull<_>>::info(accessor, other.clone(), borrow(&consumer))
                    .await
                    .unwrap()
                    .is_ok()
            );
            let batch =
                <SharedCtx as Pull<_>>::fetch(accessor, other.clone(), borrow(&consumer), 5, 1000)
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(batch.messages.len(), 5);
            for (i, handle) in batch.messages.into_iter().enumerate() {
                store_accessor.with(|mut access| {
                    let mut active = extract_active_ctx(access.get());
                    let msg = futures::executor::block_on(Message::message(
                        &mut active,
                        other.clone(),
                        borrow(&handle),
                    ))
                    .unwrap();
                    assert_eq!(msg.body, [i as u8]);
                    assert!(
                        futures::executor::block_on(Message::sequence(
                            &mut active,
                            other.clone(),
                            borrow(&handle)
                        ))
                        .unwrap()
                            > 0
                    );
                    assert!(
                        futures::executor::block_on(Message::stored_at_unix_nanos(
                            &mut active,
                            other.clone(),
                            borrow(&handle)
                        ))
                        .unwrap()
                            > 0
                    );
                    assert_eq!(
                        futures::executor::block_on(Message::delivery_count(
                            &mut active,
                            other.clone(),
                            borrow(&handle)
                        ))
                        .unwrap(),
                        1
                    );
                });
                <SharedCtx as Settle<_>>::in_progress(accessor, other.clone(), borrow(&handle))
                    .await
                    .unwrap()
                    .unwrap();
                match i {
                    0 => {
                        <SharedCtx as Settle<_>>::ack_sync(accessor, other.clone(), borrow(&handle))
                            .await
                            .unwrap()
                            .unwrap()
                    }
                    1 => <SharedCtx as Settle<_>>::ack(accessor, other.clone(), borrow(&handle))
                        .await
                        .unwrap()
                        .unwrap(),
                    2 => <SharedCtx as Settle<_>>::term(accessor, other.clone(), borrow(&handle))
                        .await
                        .unwrap()
                        .unwrap(),
                    _ => <SharedCtx as Settle<_>>::nak(
                        accessor,
                        other.clone(),
                        borrow(&handle),
                        Some(1000),
                    )
                    .await
                    .unwrap()
                    .unwrap(),
                }
                store_accessor.with(|mut access| {
                    let mut active = extract_active_ctx(access.get());
                    futures::executor::block_on(Message::drop(&mut active, other.clone(), handle))
                        .unwrap();
                });
            }
            let batch = <SharedCtx as Pull<_>>::fetch_with_limits(
                accessor,
                other.clone(),
                borrow(&consumer),
                0,
                0,
                1,
            )
            .await
            .unwrap();
            assert!(batch.is_err());
            store_accessor.with(|mut access| {
                let mut active = extract_active_ctx(access.get());
                futures::executor::block_on(PullDrop::drop(&mut active, other.clone(), consumer))
                    .unwrap();
            });
            Ok::<_, wasmtime::Error>(())
        }),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    plugin.connections.shutdown().await;
}
