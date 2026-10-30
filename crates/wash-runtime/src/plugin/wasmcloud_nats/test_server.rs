//! NATS for the ignored unit suites: `NATS_URL` when set, otherwise a
//! JetStream-enabled container.
use std::time::Duration;

use testcontainers::{
    ContainerAsync, GenericImage, ImageExt as _,
    core::{IntoContainerPort as _, WaitFor},
    runners::AsyncRunner as _,
};

pub(super) struct NatsServer {
    pub(super) url: String,
    _container: Option<ContainerAsync<GenericImage>>,
}

impl NatsServer {
    pub(super) async fn start() -> anyhow::Result<Self> {
        if let Ok(url) = std::env::var("NATS_URL") {
            return Ok(Self {
                url,
                _container: None,
            });
        }
        Self::container().await
    }

    /// Always a server of its own, for a test that needs two distinct servers.
    pub(super) async fn container() -> anyhow::Result<Self> {
        let container = GenericImage::new("nats", "2.12.8-alpine")
            .with_exposed_port(4222.tcp())
            .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
            .with_cmd(["-js"])
            .start()
            .await?;
        let url = format!(
            "nats://127.0.0.1:{}",
            container.get_host_port_ipv4(4222).await?
        );
        Ok(Self {
            url,
            _container: Some(container),
        })
    }

    /// Connects, retrying until the server accepts clients.
    pub(super) async fn client(&self) -> anyhow::Result<async_nats::Client> {
        let client = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let connected = async_nats::ConnectOptions::new()
                    .request_timeout(Some(Duration::from_millis(100)))
                    .connect(&self.url)
                    .await;
                match connected {
                    Ok(client) => break client,
                    Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
                }
            }
        })
        .await?;
        Ok(client)
    }
}
