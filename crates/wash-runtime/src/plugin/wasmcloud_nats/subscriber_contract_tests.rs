use super::*;
use std::time::Duration;

use testcontainers::{
    ContainerAsync, GenericImage,
    core::{IntoContainerPort as _, WaitFor},
    runners::AsyncRunner as _,
};
use tokio::time::timeout;

async fn nats() -> anyhow::Result<(async_nats::Client, Option<ContainerAsync<GenericImage>>)> {
    let (url, server) = if let Ok(url) = std::env::var("WASH_TEST_NATS_URL") {
        (url, None)
    } else {
        let server = GenericImage::new("nats", "2.12.8-alpine")
            .with_exposed_port(4222.tcp())
            .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
            .start()
            .await?;
        (
            format!(
                "nats://127.0.0.1:{}",
                server.get_host_port_ipv4(4222).await?
            ),
            Some(server),
        )
    };
    let client = timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(client) = async_nats::ConnectOptions::new()
                .request_timeout(Some(Duration::from_millis(100)))
                .connect(&url)
                .await
            {
                break client;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    Ok((client, server))
}

#[test]
fn deliveries_preserve_repeated_headers_and_kv_operation_metadata() {
    let mut headers = async_nats::HeaderMap::new();
    headers.append("X-Delivery", "first");
    headers.append("X-Delivery", "second");
    let mut converted = nats_headers_to_core_wit(&headers)
        .into_iter()
        .map(|h| (h.name.to_ascii_lowercase(), h.value))
        .collect::<Vec<_>>();
    converted.sort();
    assert_eq!(
        converted,
        [
            ("x-delivery".into(), "first".into()),
            ("x-delivery".into(), "second".into())
        ]
    );
    assert!(nats_headers_to_core_wit(&async_nats::HeaderMap::new()).is_empty());
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
        let entry = jetstream::kv::Entry {
            bucket: "bucket".into(),
            key: "key".into(),
            value: b"value".as_slice().into(),
            revision: 42,
            delta: 0,
            created: (std::time::UNIX_EPOCH - Duration::from_secs(1)).into(),
            operation,
            seen_current: true,
        };
        let converted = kv_entry_to_kv_handler_wit(&entry);
        assert_eq!(converted.key, "key");
        assert_eq!(converted.value, b"value");
        assert_eq!(converted.revision, 42);
        assert_eq!(converted.created_at_unix_nanos, 0);
        assert_eq!(converted.operation, expected);
    }
}

#[tokio::test]
#[ignore = "requires NATS (WASH_TEST_NATS_URL or Docker)"]
async fn core_readers_bound_backlogs_count_shedding_and_release_budgets() -> anyhow::Result<()> {
    let (client, _server) = nats().await?;
    for (bytes, host_limit) in [(usize::MAX, u64::MAX), (1, u64::MAX), (usize::MAX, 1)] {
        let subject = format!("contract.reader.{}", uuid::Uuid::new_v4().simple());
        let subscription = client.subscribe(subject.clone()).await?;
        client.flush().await?;
        let host = Arc::new(HostBacklogBudget::new(host_limit));
        let backlog = Arc::new(CoreBacklog::new(1, bytes, Arc::clone(&host)));
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
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
        assert!(backlog.shed_bytes.load(Ordering::Relaxed) > 0);
        assert!((1..=2).contains(&backlog.reported.load(Ordering::Relaxed)));
        let first = timeout(Duration::from_secs(5), receiver.recv())
            .await?
            .unwrap();
        assert_eq!(first.payload.as_ref(), b"first");
        assert_eq!(host.queued(), first.length as u64);
        backlog.dequeue(first.length);
        assert_eq!(host.queued(), 0);
        client.publish(subject, "after-drain".into()).await?;
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
        drop(backlog);
        assert_eq!(host.queued(), 0);
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires NATS (WASH_TEST_NATS_URL or Docker)"]
async fn core_reader_cancellation_and_closed_receivers_remove_subscriptions() -> anyhow::Result<()>
{
    let (client, _server) = nats().await?;
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
        if closed_receiver {
            receiver.close();
            client.publish(subject.clone(), "closed".into()).await?;
        } else {
            cancel.cancel();
        }
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
