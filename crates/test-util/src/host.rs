//! Utilities for managing wasmCloud hosts locally or remotely via the lattice

use std::collections::{BTreeMap, HashMap};
use std::pin::Pin;
use std::time::Duration;
use std::{future::Future, sync::Arc};

use anyhow::{anyhow, Context as _, Result};
use async_nats::{Client as NatsClient, ServerAddr};
use nkeys::KeyPair;
use tokio::task::JoinSet;
use url::Url;

use wasmcloud_control_interface::{Client as WasmcloudCtlClient, ClientBuilder};
use wasmcloud_host::nats::connect_nats;
use wasmcloud_host::wasmbus::host_config::PolicyService;
use wasmcloud_host::wasmbus::{Features, Host, HostConfig};

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
    ctl_server_handle: JoinSet<anyhow::Result<()>>,
    lattice_name: String,
    host: Arc<Host>,
    shutdown_hook: Pin<Box<dyn Future<Output = Result<()>>>>,
}

#[allow(unused)]
impl WasmCloudTestHost {
    /// Start a test wasmCloud [`Host`] instance, with generated cluster & host keys.
    ///
    /// # Arguments
    ///
    /// * `nats_url` - URL of the NATS instance to which we should connect (ex. "nats://localhost:4222")
    /// * `lattice_name` - Name of the wasmCloud lattice to which we should connect (ex. "default")
    pub async fn start(nats_url: impl AsRef<str>, lattice_name: impl AsRef<str>) -> Result<Self> {
        Self::start_custom(nats_url, lattice_name, None, None, None, None, None).await
    }

    /// Start a test wasmCloud [`Host`], with customization for the host that is started
    ///
    /// # Arguments
    ///
    /// * `nats_url` - URL of the NATS instance to which we should connect (ex. "nats://localhost:4222")
    /// * `lattice_name` - Name of the wasmCloud lattice to which we should connect (ex. "default")
    /// * `cluster_key` - An optional `nkeys::KeyPair` to use for the lattice. If not specified, one is generated.
    /// * `host_key` - An optional `nkeys::KeyPair` to use for the host. If not specified, one is generated.
    /// * `policy_service_config` - Configuration for a [Policy Service](https://wasmcloud.com/docs/deployment/security/policy-service) to use with the host
    /// * `secrets_backend_topic` - Topic for the host to use for secrets requests
    pub async fn start_custom(
        nats_url: impl AsRef<str>,
        lattice_name: impl AsRef<str>,
        cluster_key: Option<KeyPair>,
        host_key: Option<KeyPair>,
        policy_service_config: Option<PolicyService>,
        secrets_topic_prefix: Option<String>,
        experimental_features: Option<Features>,
    ) -> Result<Self> {
        let nats_url = Url::try_from(nats_url.as_ref()).context("failed to parse NATS URL")?;
        let lattice_name = lattice_name.as_ref();
        let cluster_key = Arc::new(cluster_key.unwrap_or(KeyPair::new_cluster()));
        let host_key = Arc::new(host_key.unwrap_or(KeyPair::new_server()));
        let experimental_features = experimental_features.unwrap_or_else(|| {
            Features::new()
                .enable_builtin_http_server()
                .enable_builtin_messaging_nats()
                .enable_wasmcloud_messaging_v3()
        });

        let host_config = HostConfig {
            rpc_nats_url: nats_url.clone(),
            lattice: lattice_name.into(),
            host_key: Arc::clone(&host_key),
            provider_shutdown_delay: Some(Duration::from_millis(300)),
            allow_file_load: true,
            experimental_features,
            ..Default::default()
        };

        let nats_client = connect_nats(nats_url.as_str(), None, None, false, None, None)
            .await
            .context("failed to connect to NATS")?;

        let nats_builder = wasmcloud_host::nats::builder::NatsHostBuilder::new(
            nats_client.clone(),
            None,
            lattice_name.into(),
            None,
            None,
            BTreeMap::new(),
            false,
            false,
            false,
        )
        .await?
        .with_event_publisher(host_key.public_key());

        let nats_builder = if let Some(secrets_topic_prefix) = secrets_topic_prefix {
            nats_builder.with_secrets_manager(secrets_topic_prefix)?
        } else {
            nats_builder
        };

        let nats_builder = if let Some(psc) = policy_service_config {
            nats_builder
                .with_policy_manager(
                    host_key.clone(),
                    HashMap::new(),
                    psc.policy_topic,
                    psc.policy_timeout_ms,
                    psc.policy_changes_topic,
                )
                .await?
        } else {
            nats_builder
        };

        let (host_builder, ctl_server) = nats_builder.build(host_config).await?;
        let (host, shutdown_hook) = host_builder
            .build()
            .await
            .context("failed to initialize host")?;

        let ctl_server_handle = ctl_server.start(host.clone()).await?;

        Ok(Self {
            cluster_key,
            host_key,
            ctl_server_handle,
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

    /// Get a usable NATS client for the lattice control plane
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
    #[must_use]
    pub fn host_key(&self) -> Arc<KeyPair> {
        self.host_key.clone()
    }

    /// Get the cluster key
    #[must_use]
    pub fn cluster_key(&self) -> Arc<KeyPair> {
        self.cluster_key.clone()
    }

    /// Get the lattice name for the host
    #[must_use]
    pub fn lattice_name(&self) -> &str {
        self.lattice_name.as_ref()
    }

    /// Get the host ID
    #[must_use]
    pub fn host_id(&self) -> String {
        self.host_key().public_key()
    }
}
