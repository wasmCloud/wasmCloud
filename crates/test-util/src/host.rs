//! Utilities for managing wasmCloud hosts locally or remotely via the lattice

use std::pin::Pin;
use std::time::Duration;
use std::{future::Future, sync::Arc};

use anyhow::{anyhow, Context as _, Result};
use async_nats::{Client as NatsClient, ServerAddr};
use nkeys::KeyPair;
use url::Url;

use wasmcloud_control_interface::{Client as WasmcloudCtlClient, ClientBuilder};
use wasmcloud_host::wasmbus::{Host, HostConfig};

/// Add a host label, and ensure that it has been added
pub async fn assert_put_label(
    client: impl AsRef<WasmcloudCtlClient>,
    host_id: impl AsRef<str>,
    key: impl AsRef<str>,
    value: impl AsRef<str>,
) -> Result<()> {
    let client = client.as_ref();
    let host_id = host_id.as_ref();
    let key = key.as_ref();
    let value = value.as_ref();
    client
        .put_label(host_id, key, value)
        .await
        .map(|_| ())
        .map_err(|e| anyhow!(e).context("failed to put label"))
}

/// Remove a host label, ensuring that it has been deleted
pub async fn assert_delete_label(
    client: impl AsRef<WasmcloudCtlClient>,
    host_id: impl AsRef<str>,
    key: impl AsRef<str>,
) -> Result<()> {
    let client = client.as_ref();
    let host_id = host_id.as_ref();
    let key = key.as_ref();
    client
        .delete_label(host_id, key)
        .await
        .map(|_| ())
        .map_err(|e| anyhow!(e).context("failed to put label"))
}

/// wasmCloud host used in testing
#[allow(unused)]
pub struct WasmCloudTestHost {
    cluster_key: Arc<KeyPair>,
    host_key: Arc<KeyPair>,
    nats_url: ServerAddr,
    lattice_name: String,
    host: Arc<Host>,
    shutdown_hook: Pin<Box<dyn Future<Output = Result<()>>>>,
}

#[allow(unused)]
impl WasmCloudTestHost {
    /// Start a test wasmCloud [`Host`]
    pub async fn start(
        nats_url: impl Into<&Url>,
        lattice_name: impl AsRef<str>,
        cluster_key: Option<KeyPair>,
        host_key: Option<KeyPair>,
    ) -> Result<Self> {
        let nats_url = nats_url.into();
        let lattice_name = lattice_name.as_ref();
        let cluster_key = Arc::new(cluster_key.unwrap_or(KeyPair::new_cluster()));
        let host_key = Arc::new(host_key.unwrap_or(KeyPair::new_server()));

        let (host, shutdown_hook) = Host::new(HostConfig {
            ctl_nats_url: nats_url.clone(),
            rpc_nats_url: nats_url.clone(),
            lattice: lattice_name.into(),
            cluster_key: Some(Arc::clone(&cluster_key)),
            cluster_issuers: Some(vec![cluster_key.public_key()]),
            host_key: Some(Arc::clone(&host_key)),
            provider_shutdown_delay: Some(Duration::from_millis(300)),
            allow_file_load: true,
            ..Default::default()
        })
        .await
        .context("failed to initialize host")?;

        Ok(Self {
            cluster_key,
            host_key,
            nats_url: ServerAddr::from_url(nats_url.clone())
                .context("failed to build NATS server address from URL")?,
            lattice_name: lattice_name.into(),
            host,
            shutdown_hook: Box::pin(shutdown_hook),
        })
    }

    /// Stop this test host
    pub async fn stop(self) -> Result<()> {
        self.shutdown_hook
            .await
            .context("failed to perform shutdown hook")
    }

    /// Get a usable NATS client for the host
    pub async fn get_ctl_client(
        &self,
        nats_client: Option<NatsClient>,
    ) -> Result<WasmcloudCtlClient> {
        let nats_client = match nats_client {
            Some(c) => c,
            None => async_nats::connect(self.nats_url.clone())
                .await
                .context("failed to connect to NATS client via URL used at test host creation")?,
        };
        Ok(ClientBuilder::new(nats_client.clone())
            .lattice(self.lattice_name.to_string())
            .build())
    }

    /// Get the host key
    pub fn host_key(&self) -> Arc<KeyPair> {
        self.host_key.clone()
    }
}
