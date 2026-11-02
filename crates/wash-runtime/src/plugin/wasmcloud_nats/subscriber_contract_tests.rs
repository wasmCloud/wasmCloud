use super::super::test_server::NatsServer;
use super::*;
use std::time::Duration;

use tokio::sync::mpsc::error::TryRecvError;
use tokio::time::timeout;

#[test]
fn repeated_header_values_each_become_an_entry_with_the_name_case_kept() {
    let mut headers = async_nats::HeaderMap::new();
    headers.append("X-Delivery", "first");
    headers.append("X-Delivery", "second");
    let converted = nats_headers_to_core_wit(&headers)
        .into_iter()
        .map(|h| (h.name, h.value))
        .collect::<Vec<_>>();
    assert_eq!(
        converted,
        [
            ("X-Delivery".to_string(), "first".to_string()),
            ("X-Delivery".to_string(), "second".to_string())
        ]
    );
}

#[test]
fn an_empty_header_map_converts_to_no_entries() {
    assert!(nats_headers_to_core_wit(&async_nats::HeaderMap::new()).is_empty());
}

fn kv_entry(
    operation: jetstream::kv::Operation,
    created: std::time::SystemTime,
) -> jetstream::kv::Entry {
    jetstream::kv::Entry {
        bucket: "bucket".into(),
        key: "key".into(),
        value: b"value".as_slice().into(),
        revision: 42,
        delta: 0,
        created: created.into(),
        operation,
        seen_current: true,
    }
}

#[test]
fn kv_operations_convert_one_to_one() {
    for (operation, expected) in [
        (
            jetstream::kv::Operation::Put,
            kv_bindings::wasmcloud::nats::kv::KvOperation::Put,
        ),
        (
            jetstream::kv::Operation::Delete,
            kv_bindings::wasmcloud::nats::kv::KvOperation::Delete,
        ),
        (
            jetstream::kv::Operation::Purge,
            kv_bindings::wasmcloud::nats::kv::KvOperation::Purge,
        ),
    ] {
        let entry = kv_entry(operation, std::time::UNIX_EPOCH);
        assert_eq!(kv_entry_to_kv_handler_wit(&entry).operation, expected);
    }
}

#[test]
fn kv_entries_carry_key_value_revision_and_timestamp() {
    let created = std::time::UNIX_EPOCH + Duration::from_secs(1) + Duration::from_millis(500);
    let converted = kv_entry_to_kv_handler_wit(&kv_entry(jetstream::kv::Operation::Put, created));
    assert_eq!(converted.key, "key");
    assert_eq!(converted.value, b"value");
    assert_eq!(converted.revision, 42);
    assert_eq!(converted.created_at_unix_nanos, 1_500_000_000);
}

#[test]
fn kv_timestamps_before_the_epoch_clamp_to_zero() {
    let created = std::time::UNIX_EPOCH - Duration::from_secs(1);
    let converted = kv_entry_to_kv_handler_wit(&kv_entry(jetstream::kv::Operation::Put, created));
    assert_eq!(converted.created_at_unix_nanos, 0);
}

#[test]
fn a_dropped_backlog_returns_its_stranded_bytes_to_the_host() {
    let host = Arc::new(HostBacklogBudget::new(4096));
    let backlog = CoreBacklog::new(1, usize::MAX, Arc::clone(&host));
    backlog.queue(100);
    backlog.queue(28);
    assert_eq!(host.queued(), 128);
    drop(backlog);
    assert_eq!(host.queued(), 0);

    let second = CoreBacklog::new(1, usize::MAX, Arc::clone(&host));
    second.queue(1);
    assert_eq!(refuses(&second, 4095), None);
}

#[tokio::test(start_paused = true)]
async fn in_flight_tracking_resumes_at_the_oldest_and_prunes_by_ttl() {
    let sequences: InFlight = Arc::default();
    track_sequence(&sequences, 5);
    track_sequence(&sequences, 3);
    track_sequence(&sequences, 9);
    assert_eq!(oldest_tracked(&sequences), Some(3));

    release_sequence(&sequences, 3);
    assert_eq!(oldest_tracked(&sequences), Some(5));

    tokio::time::advance(TRACKING_TTL + Duration::from_secs(1)).await;
    assert_eq!(oldest_tracked(&sequences), None);

    track_sequence(&sequences, 7);
    assert_eq!(oldest_tracked(&sequences), Some(7));
    let tracked = sequences
        .lock()
        .unwrap()
        .keys()
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(tracked, [7]);
}

#[test]
fn redelivery_backoff_ladders_to_its_cap() {
    for (delivery_count, secs) in [(0, 1), (1, 1), (2, 2), (16, 30), (31, 60), (u32::MAX, 60)] {
        assert_eq!(
            redelivery_backoff(delivery_count),
            Duration::from_secs(secs),
            "delivery {delivery_count}"
        );
    }
}

#[test]
fn resubscribe_backoff_doubles_to_its_cap() {
    for (failures, secs) in [(0, 2), (1, 4), (4, 32), (5, 60), (40, 60)] {
        assert_eq!(
            resubscribe_backoff(failures),
            Duration::from_secs(secs),
            "failure {failures}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn wait_or_cancel_returns_true_for_a_cancelled_token_without_waiting() {
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let before = tokio::time::Instant::now();
    assert!(wait_or_cancel(&cancelled, Duration::from_secs(3600)).await);
    assert_eq!(tokio::time::Instant::now(), before);
}

#[tokio::test(start_paused = true)]
async fn wait_or_cancel_returns_false_once_the_delay_elapses() -> anyhow::Result<()> {
    let live = CancellationToken::new();
    let delay = Duration::from_secs(30);
    let waiter = tokio::spawn({
        let live = live.clone();
        async move { wait_or_cancel(&live, delay).await }
    });
    tokio::task::yield_now().await;
    tokio::time::advance(delay).await;
    assert!(!waiter.await?);
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn core_readers_bound_backlogs_count_shedding_and_release_budgets() -> anyhow::Result<()> {
    let server = NatsServer::start().await?;
    let client = server.client().await?;
    // A channel with room makes `backlog full` impossible, so a shed in the two
    // budget cases can only come from `refuses`.
    for (bytes, host_limit, channel) in [
        (usize::MAX, u64::MAX, 1),
        (1, u64::MAX, 8),
        (usize::MAX, 1, 8),
    ] {
        let subject = format!("contract.reader.{}", uuid::Uuid::new_v4().simple());
        let subscription = client.subscribe(subject.clone()).await?;
        client.flush().await?;
        let host = Arc::new(HostBacklogBudget::new(host_limit));
        let backlog = Arc::new(CoreBacklog::new(1, bytes, Arc::clone(&host)));
        let (sender, mut receiver) = tokio::sync::mpsc::channel(channel);
        let cancel = CancellationToken::new();
        spawn_core_reader(
            subject.clone(),
            subscription,
            sender,
            Arc::clone(&backlog),
            cancel.clone(),
        );
        for payload in ["first", "second", "third"] {
            client.publish(subject.clone(), payload.into()).await?;
        }
        client.flush().await?;
        timeout(Duration::from_secs(10), async {
            while backlog.shed.load(Ordering::Relaxed) != 2 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        let shed_bytes = (subject.len() + "second".len()) + (subject.len() + "third".len());
        assert_eq!(
            backlog.shed_bytes.load(Ordering::Relaxed),
            shed_bytes as u64
        );
        assert_eq!(backlog.reported.load(Ordering::Relaxed), 1);

        let first = receiver.try_recv()?;
        assert_eq!(first.payload.as_ref(), b"first");
        assert_eq!(receiver.try_recv().err(), Some(TryRecvError::Empty));
        assert_eq!(first.length, subject.len() + "first".len());
        assert_eq!(backlog.queued_bytes.load(Ordering::Relaxed), first.length);
        assert_eq!(host.queued(), first.length as u64);
        backlog.dequeue(first.length);
        assert_eq!(backlog.queued_bytes.load(Ordering::Relaxed), 0);
        assert_eq!(host.queued(), 0);

        client
            .publish(subject.clone(), "after-drain".into())
            .await?;
        let next = timeout(Duration::from_secs(5), receiver.recv())
            .await?
            .unwrap();
        assert_eq!(next.payload.as_ref(), b"after-drain");
        cancel.cancel();
        assert!(
            timeout(Duration::from_secs(5), receiver.recv())
                .await?
                .is_none()
        );
        assert_eq!(host.queued(), next.length as u64);
        drop(backlog);
        assert_eq!(host.queued(), 0);
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (NATS) or NATS_URL"]
async fn core_reader_cancellation_and_closed_receivers_remove_subscriptions() -> anyhow::Result<()>
{
    let server = NatsServer::start().await?;
    let client = server.client().await?;
    for closed_receiver in [false, true] {
        let subject = format!("contract.reader.{}", uuid::Uuid::new_v4().simple());
        let subscription = client.subscribe(subject.clone()).await?;
        client.flush().await?;
        let host = Arc::new(HostBacklogBudget::unbounded());
        let backlog = Arc::new(CoreBacklog::new(1, usize::MAX, Arc::clone(&host)));
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        let cancel = CancellationToken::new();
        spawn_core_reader(
            subject.clone(),
            subscription,
            sender,
            backlog,
            cancel.clone(),
        );

        let queued_len = (subject.len() + "queued".len()) as u64;
        client.publish(subject.clone(), "queued".into()).await?;
        client.flush().await?;
        timeout(Duration::from_secs(10), async {
            while host.queued() != queued_len {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;

        if closed_receiver {
            receiver.close();
            client.publish(subject.clone(), "closed".into()).await?;
        } else {
            cancel.cancel();
        }
        let queued = timeout(Duration::from_secs(5), receiver.recv())
            .await?
            .unwrap();
        assert_eq!(queued.payload.as_ref(), b"queued");
        assert!(
            timeout(Duration::from_secs(5), receiver.recv())
                .await?
                .is_none()
        );
        timeout(Duration::from_secs(5), async {
            loop {
                client.flush().await?;
                let result = client.request(subject.clone(), "probe".into()).await;
                if result
                    .as_ref()
                    .is_err_and(|e| e.kind() == async_nats::RequestErrorKind::NoResponders)
                {
                    break Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await??;
        assert_eq!(host.queued(), 0);
    }
    Ok(())
}
