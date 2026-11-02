use super::super::contract_tests::{
    Fixture, expect_denied, expect_not_found, finished, fixture, message, new_store,
};
use super::*;
use crate::engine::ctx::extract_active_ctx;
use labeled_js::HostWithStore as _;

/// A stream over the scope's `stream.>` subjects plus one the grant does not
/// cover, so every read has messages it must withhold.
async fn stream_with_hidden_subjects(fx: &Fixture) -> anyhow::Result<jetstream::stream::Stream> {
    Ok(fx
        .conn
        .jetstream
        .create_stream(jetstream::stream::Config {
            name: fx.scope.stream.clone(),
            subjects: vec![fx.scope.subject("stream.>"), hidden(fx)],
            allow_direct: true,
            ..Default::default()
        })
        .await?)
}

fn hidden(fx: &Fixture) -> String {
    format!("hidden.{}.>", fx.scope.stream)
}

/// Publishes around the plugin, which is the only way onto a subject the
/// workload is not granted.
async fn publish_raw(fx: &Fixture, subject: String, body: &'static [u8]) -> anyhow::Result<u64> {
    Ok(fx
        .conn
        .jetstream
        .publish(subject, body.into())
        .await?
        .await?
        .sequence)
}

#[test]
fn stored_times_clamp_into_unsigned_nanoseconds() {
    assert_eq!(unix_nanos(-1), 0);
    assert_eq!(unix_nanos(1_500), 1_500);
    assert_eq!(unix_nanos(i128::MAX), u64::MAX);
}

#[test]
fn an_unfiltered_consumer_captures_the_whole_stream() {
    let mut config = jetstream::consumer::Config::default();
    assert_eq!(consumer_filters(&config), [">"]);
    config.filter_subjects = vec!["a".into(), "b".into()];
    assert_eq!(consumer_filters(&config), ["a", "b"]);
    config.filter_subject = "c".into();
    assert_eq!(consumer_filters(&config), ["c"]);
}

#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn every_stream_call_names_a_missing_or_ungranted_stream() -> anyhow::Result<()> {
    let fx = fixture().await?;
    let mut store = new_store("workload", Some(&fx.plugin))?;
    let conn = &fx.conn;
    let stream = fx.scope.stream.clone();
    let subject = fx.scope.subject("stream.a");
    let ran = tokio::time::timeout(
        Duration::from_secs(60),
        store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let accessor = &host_accessor;
            let s = || stream.clone();
            expect_not_found(SharedCtx::get_stream_info(accessor, conn.clone(), s()).await?);
            expect_not_found(SharedCtx::get_stream_created(accessor, conn.clone(), s()).await?);
            expect_not_found(SharedCtx::get_by_sequence(accessor, conn.clone(), s(), 1).await?);
            expect_not_found(SharedCtx::get_by_sequence_at(accessor, conn.clone(), s(), 1).await?);
            expect_not_found(
                SharedCtx::get_last_by_subject(accessor, conn.clone(), s(), subject.clone())
                    .await?,
            );
            expect_not_found(SharedCtx::scan(accessor, conn.clone(), s(), 1, 1).await?);
            expect_not_found(
                SharedCtx::list_stream_subjects(accessor, conn.clone(), s(), String::new()).await?,
            );
            expect_not_found(SharedCtx::delete_message(accessor, conn.clone(), s(), 1).await?);
            expect_not_found(
                SharedCtx::purge_subject(accessor, conn.clone(), s(), subject.clone(), None)
                    .await?,
            );
            expect_not_found(
                SharedCtx::open_pull_consumer(accessor, conn.clone(), s(), "worker".into()).await?,
            );
            expect_not_found(
                SharedCtx::get_consumer_info(accessor, conn.clone(), s(), "worker".into()).await?,
            );

            let other = || "UNGRANTED".to_string();
            for denial in [
                expect_denied(SharedCtx::get_stream_info(accessor, conn.clone(), other()).await?),
                expect_denied(
                    SharedCtx::get_stream_created(accessor, conn.clone(), other()).await?,
                ),
                expect_denied(
                    SharedCtx::get_by_sequence(accessor, conn.clone(), other(), 1).await?,
                ),
                expect_denied(
                    SharedCtx::get_by_sequence_at(accessor, conn.clone(), other(), 1).await?,
                ),
                expect_denied(
                    SharedCtx::get_last_by_subject(
                        accessor,
                        conn.clone(),
                        other(),
                        subject.clone(),
                    )
                    .await?,
                ),
                expect_denied(SharedCtx::scan(accessor, conn.clone(), other(), 1, 1).await?),
                expect_denied(
                    SharedCtx::list_stream_subjects(accessor, conn.clone(), other(), String::new())
                        .await?,
                ),
                expect_denied(SharedCtx::delete_message(accessor, conn.clone(), other(), 1).await?),
                expect_denied(
                    SharedCtx::purge_subject(
                        accessor,
                        conn.clone(),
                        other(),
                        subject.clone(),
                        None,
                    )
                    .await?,
                ),
                expect_denied(
                    SharedCtx::get_consumer_info(accessor, conn.clone(), other(), "worker".into())
                        .await?,
                ),
            ] {
                assert_eq!(denial.reason, types::DenialReason::NotGranted);
                assert!(matches!(denial.target, types::DeniedResource::Stream));
                assert_eq!(denial.name, "UNGRANTED");
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
async fn publish_maps_every_refusal() -> anyhow::Result<()> {
    let fx = fixture().await?;
    stream_with_hidden_subjects(&fx).await?;
    let mut store = new_store("workload", Some(&fx.plugin))?;
    let conn = &fx.conn;
    let scope = &fx.scope;
    let ran = tokio::time::timeout(
        Duration::from_secs(60),
        store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let accessor = &host_accessor;
            let subject = scope.subject("stream.a");
            let mut headed = message(&subject, b"one");
            headed.reply_to = Some(scope.subject("ignored"));
            headed.headers = Some(vec![types::HeaderEntry {
                name: "X-Test".into(),
                value: "1".into(),
            }]);
            let ack = SharedCtx::publish(accessor, conn.clone(), headed)
                .await?
                .unwrap();
            assert_eq!(ack.stream_name, scope.stream);
            assert_eq!(ack.sequence, 1);
            assert!(!ack.duplicate);

            let denial = expect_denied(
                SharedCtx::publish(accessor, conn.clone(), message("elsewhere.a", b"x")).await?,
            );
            assert!(matches!(denial.target, types::DeniedResource::Subject));
            assert_eq!(denial.name, "elsewhere.a");

            let mut bad_header = message(&subject, b"x");
            bad_header.headers = Some(vec![types::HeaderEntry {
                name: "bad name".into(),
                value: "x".into(),
            }]);
            assert!(matches!(
                SharedCtx::publish(accessor, conn.clone(), bad_header).await?,
                Err(types::NatsError::InvalidHeader(_))
            ));

            let max = conn.max_payload();
            let oversized = vec![0; usize::try_from(max)? + 1];
            match SharedCtx::publish(accessor, conn.clone(), message(&subject, &oversized)).await? {
                Err(types::NatsError::MaxPayloadExceeded(limit)) => assert_eq!(limit, max),
                unexpected => panic!("expected max-payload-exceeded, got {unexpected:?}"),
            }

            expect_not_found(
                SharedCtx::publish(
                    accessor,
                    conn.clone(),
                    message(&scope.subject("loose"), b"x"),
                )
                .await?,
            );
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
async fn stream_reads_withhold_subjects_outside_the_grant() -> anyhow::Result<()> {
    let fx = fixture().await?;
    stream_with_hidden_subjects(&fx).await?;
    let a = fx.scope.subject("stream.a");
    let b = fx.scope.subject("stream.b");
    let hidden_subject = format!("hidden.{}.x", fx.scope.stream);
    assert_eq!(publish_raw(&fx, a.clone(), b"a1").await?, 1);
    assert_eq!(publish_raw(&fx, hidden_subject.clone(), b"h1").await?, 2);
    assert_eq!(publish_raw(&fx, b.clone(), b"b1").await?, 3);
    assert_eq!(publish_raw(&fx, a.clone(), b"a2").await?, 4);
    assert_eq!(publish_raw(&fx, hidden_subject.clone(), b"h2").await?, 5);

    let mut store = new_store("workload", Some(&fx.plugin))?;
    let conn = &fx.conn;
    let stream = fx.scope.stream.clone();
    let hidden_filter = hidden(&fx);
    let none = fx.scope.subject("stream.none");
    let ran = tokio::time::timeout(
        Duration::from_secs(60),
        store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let accessor = &host_accessor;
            let s = || stream.clone();
            let withheld = |denial: types::Denial, sequence: u64| {
                assert_eq!(denial.reason, types::DenialReason::NotGranted);
                assert!(
                    matches!(denial.target, types::DeniedResource::Message(seq) if seq == sequence)
                );
                assert_eq!(denial.name, stream);
            };

            let info = SharedCtx::get_stream_info(accessor, conn.clone(), s())
                .await?
                .unwrap();
            assert_eq!(info.name, stream);
            assert_eq!(info.messages, 5);
            assert_eq!((info.first_sequence, info.last_sequence), (1, 5));
            assert_eq!(info.consumer_count, 0);
            assert!(
                SharedCtx::get_stream_created(accessor, conn.clone(), s())
                    .await?
                    .unwrap()
                    > 0
            );

            let first = SharedCtx::get_by_sequence(accessor, conn.clone(), s(), 1)
                .await?
                .unwrap();
            assert_eq!(
                (first.subject.as_str(), first.data.as_slice()),
                (a.as_str(), &b"a1"[..])
            );
            withheld(
                expect_denied(SharedCtx::get_by_sequence(accessor, conn.clone(), s(), 2).await?),
                2,
            );
            assert!(matches!(
                SharedCtx::get_by_sequence(accessor, conn.clone(), s(), 99).await?,
                Err(types::NatsError::Jetstream(_))
            ));

            let at = SharedCtx::get_by_sequence_at(accessor, conn.clone(), s(), 3)
                .await?
                .unwrap();
            assert_eq!(at.message.subject, b);
            assert!(at.stored_at_unix_nanos > 0);
            withheld(
                expect_denied(SharedCtx::get_by_sequence_at(accessor, conn.clone(), s(), 5).await?),
                5,
            );
            expect_not_found(SharedCtx::get_by_sequence_at(accessor, conn.clone(), s(), 99).await?);

            let last = SharedCtx::get_last_by_subject(accessor, conn.clone(), s(), a.clone())
                .await?
                .unwrap();
            assert_eq!(
                (last.message.sequence, last.message.data),
                (4, b"a2".to_vec())
            );
            let denial = expect_denied(
                SharedCtx::get_last_by_subject(accessor, conn.clone(), s(), hidden_subject.clone())
                    .await?,
            );
            assert!(matches!(denial.target, types::DeniedResource::Subject));
            expect_not_found(
                SharedCtx::get_last_by_subject(accessor, conn.clone(), s(), none.clone()).await?,
            );

            let subjects =
                SharedCtx::list_stream_subjects(accessor, conn.clone(), s(), String::new())
                    .await?
                    .unwrap()
                    .into_iter()
                    .map(|c| (c.subject, c.count))
                    .collect::<Vec<_>>();
            assert_eq!(subjects, [(a.clone(), 2), (b.clone(), 1)]);
            let only_b = SharedCtx::list_stream_subjects(accessor, conn.clone(), s(), b.clone())
                .await?
                .unwrap();
            assert_eq!(only_b.len(), 1);
            let denial = expect_denied(
                SharedCtx::list_stream_subjects(accessor, conn.clone(), s(), hidden_filter.clone())
                    .await?,
            );
            assert!(matches!(denial.target, types::DeniedResource::Subject));
            assert_eq!(denial.name, hidden_filter);

            let scanned = SharedCtx::scan(accessor, conn.clone(), s(), 0, 10)
                .await?
                .unwrap();
            assert_eq!(
                scanned.iter().map(|m| m.sequence).collect::<Vec<_>>(),
                [1, 3, 4]
            );
            let one = SharedCtx::scan(accessor, conn.clone(), s(), 2, 1)
                .await?
                .unwrap();
            assert_eq!(one.iter().map(|m| m.sequence).collect::<Vec<_>>(), [3]);
            let denial = expect_denied(SharedCtx::scan(accessor, conn.clone(), s(), 5, 10).await?);
            assert!(matches!(denial.target, types::DeniedResource::Subject));
            assert_eq!(denial.name, stream);
            Ok::<_, wasmtime::Error>(())
        }),
    )
    .await?;
    finished(ran)?;
    let consumers = fx.conn.jetstream.get_stream(&fx.scope.stream).await?;
    assert_eq!(consumers.cached_info().state.consumer_count, 0);
    fx.plugin.connections.shutdown().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn deletes_and_purges_are_held_to_both_grants() -> anyhow::Result<()> {
    let fx = fixture().await?;
    stream_with_hidden_subjects(&fx).await?;
    let a = fx.scope.subject("stream.a");
    let hidden_subject = format!("hidden.{}.x", fx.scope.stream);
    for body in [b"a1", b"a2", b"a3"] {
        publish_raw(&fx, a.clone(), body).await?;
    }
    assert_eq!(publish_raw(&fx, hidden_subject.clone(), b"h").await?, 4);

    let mut store = new_store("workload", Some(&fx.plugin))?;
    let conn = &fx.conn;
    let stream = fx.scope.stream.clone();
    let ran = tokio::time::timeout(
        Duration::from_secs(60),
        store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let accessor = &host_accessor;
            let s = || stream.clone();

            let denial =
                expect_denied(SharedCtx::delete_message(accessor, conn.clone(), s(), 4).await?);
            assert!(matches!(denial.target, types::DeniedResource::Message(4)));
            SharedCtx::delete_message(accessor, conn.clone(), s(), 1)
                .await?
                .unwrap();
            expect_not_found(SharedCtx::delete_message(accessor, conn.clone(), s(), 1).await?);

            let denial = expect_denied(
                SharedCtx::purge_subject(accessor, conn.clone(), s(), hidden_subject.clone(), None)
                    .await?,
            );
            assert!(matches!(denial.target, types::DeniedResource::Subject));
            assert_eq!(
                SharedCtx::purge_subject(accessor, conn.clone(), s(), a.clone(), Some(1))
                    .await?
                    .unwrap(),
                1
            );
            assert_eq!(
                SharedCtx::purge_subject(accessor, conn.clone(), s(), a.clone(), None)
                    .await?
                    .unwrap(),
                1
            );
            Ok::<_, wasmtime::Error>(())
        }),
    )
    .await?;
    finished(ran)?;
    let info = fx
        .conn
        .jetstream
        .get_stream(&fx.scope.stream)
        .await?
        .cached_info()
        .clone();
    assert_eq!(info.state.messages, 1);
    fx.plugin.connections.shutdown().await;
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn a_subject_map_too_large_for_one_call_is_refused_not_truncated() -> anyhow::Result<()> {
    let fx = fixture().await?;
    stream_with_hidden_subjects(&fx).await?;
    let mut acks = Vec::new();
    for i in 0..=MAX_STREAM_SUBJECTS {
        let subject = fx.scope.subject(&format!("stream.many.{i}"));
        acks.push(fx.conn.jetstream.publish(subject, "x".into()).await?);
    }
    for ack in acks {
        ack.await?;
    }

    let mut store = new_store("workload", Some(&fx.plugin))?;
    let conn = &fx.conn;
    let stream = fx.scope.stream.clone();
    let filter = fx.scope.subject("stream.many.>");
    let ran = tokio::time::timeout(
        Duration::from_secs(60),
        store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let accessor = &host_accessor;
            match SharedCtx::list_stream_subjects(
                accessor,
                conn.clone(),
                stream.clone(),
                filter.clone(),
            )
            .await?
            {
                Err(types::NatsError::LimitExceeded(msg)) => {
                    assert!(msg.contains("entry limit"), "{msg}")
                }
                unexpected => panic!("expected limit-exceeded, got {unexpected:?}"),
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
async fn consumer_lookups_are_held_to_the_subject_grant() -> anyhow::Result<()> {
    let fx = fixture().await?;
    let stream = stream_with_hidden_subjects(&fx).await?;
    let a = fx.scope.subject("stream.a");
    let b = fx.scope.subject("stream.b");
    stream
        .create_consumer(jetstream::consumer::pull::Config {
            durable_name: Some("narrow".into()),
            filter_subject: a.clone(),
            max_ack_pending: 7,
            ack_wait: Duration::from_secs(3),
            ..Default::default()
        })
        .await?;
    stream
        .create_consumer(jetstream::consumer::pull::Config {
            durable_name: Some("plural".into()),
            filter_subjects: vec![a.clone(), b.clone()],
            ..Default::default()
        })
        .await?;
    stream
        .create_consumer(jetstream::consumer::pull::Config {
            durable_name: Some("wide".into()),
            ..Default::default()
        })
        .await?;
    stream
        .create_consumer(jetstream::consumer::push::Config {
            durable_name: Some("pusher".into()),
            deliver_subject: fx.scope.subject("deliver"),
            filter_subject: a.clone(),
            ..Default::default()
        })
        .await?;

    let mut store = new_store("workload", Some(&fx.plugin))?;
    let conn = &fx.conn;
    let name = fx.scope.stream.clone();
    let ran = tokio::time::timeout(
        Duration::from_secs(60),
        store.run_concurrent(async |store_accessor| {
            let host_accessor = store_accessor.with_getter::<SharedCtx>(extract_active_ctx);
            let accessor = &host_accessor;
            let s = || name.clone();

            let narrow = SharedCtx::get_consumer_info(accessor, conn.clone(), s(), "narrow".into())
                .await?
                .unwrap();
            assert_eq!(narrow.name, "narrow");
            assert_eq!(narrow.stream_name, name);
            assert_eq!(narrow.filter_subject, a);
            assert!(narrow.filter_subjects.is_empty());
            assert_eq!(narrow.max_ack_pending, 7);
            assert_eq!(narrow.ack_wait_ms, 3_000);
            assert_eq!(narrow.num_pending, 0);

            let plural = SharedCtx::get_consumer_info(accessor, conn.clone(), s(), "plural".into())
                .await?
                .unwrap();
            assert!(plural.filter_subject.is_empty());
            assert_eq!(plural.filter_subjects, [a.clone(), b.clone()]);
            SharedCtx::open_pull_consumer(accessor, conn.clone(), s(), "plural".into())
                .await?
                .unwrap();

            for denial in [
                expect_denied(
                    SharedCtx::get_consumer_info(accessor, conn.clone(), s(), "wide".into())
                        .await?,
                ),
                expect_denied(
                    SharedCtx::open_pull_consumer(accessor, conn.clone(), s(), "wide".into())
                        .await?,
                ),
            ] {
                assert!(matches!(denial.target, types::DeniedResource::Subject));
                assert_eq!(denial.name, ">");
            }

            match SharedCtx::open_pull_consumer(accessor, conn.clone(), s(), "pusher".into())
                .await?
            {
                Err(types::NatsError::Jetstream(msg)) => {
                    assert!(msg.contains("is not a pull consumer"), "{msg}")
                }
                unexpected => panic!("expected jetstream(..), got {unexpected:?}"),
            }
            expect_not_found(
                SharedCtx::get_consumer_info(accessor, conn.clone(), s(), "missing".into()).await?,
            );
            expect_not_found(
                SharedCtx::open_pull_consumer(accessor, conn.clone(), s(), "missing".into())
                    .await?,
            );
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
async fn an_armed_guard_deletes_its_consumer_and_a_defused_one_does_not() -> anyhow::Result<()> {
    let fx = fixture().await?;
    let stream = stream_with_hidden_subjects(&fx).await?;
    let ephemeral = || {
        stream.create_consumer(jetstream::consumer::pull::Config {
            inactive_threshold: Duration::from_secs(30),
            ..Default::default()
        })
    };
    let kept = ephemeral().await?.cached_info().name.clone();
    let deleted = ephemeral().await?.cached_info().name.clone();

    ConsumerGuard::arm(&stream, &kept).defuse();
    drop(ConsumerGuard::arm(&stream, &deleted));
    tokio::time::timeout(Duration::from_secs(10), async {
        while stream.consumer_info(&deleted).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    stream.consumer_info(&kept).await?;
    fx.plugin.connections.shutdown().await;
    Ok(())
}
