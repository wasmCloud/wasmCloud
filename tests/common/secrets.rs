use anyhow::{Context, Result};
use async_nats::Client;
use std::{collections::HashSet, process::Output, time::Duration};

use wasmcloud_secrets_types::Secret;

use super::BackgroundServer;

#[derive(Debug)]
pub struct NatsKvSecretsBackend {
    encryption_xkey: nkeys::XKey,
    transit_xkey: nkeys::XKey,
    subject_base: String,
    secrets_bucket: String,
    nats_address: String,
    nats_client: Client,
}

impl NatsKvSecretsBackend {
    pub async fn new(
        subject_base: String,
        secrets_bucket: String,
        nats_address: String,
    ) -> Result<Self> {
        Ok(Self {
            encryption_xkey: nkeys::XKey::new(),
            transit_xkey: nkeys::XKey::new(),
            subject_base,
            secrets_bucket,
            nats_client: async_nats::connect(nats_address.clone()).await?,
            nats_address,
        })
    }

    pub async fn ensure_build(&self) -> Result<Output> {
        std::env::set_current_dir("crates/secrets-nats-kv")?;
        let res = tokio::process::Command::new("cargo")
            .arg("build")
            .output()
            .await;
        std::env::set_current_dir("../../")?;
        res.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn put_secret(&self, secret: Secret) -> Result<()> {
        secrets_nats_kv::client::put_secret(
            &self.nats_client,
            &self.subject_base,
            &self.transit_xkey,
            secret,
        )
        .await?;

        Ok(())
    }

    pub async fn add_mapping(&self, public_key: &str, secrets: HashSet<String>) -> Result<()> {
        secrets_nats_kv::client::add_mapping(
            &self.nats_client,
            &self.subject_base,
            public_key,
            secrets,
        )
        .await?;

        Ok(())
    }

    pub async fn start(&self) -> Result<BackgroundServer> {
        let res = BackgroundServer::spawn(
            tokio::process::Command::new(
                std::env::var("WASMCLOUD_NATS_KV_SECRETS_BACKEND")
                    .as_deref()
                    .unwrap_or("./target/debug/secrets-nats-kv"),
            )
            .args([
                "run",
                "--encryption-xkey-seed",
                &self
                    .encryption_xkey
                    .seed()
                    .context("seed should be valid")?,
                "--transit-xkey-seed",
                &self.transit_xkey.seed().context("seed should be valid")?,
                "--subject-base",
                &self.subject_base,
                "--secrets-bucket",
                &self.secrets_bucket,
                "--nats-address",
                &self.nats_address,
            ]),
        )
        .await
        .context("failed to start NATS KV secrets backend");
        self.wait_for_started().await?;
        res
    }

    async fn wait_for_started(&self) -> Result<()> {
        for _ in 0..10 {
            let resp = self
                .nats_client
                .request(self.topic("server_xkey"), "".into())
                .await
                .map_err(|e| {
                    tracing::error!(?e);
                    anyhow::anyhow!("Request for server xkey failed")
                });

            if resp.map(|r| r.payload.len()).unwrap_or(0) > 0 {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
        anyhow::bail!("NATS KV secrets backend did not start, timed out waiting for server_xkey topic request")
    }

    fn topic(&self, operation: &str) -> String {
        format!("wasmcloud.secrets.v1alpha1.nats-kv.{}", operation)
    }
}
