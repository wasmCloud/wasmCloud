use anyhow::Context;
use futures::stream::select_all;
use futures::StreamExt as _;
use tokio::select;
use tokio::task::JoinSet;
use tokio::time::timeout;
use tracing::warn;
use url::Url;

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "test-components:testing/pingpong@0.1.0": generate,
        }
    });
}

#[derive(Clone)]
struct PingServer;

impl bindings::exports::test_components::testing::pingpong::Handler<Option<async_nats::HeaderMap>>
    for PingServer
{
    async fn ping(&self, _cx: Option<async_nats::HeaderMap>) -> anyhow::Result<String> {
        Ok("Pong from external!".to_string())
    }

    async fn ping_secret(&self, _cx: Option<async_nats::HeaderMap>) -> anyhow::Result<String> {
        Ok("Secret pong from external!".to_string())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let nats_url = match std::env::args().nth(1) {
        Some(url) => {
            let url = Url::parse(&url).context("Invalid NATS URL format")?;
            if url.scheme() != "nats" {
                anyhow::bail!("URL must start with nats://");
            }
            url.to_string()
        }
        None => "nats://localhost:4222".to_string(),
    };

    let nats = timeout(
        std::time::Duration::from_secs(5),
        async_nats::connect_with_options(
            &nats_url,
            async_nats::ConnectOptions::new()
                .retry_on_initial_connect()
                .max_reconnects(Some(3)),
        ),
    )
    .await
    .context("NATS connection timeout")??;

    let prefix = "default.mock-server";

    let wrpc =
        wrpc_transport_nats::Client::new(nats.clone(), prefix.to_string(), Some(prefix.into()))
            .await
            .context("failed to create wRPC client")?;

    let invocations = bindings::serve(&wrpc, PingServer)
        .await
        .context("failed to serve pingpong interface")?;

    let mut invocations = select_all(
        invocations
            .into_iter()
            .map(|(instance, name, invocations)| invocations.map(move |res| (instance, name, res))),
    );
    let mut tasks = JoinSet::new();

    loop {
        select! {
            Some((instance, name, res)) = invocations.next() => {
                match res {
                    Ok(fut) => {
                        tasks.spawn(async move {
                            if let Err(err) = fut.await {
                                warn!(?err, "Failed to handle invocation");
                            }
                        });
                    }
                    Err(err) => warn!(?err, instance, name, "Failed to accept invocation"),
                }
            }
            Some(res) = tasks.join_next() => {
                if let Err(err) = res {
                    warn!(?err, "Task join failed");
                }
            }
            _ = tokio::signal::ctrl_c() => {
                while tasks.join_next().await.is_some() {}
                return Ok(());
            }
        }
    }
}
