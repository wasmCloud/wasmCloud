#![cfg(feature = "wasm_component_model_implements")]
//! End-to-end test for multiplexed `wasmcloud:messaging/consumer` routing via
//! `(implements ..)` across TWO independent NATS clusters. `team-a` resolves to
//! cluster A and `team-b` to cluster B. Both verification clients subscribe to
//! the *same* subject, so only the routing differs: a publish through `team-a`
//! lands on cluster A and never on cluster B (and vice versa), and a `request`
//! through `team-a` round-trips against a responder on cluster A. This proves
//! each named import is bound to its own backend, not merely its own subject.
//!
//! Requires Docker (NATS); marked `#[ignore]`, so it runs only under
//! `cargo test --include-ignored` (CI's Linux leg) and not a plain `cargo test`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt;
use testcontainers::{
    ContainerAsync, GenericImage,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};

use wash_runtime::plugin::wasmcloud_messaging::{
    BrokerMessage, MultiplexedMessaging, NatsMsgProvider,
};
use wash_runtime::wit::WitInterface;

fn msg_iface(name: &str, url: &str) -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "messaging".to_string(),
        interfaces: ["consumer".to_string(), "types".to_string()]
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

fn err(e: impl std::fmt::Debug) -> anyhow::Error {
    anyhow::anyhow!("messaging backend error: {e:?}")
}

/// Start a fresh single-node NATS cluster in a container, returning the guard
/// (kept alive for the test's duration) and its `nats://` url.
async fn start_nats() -> Result<(ContainerAsync<GenericImage>, String)> {
    let container = GenericImage::new("nats", "2.12.8-alpine")
        .with_exposed_port(4222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start NATS: {e}"))?;
    let port = container.get_host_port_ipv4(4222).await?;
    Ok((container, format!("nats://127.0.0.1:{port}")))
}

/// Force a server-side round-trip so every subscription queued earlier on
/// `client` is registered before we publish. `flush()` only flushes the local
/// TCP buffer; NATS processes commands per-connection in order, so once our
/// sentinel comes back, the earlier SUBs are active too.
async fn sync(client: &async_nats::Client) -> Result<()> {
    let inbox = client.new_inbox();
    let mut sentinel = client.subscribe(inbox.clone()).await?;
    client
        .publish(inbox, bytes::Bytes::from_static(&[0]))
        .await?;
    client.flush().await?;
    tokio::time::timeout(Duration::from_secs(5), sentinel.next())
        .await
        .context("sync round-trip timed out")?
        .context("sync inbox closed")?;
    Ok(())
}

/// Await the next message on `sub`, failing if none arrives in time.
async fn next_msg(sub: &mut async_nats::Subscriber) -> Result<async_nats::Message> {
    tokio::time::timeout(Duration::from_secs(5), sub.next())
        .await
        .context("timed out waiting for message")?
        .context("subscription closed")
}

/// Assert no message arrives on `sub` within a short window — the cross-cluster
/// isolation check (the other cluster must never see this import's publish).
async fn expect_silent(sub: &mut async_nats::Subscriber) -> Result<()> {
    match tokio::time::timeout(Duration::from_secs(1), sub.next()).await {
        Err(_) => Ok(()),   // timed out: silent, as required
        Ok(None) => Ok(()), // subscription closed: no message
        Ok(Some(m)) => Err(anyhow::anyhow!(
            "unexpected message on the isolated cluster (subject {})",
            m.subject
        )),
    }
}

#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn multiplexed_messaging_routes_each_import_to_its_cluster() -> Result<()> {
    // Two independent clusters; team-a -> A, team-b -> B.
    let (_nats_a, url_a) = start_nats().await?;
    let (_nats_b, url_b) = start_nats().await?;

    let plugin = MultiplexedMessaging::new().with_provider(Arc::new(NatsMsgProvider));
    let interfaces = HashSet::from([msg_iface("team-a", &url_a), msg_iface("team-b", &url_b)]);
    let registry = plugin.build_registry(&interfaces).await?;
    let team_a = registry.get("team-a").expect("team-a routed").clone();
    let team_b = registry.get("team-b").expect("team-b routed").clone();

    // A verification client on each cluster, both subscribed to the SAME subject
    // so only the routing (which cluster) distinguishes them.
    let verify_a = async_nats::connect(&url_a)
        .await
        .context("verify-a connect")?;
    let verify_b = async_nats::connect(&url_b)
        .await
        .context("verify-b connect")?;
    let subject = "team.events";
    let mut sub_a = verify_a.subscribe(subject).await?;
    let mut sub_b = verify_b.subscribe(subject).await?;
    sync(&verify_a).await?;
    sync(&verify_b).await?;

    // team-a's publish lands on cluster A only.
    team_a
        .publish(BrokerMessage {
            subject: subject.to_string(),
            reply_to: None,
            body: b"from-a".to_vec(),
        })
        .await
        .map_err(err)?;
    assert_eq!(next_msg(&mut sub_a).await?.payload.as_ref(), b"from-a");
    expect_silent(&mut sub_b)
        .await
        .context("cluster B must not see team-a's publish")?;

    // team-b's publish lands on cluster B only.
    team_b
        .publish(BrokerMessage {
            subject: subject.to_string(),
            reply_to: None,
            body: b"from-b".to_vec(),
        })
        .await
        .map_err(err)?;
    assert_eq!(next_msg(&mut sub_b).await?.payload.as_ref(), b"from-b");
    expect_silent(&mut sub_a)
        .await
        .context("cluster A must not see team-b's publish")?;

    // `request` routed through team-a reaches a responder on cluster A and
    // returns its reply.
    let responder = async_nats::connect(&url_a)
        .await
        .context("responder connect")?;
    let mut requests = responder.subscribe("team-a.rpc").await?;
    sync(&responder).await?;
    let responder_handle = tokio::spawn(async move {
        if let Some(msg) = requests.next().await
            && let Some(reply) = msg.reply
        {
            let _ = responder.publish(reply, b"pong".to_vec().into()).await;
            let _ = responder.flush().await;
        }
    });

    let reply = team_a
        .request("team-a.rpc".to_string(), b"ping".to_vec(), 5000)
        .await
        .map_err(err)?;
    assert_eq!(reply.body, b"pong".to_vec());
    responder_handle.abort();

    Ok(())
}
