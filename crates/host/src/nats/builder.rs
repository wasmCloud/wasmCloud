//! An opinionated [crate::wasmbus::HostBuilder] that uses NATS as the primary transport.

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::Duration,
};

use anyhow::{ensure, Context as _};
use async_nats::{jetstream::kv::Store, Client};
use nkeys::KeyPair;
use serde::Deserialize;
use serde_json::json;
use tracing::{debug, error, instrument};
use wasmcloud_control_interface::RegistryCredential;
use wasmcloud_core::RegistryConfig;

use crate::{
    event::EventPublisher,
    nats::{event::NatsEventPublisher, policy::NatsPolicyManager, secrets::NatsSecretsManager},
    oci,
    registry::{merge_registry_config, RegistryCredentialExt as _, SupplementalConfig},
    secrets::SecretsManager,
    store::StoreManager,
    wasmbus::{config::BundleGenerator, HostBuilder},
    PolicyHostInfo, PolicyManager, WasmbusHostConfig,
};

const DEFAULT_CTL_TOPIC_PREFIX: &str = "wasmbus.ctl";

use super::{create_bucket, ctl::NatsControlInterfaceServer};

/// Opinionated [crate::wasmbus::HostBuilder] that uses NATS as the primary transport and implementations
/// for the [crate::wasmbus::Host] extension traits.
///
/// This builder is used to create a [crate::wasmbus::HostBuilder] and a [NatsControlInterfaceServer] for
/// listening for control messages on the NATS message bus. Incoming messages will use the
/// [crate::wasmbus::ctl::ControlInterfaceServer] trait to handle the messages and
/// send them to the host.
pub struct NatsHostBuilder {
    // Required fields
    ctl_nats: Client,
    ctl_topic_prefix: String,
    lattice: String,
    config_generator: BundleGenerator,
    registry_config: HashMap<String, RegistryConfig>,
    enable_component_auction: bool,
    enable_provider_auction: bool,

    // Trait implementations for NATS
    config_store: Arc<dyn StoreManager>,
    data_store: Store,
    policy_manager: Option<Arc<dyn PolicyManager>>,
    secrets_manager: Option<Arc<dyn SecretsManager>>,
    event_publisher: Option<Arc<dyn EventPublisher>>,
}

impl NatsHostBuilder {
    /// Initialize the host with the NATS control interface connection
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        ctl_nats: Client,
        ctl_topic_prefix: Option<String>,
        lattice: String,
        js_domain: Option<String>,
        oci_opts: Option<oci::Config>,
        labels: BTreeMap<String, String>,
        config_service_enabled: bool,
        enable_component_auction: bool,
        enable_provider_auction: bool,
    ) -> anyhow::Result<Self> {
        let ctl_jetstream = if let Some(domain) = js_domain.as_ref() {
            async_nats::jetstream::with_domain(ctl_nats.clone(), domain)
        } else {
            async_nats::jetstream::new(ctl_nats.clone())
        };
        let bucket = format!("LATTICEDATA_{}", lattice);
        let data_store = create_bucket(&ctl_jetstream, &bucket).await?;

        let config_bucket = format!("CONFIGDATA_{}", lattice);
        let config_data = create_bucket(&ctl_jetstream, &config_bucket).await?;

        let supplemental_config = if config_service_enabled {
            load_supplemental_config(&ctl_nats, &lattice, &labels).await?
        } else {
            SupplementalConfig::default()
        };

        let mut registry_config = supplemental_config.registry_config.unwrap_or_default();
        if let Some(oci_opts) = oci_opts {
            debug!("supplementing OCI config with OCI options");
            merge_registry_config(&mut registry_config, oci_opts).await;
        }

        // TODO(brooksmtownsend): figure this out where go
        let config_generator = BundleGenerator::new(Arc::new(config_data.clone()));

        Ok(Self {
            ctl_nats,
            ctl_topic_prefix: ctl_topic_prefix
                .unwrap_or_else(|| DEFAULT_CTL_TOPIC_PREFIX.to_string()),
            lattice,
            config_generator,
            registry_config,
            config_store: Arc::new(config_data),
            data_store,
            policy_manager: None,
            secrets_manager: None,
            event_publisher: None,
            enable_component_auction,
            enable_provider_auction,
        })
    }

    /// Setup the NATS policy manager for the host
    pub async fn with_policy_manager(
        self,
        host_key: Arc<KeyPair>,
        labels: HashMap<String, String>,
        policy_topic: Option<String>,
        policy_timeout: Option<Duration>,
        policy_changes_topic: Option<String>,
    ) -> anyhow::Result<Self> {
        let policy_manager = NatsPolicyManager::new(
            self.ctl_nats.clone(),
            PolicyHostInfo {
                public_key: host_key.public_key(),
                lattice: self.lattice.clone(),
                labels,
            },
            policy_topic,
            policy_timeout,
            policy_changes_topic,
        )
        .await?;

        Ok(NatsHostBuilder {
            policy_manager: Some(Arc::new(policy_manager)),
            ..self
        })
    }

    /// Setup the NATS secrets manager for the host
    pub fn with_secrets_manager(self, secrets_topic_prefix: String) -> anyhow::Result<Self> {
        ensure!(
            !secrets_topic_prefix.is_empty(),
            "secrets topic prefix must be non-empty"
        );
        let secrets_manager = NatsSecretsManager::new(
            Arc::clone(&self.config_store),
            Some(&secrets_topic_prefix),
            &self.ctl_nats,
        );

        Ok(NatsHostBuilder {
            secrets_manager: Some(Arc::new(secrets_manager)),
            ..self
        })
    }

    /// Setup the NATS event publisher for the host
    ///
    /// This will create a new NATS event publisher with the provided source. It's strongly
    /// recommended to use the host's public key as the source, as this will allow tracing
    /// events back to the host that published them.
    pub fn with_event_publisher(self, source: String) -> Self {
        let event_publisher =
            NatsEventPublisher::new(source, self.lattice.clone(), self.ctl_nats.clone());

        NatsHostBuilder {
            event_publisher: Some(Arc::new(event_publisher)),
            ..self
        }
    }

    /// Build the [`HostBuilder`] with the NATS extension traits and the provided [`WasmbusHostConfig`].
    pub async fn build(
        self,
        config: WasmbusHostConfig,
    ) -> anyhow::Result<(HostBuilder, NatsControlInterfaceServer)> {
        Ok((
            HostBuilder::from(config)
                .with_registry_config(self.registry_config)
                .with_event_publisher(self.event_publisher)
                .with_policy_manager(self.policy_manager)
                .with_secrets_manager(self.secrets_manager)
                .with_bundle_generator(Some(self.config_generator))
                .with_config_store(Some(self.config_store))
                .with_data_store(Some(Arc::new(self.data_store.clone()))),
            NatsControlInterfaceServer::new(
                self.ctl_nats,
                self.data_store,
                self.ctl_topic_prefix,
                self.enable_component_auction,
                self.enable_provider_auction,
            ),
        ))
    }
}

#[instrument(level = "debug", skip_all)]
async fn load_supplemental_config(
    ctl_nats: &async_nats::Client,
    lattice: &str,
    labels: &BTreeMap<String, String>,
) -> anyhow::Result<SupplementalConfig> {
    #[derive(Deserialize, Default)]
    struct SerializedSupplementalConfig {
        #[serde(default, rename = "registryCredentials")]
        registry_credentials: Option<HashMap<String, RegistryCredential>>,
    }

    let cfg_topic = format!("wasmbus.cfg.{lattice}.req");
    let cfg_payload = serde_json::to_vec(&json!({
        "labels": labels,
    }))
    .context("failed to serialize config payload")?;

    debug!("requesting supplemental config");
    match ctl_nats.request(cfg_topic, cfg_payload.into()).await {
        Ok(resp) => {
            match serde_json::from_slice::<SerializedSupplementalConfig>(resp.payload.as_ref()) {
                Ok(ser_cfg) => Ok(SupplementalConfig {
                    registry_config: ser_cfg.registry_credentials.and_then(|creds| {
                        creds
                            .into_iter()
                            .map(|(k, v)| {
                                debug!(registry_url = %k, "set registry config");
                                v.into_registry_config().map(|v| (k, v))
                            })
                            .collect::<anyhow::Result<_>>()
                            .ok()
                    }),
                }),
                Err(e) => {
                    error!(
                        ?e,
                        "failed to deserialize supplemental config. Defaulting to empty config"
                    );
                    Ok(SupplementalConfig::default())
                }
            }
        }
        Err(e) => {
            error!(
                ?e,
                "failed to request supplemental config. Defaulting to empty config"
            );
            Ok(SupplementalConfig::default())
        }
    }
}
