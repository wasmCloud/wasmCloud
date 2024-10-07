use crate::wasmbus::host_config::PolicyService;
use crate::wasmbus::{
    component_import_links, create_bucket, event, fetch_component, handler::Handler,
    injector_to_headers, load_supplemental_config, merge_registry_config, serialize_ctl_response,
    Annotations, Claims, Component, ComponentSpecification, Provider, Queue, StoredClaims,
    SupplementalConfig, WrpcServer, MAX_INVOCATION_CHANNEL_SIZE, MIN_INVOCATION_CHANNEL_SIZE,
};
use crate::WasmbusHostConfig;

use std::collections::hash_map::{self};
use url::Url;
use wasmcloud_core::logging::Level as LogLevel;

use std::collections::btree_map::Entry as BTreeMapEntry;
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::num::NonZeroUsize;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Context as _};
use async_nats::jetstream::kv::{Entry as KvEntry, Operation, Store};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::Bytes;
use futures::stream::{AbortHandle, Abortable};
use futures::{join, stream, StreamExt, TryFutureExt, TryStreamExt};
use nkeys::{KeyPair, XKey};
use secrecy::Secret;
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::sync::{broadcast, mpsc, watch, RwLock, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Instant, Interval};
use tokio::{process, select, spawn};
use tokio_stream::wrappers::IntervalStream;
use tracing::{debug, error, info, instrument, trace, warn, Instrument as _};
use uuid::Uuid;
use wascap::jwt;
use wasmcloud_control_interface::{
    ComponentAuctionAck, ComponentAuctionRequest, ComponentDescription, CtlResponse,
    DeleteInterfaceLinkDefinitionRequest, HostLabel, Link, ProviderAuctionAck,
    ProviderAuctionRequest, ProviderDescription, RegistryCredential, ScaleComponentCommand,
    StartProviderCommand, StopProviderCommand, UpdateComponentCommand,
};
use wasmcloud_core::{
    provider_config_update_subject, ComponentId, HealthCheckResponse, HostData, OtelConfig,
};
use wasmcloud_runtime::capability::secrets::store::SecretValue;
use wasmcloud_runtime::component::WrpcServeEvent;
use wasmcloud_runtime::Runtime;
use wasmcloud_secrets_types::SECRET_PREFIX;
use wasmcloud_tracing::context::TraceContextInjector;
use wasmcloud_tracing::KeyValue;

use crate::registry::RegistryCredentialExt;
use crate::{
    HostMetrics, OciConfig, PolicyHostInfo, PolicyManager, PolicyResponse, RegistryConfig,
    SecretsManager,
};

use crate::wasmbus::config::{BundleGenerator, ConfigBundle};

/// wasmCloud Host configuration
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug)]
pub struct LatticeConfig {
    /// The topic prefix to use for control interface subscriptions, defaults to `wasmbus.ctl`
    pub ctl_topic_prefix: String,
    /// NATS URL to connect to for component RPC
    pub rpc_nats_url: Url,
    /// Timeout period for all RPC calls
    pub rpc_timeout: Duration,
    /// Authentication JWT for RPC connection, must be specified with `rpc_seed`
    pub rpc_jwt: Option<String>,
    /// Authentication key pair for RPC connection, must be specified with `rpc_jwt`
    pub rpc_key: Option<Arc<KeyPair>>,
    /// The amount of time to wait for a provider to gracefully shut down before terminating it
    pub provider_shutdown_delay: Option<Duration>,
    /// Configuration for downloading artifacts from OCI registries
    pub oci_opts: OciConfig,
    /// Whether to allow loading component or provider components from the filesystem
    pub allow_file_load: bool,
    /// Whether or not structured logging is enabled
    pub enable_structured_logging: bool,
    /// Log level to pass to capability providers to use. Should be parsed from a [`tracing::Level`]
    pub log_level: LogLevel,
    /// Whether to enable loading supplemental configuration
    pub config_service_enabled: bool,
    /// configuration for OpenTelemetry tracing
    pub otel_config: OtelConfig,
    /// configuration for wasmCloud policy service
    pub policy_service_config: PolicyService,
    /// topic for wasmCloud secrets backend
    pub secrets_topic_prefix: Option<String>,
}

impl From<WasmbusHostConfig> for LatticeConfig {
    fn from(config: WasmbusHostConfig) -> Self {
        Self {
            ctl_topic_prefix: config.ctl_topic_prefix,
            rpc_nats_url: config.rpc_nats_url,
            rpc_timeout: config.rpc_timeout,
            rpc_jwt: config.rpc_jwt,
            rpc_key: config.rpc_key,
            provider_shutdown_delay: config.provider_shutdown_delay,
            oci_opts: config.oci_opts,
            allow_file_load: config.allow_file_load,
            enable_structured_logging: config.enable_structured_logging,
            log_level: config.log_level,
            config_service_enabled: config.config_service_enabled,
            otel_config: config.otel_config,
            secrets_topic_prefix: config.secrets_topic_prefix,
            policy_service_config: config.policy_service_config,
        }
    }
}

/// All data associated with a particular lattice
pub struct Lattice {
    name: Arc<str>,
    ctl_nats: async_nats::Client,
    rpc_nats: Arc<async_nats::Client>,
    config: LatticeConfig,
    host_key: String,
    host_token: jwt::Token<jwt::Host>,
    labels: Arc<RwLock<BTreeMap<String, String>>>,
    components: RwLock<HashMap<ComponentId, Arc<Component>>>,
    secrets_xkey: Arc<XKey>,
    data: Store,
    data_watch_abort: AbortHandle,
    config_data: Store,
    config_generator: BundleGenerator,
    policy_manager: Arc<PolicyManager>,
    secrets_manager: Arc<SecretsManager>,
    providers: RwLock<HashMap<String, Provider>>,
    registry_config: RwLock<HashMap<String, RegistryConfig>>,
    queue_abort: AbortHandle,
    links: RwLock<HashMap<String, Vec<Link>>>,
    component_claims: Arc<RwLock<HashMap<ComponentId, jwt::Claims<jwt::Component>>>>, // TODO: use a single map once Claims is an enum
    provider_claims: Arc<RwLock<HashMap<String, jwt::Claims<jwt::CapabilityProvider>>>>,
    metrics: Arc<HostMetrics>,
    pub(crate) stop_tx: watch::Sender<Option<Instant>>,
    stop_rx: watch::Receiver<Option<Instant>>,
    max_execution_time: Duration,
    runtime: Runtime,
    event_rx: mpsc::Sender<(String, String, serde_json::Value)>,
    ctl_topic_prefix: String,
    heartbeat_abort: AbortHandle,
}

impl Lattice {
    /// Instantiate a new Lattice which is capable of running workloads for a particular wasmCloud
    /// lattice.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn new(
        name: String,
        client: async_nats::Client,
        ctl_client: async_nats::Client,
        jetstream_client: async_nats::jetstream::Context,
        host_key: String,
        labels: Arc<RwLock<BTreeMap<String, String>>>,
        metrics: Arc<HostMetrics>,
        max_execution_time: Duration,
        runtime: Runtime,
        event_rx: mpsc::Sender<(String, String, serde_json::Value)>,
        config: LatticeConfig,
        heartbeat_interval: Interval,
        host_token: jwt::Token<jwt::Host>,
    ) -> anyhow::Result<Arc<Self>> {
        let (stop_tx, stop_rx) = watch::channel(None);

        let bucket = format!("LATTICEDATA_{}", name.clone());
        let data = create_bucket(&jetstream_client, &bucket).await?;

        let config_bucket = format!("CONFIGDATA_{}", name.clone());
        let config_data = create_bucket(&jetstream_client, &config_bucket).await?;

        let supplemental_config = if config.config_service_enabled {
            load_supplemental_config(&client, &name, &labels.read().await.clone()).await?
        } else {
            SupplementalConfig::default()
        };

        let registry_config = RwLock::new(supplemental_config.registry_config.unwrap_or_default());
        merge_registry_config(&registry_config, config.oci_opts.clone()).await;

        let policy_manager = PolicyManager::new(
            client.clone(),
            PolicyHostInfo {
                public_key: host_key.clone(),
                lattice: name.clone(),
                labels: HashMap::from_iter(labels.read().await.clone()),
            },
            config.policy_service_config.policy_topic.clone(),
            config.policy_service_config.policy_timeout_ms,
            config.policy_service_config.policy_changes_topic.clone(),
        )
        .await?;

        let secrets_manager = Arc::new(SecretsManager::new(
            &config_data,
            config.secrets_topic_prefix.as_ref(),
            &client,
        ));
        let config_generator = BundleGenerator::new(config_data.clone());

        let (data_watch_abort, data_watch_abort_reg) = AbortHandle::new_pair();
        let (queue_abort, queue_abort_reg) = AbortHandle::new_pair();
        let (heartbeat_abort, heartbeat_abort_reg) = AbortHandle::new_pair();
        let lattice = Self {
            name: Arc::from(name.clone()),
            labels,
            // TODO this is in theory redundant with some other config options
            ctl_topic_prefix: config.ctl_topic_prefix.clone(),
            config: config.clone(),
            host_key: host_key.clone(),
            ctl_nats: ctl_client.clone(),
            rpc_nats: Arc::new(client),
            components: RwLock::default(),
            secrets_xkey: Arc::new(XKey::new()),
            secrets_manager,
            policy_manager,
            registry_config,
            providers: RwLock::default(),
            config_generator,
            config_data,
            data: data.clone(),
            links: RwLock::default(),
            component_claims: Arc::default(),
            provider_claims: Arc::default(),
            queue_abort: queue_abort.clone(),
            data_watch_abort,
            metrics,
            stop_tx,
            stop_rx,
            max_execution_time,
            runtime,
            event_rx,
            host_token,
            heartbeat_abort,
        };

        let lattice = Arc::new(lattice);
        let name = name.clone();
        let _data_watch: JoinHandle<anyhow::Result<_>> = spawn({
            let data = data.clone();
            let lattice: Arc<Lattice> = Arc::clone(&lattice);
            let name = name.clone();
            async move {
                let data_watch = data
                    .watch_all()
                    .await
                    .context("failed to watch lattice data bucket")?;
                let mut data_watch = Abortable::new(data_watch, data_watch_abort_reg);
                data_watch
                    .by_ref()
                    .for_each({
                        let lattice: Arc<Lattice> = Arc::clone(&lattice);
                        move |entry| {
                            let lattice = Arc::clone(&lattice);
                            async move {
                                match entry {
                                    Err(error) => {
                                        error!("failed to watch lattice data bucket: {error}");
                                    }
                                    Ok(entry) => lattice.process_entry(entry, true).await,
                                }
                            }
                        }
                    })
                    .await;
                let deadline = { *lattice.stop_rx.borrow() };
                lattice.stop_tx.send_replace(deadline);
                if data_watch.is_aborted() {
                    info!(lattice = name, "data watch task gracefully stopped");
                } else {
                    error!(lattice = name, "data watch task unexpectedly stopped");
                }
                Ok(())
            }
        });

        let queue = Queue::new_lattice(
            &ctl_client.clone(),
            &config.ctl_topic_prefix.clone(),
            &name,
            &host_key,
        )
        .await
        .context("failed to initialize queue")?;

        let _queue = spawn({
            let lattice = Arc::clone(&lattice);
            async move {
                let mut queue = Abortable::new(queue, queue_abort_reg);
                queue
                    .by_ref()
                    .for_each_concurrent(None, {
                        let lattice = Arc::clone(&lattice);
                        move |msg| {
                            let lattice = Arc::clone(&lattice);
                            async move { lattice.handle_ctl_message(msg).await }
                        }
                    })
                    .await;
                let deadline = { *lattice.stop_rx.borrow() };
                lattice.stop_tx.send_replace(deadline);
                if queue.is_aborted() {
                    info!("control interface queue task gracefully stopped");
                } else {
                    error!("control interface queue task unexpectedly stopped");
                }
            }
        });

        // Shutdown when requested
        spawn({
            let mut stop = lattice.stop_rx.clone();
            let lattice = Arc::clone(&lattice);
            let name = lattice.name.clone();
            async move {
                if let Err(e) = stop.changed().await {
                    error!(lattice =% name.clone(), "failed to wait for stop: {e}");
                }
                if let Err(err) = lattice.shutdown().await {
                    error!(lattice =% name, "failed to shutdown lattice: {err}");
                };
                info!(lattice =% name, "lattice stopped");
            }
        });

        // TODO this feels like a host-level responsibility and not a lattice thing. The problem of
        // course is that we need to fix the control interface so that a host heartbeat contains
        // information from all lattices.
        let heartbeat = IntervalStream::new(heartbeat_interval);
        spawn({
            let lattice = Arc::clone(&lattice);
            async move {
                let mut heartbeat = Abortable::new(heartbeat, heartbeat_abort_reg);
                heartbeat
                    .by_ref()
                    .for_each({
                        let lattice = Arc::clone(&lattice);
                        move |_| {
                            let lattice = lattice.clone();
                            async move {
                                let heartbeat =
                                    match serde_json::to_value(lattice.inventory().await) {
                                        Ok(heartbeat) => heartbeat,
                                        Err(e) => {
                                            error!("failed to generate heartbeat: {e}");
                                            return;
                                        }
                                    };

                                if let Err(e) =
                                    lattice.publish_event("host_heartbeat", heartbeat).await
                                {
                                    error!("failed to publish heartbeat: {e}");
                                }
                            }
                        }
                    })
                    .await;
                let deadline = { *lattice.stop_rx.borrow() };
                lattice.stop_tx.send_replace(deadline);
                if heartbeat.is_aborted() {
                    info!(lattice = name, "heartbeat task gracefully stopped");
                } else {
                    error!(lattice = name, "heartbeat task unexpectedly stopped");
                }
            }
        });

        data.keys()
            .await
            .context("failed to read keys of lattice data bucket")?
            .map_err(|e| anyhow!(e).context("failed to read lattice data stream"))
            .try_filter_map(|key| async {
                data.entry(key)
                    .await
                    .context("failed to get entry in lattice data bucket")
            })
            .for_each(|entry| async {
                match entry {
                    Ok(entry) => lattice.process_entry(entry, false).await,
                    Err(err) => error!(%err, "failed to read entry from lattice data bucket"),
                }
            })
            .await;

        Ok(Arc::clone(&lattice))
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        self.data_watch_abort.abort();
        self.queue_abort.abort();
        self.heartbeat_abort.abort();
        self.policy_manager.policy_changes.abort();
        // TODO add this back in
        //let _ = try_join!(queue, data_watch).context("failed to await tasks")?;

        self.rpc_nats
            .flush()
            .await
            .context("failed to flush NATS connection")?;
        // NOTE: Epoch interrupt thread will only stop once there are no more references to the engine
        Ok(())
    }

    /// Waits for host to be stopped via lattice commands and returns the shutdown deadline on
    /// success
    ///
    /// # Errors
    ///
    /// Returns an error if internal stop channel is closed prematurely
    //#[instrument(level = "debug", skip_all)]
    //pub async fn stopped(&self) -> anyhow::Result<Option<Instant>> {
    //    self.stop_rx
    //        .clone()
    //        .changed()
    //        .await
    //        .context("failed to wait for stop")?;
    //    Ok(*self.stop_rx.borrow())
    //}

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn inventory(&self) -> (Vec<ComponentDescription>, Vec<ProviderDescription>) {
        trace!("generating host inventory");
        let components = self.components.read().await;
        let components: Vec<_> = stream::iter(components.iter())
            .filter_map(|(id, component)| async move {
                let mut description = ComponentDescription::builder()
                    .id(id.into())
                    .image_ref(component.image_reference.to_string())
                    .annotations(component.annotations.clone().into_iter().collect())
                    .max_instances(component.max_instances.get().try_into().unwrap_or(u32::MAX))
                    .revision(
                        component
                            .claims()
                            .and_then(|claims| claims.metadata.as_ref())
                            .and_then(|jwt::Component { rev, .. }| *rev)
                            .unwrap_or_default(),
                    );
                // Add name if present
                if let Some(name) = component
                    .claims()
                    .and_then(|claims| claims.metadata.as_ref())
                    .and_then(|metadata| metadata.name.as_ref())
                    .cloned()
                {
                    description = description.name(name);
                };

                Some(
                    description
                        .build()
                        .expect("failed to build component description: {e}"),
                )
            })
            .collect()
            .await;

        let providers: Vec<_> = self
            .providers
            .read()
            .await
            .iter()
            .map(
                |(
                    provider_id,
                    Provider {
                        annotations,
                        claims_token,
                        image_ref,
                        ..
                    },
                )| {
                    let mut provider_description = ProviderDescription::builder()
                        .id(provider_id)
                        .image_ref(image_ref);
                    if let Some(name) = claims_token
                        .as_ref()
                        .and_then(|claims| claims.claims.metadata.as_ref())
                        .and_then(|metadata| metadata.name.as_ref())
                    {
                        provider_description = provider_description.name(name);
                    }
                    provider_description
                        .annotations(
                            annotations
                                .clone()
                                .into_iter()
                                .collect::<BTreeMap<String, String>>(),
                        )
                        .revision(
                            claims_token
                                .as_ref()
                                .and_then(|claims| claims.claims.metadata.as_ref())
                                .and_then(|jwt::CapabilityProvider { rev, .. }| *rev)
                                .unwrap_or_default(),
                        )
                        .build()
                        .expect("failed to build provider description")
                },
            )
            .collect();

        (components, providers)
    }

    #[instrument(level = "debug", skip(self))]
    async fn publish_event(&self, name: &str, data: serde_json::Value) -> anyhow::Result<()> {
        self.event_rx
            .send(((*self.name).to_string(), name.to_string(), data))
            .await
            .context("failed to send event")
    }

    /// Instantiate a component
    #[allow(clippy::too_many_arguments)] // TODO: refactor into a config struct
    #[instrument(level = "debug", skip_all)]
    async fn instantiate_component(
        &self,
        annotations: &Annotations,
        image_reference: Arc<str>,
        id: Arc<str>,
        max_instances: NonZeroUsize,
        mut component: wasmcloud_runtime::Component<Handler>,
        handler: Handler,
    ) -> anyhow::Result<Arc<Component>> {
        trace!(
            component_ref = ?image_reference,
            max_instances,
            "instantiating component"
        );

        let max_execution_time = self.max_execution_time;
        component.set_max_execution_time(max_execution_time);

        let (events_tx, mut events_rx) = mpsc::channel(
            max_instances
                .get()
                .clamp(MIN_INVOCATION_CHANNEL_SIZE, MAX_INVOCATION_CHANNEL_SIZE),
        );
        let prefix = Arc::from(format!("{}.{id}", &self.name));
        let exports = component
            .serve_wrpc(
                &WrpcServer {
                    nats: wrpc_transport_nats::Client::new(
                        Arc::clone(&self.rpc_nats),
                        Arc::clone(&prefix),
                        Some(prefix),
                    ),
                    claims: component.claims().cloned().map(Arc::new),
                    id: Arc::clone(&id),
                    image_reference: Arc::clone(&image_reference),
                    annotations: Arc::new(annotations.clone()),
                    policy_manager: Arc::clone(&self.policy_manager),
                    trace_ctx: Arc::clone(&handler.trace_ctx),
                    metrics: Arc::clone(&self.metrics),
                },
                handler.clone(),
                events_tx,
            )
            .await?;
        let permits = Arc::new(Semaphore::new(
            usize::from(max_instances).min(Semaphore::MAX_PERMITS),
        ));
        let metrics: Arc<HostMetrics> = Arc::clone(&self.metrics);
        let lattice = self.name.clone().to_string();
        Ok(Arc::new(Component {
            component,
            id,
            handler,
            exports: spawn(
                async move {
                    join!(
                        async move {
                            let mut tasks = JoinSet::new();
                            let mut exports = stream::select_all(exports);
                            loop {
                                let permits = Arc::clone(&permits);
                                select! {
                                    Some(fut) = exports.next() => {
                                        match fut {
                                            Ok(fut) => {
                                                debug!("accepted invocation, acquiring permit");
                                                let permit = permits.acquire_owned().await;
                                                tasks.spawn(async move {
                                                    let _permit = permit;
                                                    debug!("handling invocation");
                                                    match fut.await {
                                                        Ok(()) => {
                                                            debug!("successfully handled invocation");
                                                            Ok(())
                                                        },
                                                        Err(err) => {
                                                            warn!(?err, "failed to handle invocation");
                                                            Err(err)
                                                        },
                                                    }
                                                });
                                            }
                                            Err(err) => {
                                                warn!(?err, "failed to accept invocation")
                                            }
                                        }
                                    }
                                    Some(res) = tasks.join_next() => {
                                        if let Err(err) = res {
                                            error!(?err, "export serving task failed");
                                        }
                                    }
                                }
                            }
                        },
                        async move {
                            while let Some(evt) = events_rx.recv().await {
                                match evt {
                                    WrpcServeEvent::HttpIncomingHandlerHandleReturned {
                                        context: (start_at, ref attributes),
                                        success,
                                    }
                                    | WrpcServeEvent::MessagingHandlerHandleMessageReturned {
                                        context: (start_at, ref attributes),
                                        success,
                                    }
                                    | WrpcServeEvent::DynamicExportReturned {
                                        context: (start_at, ref attributes),
                                        success,
                                    } => {
                                        let mut attributes = attributes.clone();
                                        attributes.push(KeyValue::new("lattice".to_string(), lattice.clone()));
                                        metrics.record_component_invocation(
                                        u64::try_from(start_at.elapsed().as_nanos())
                                            .unwrap_or_default(),
                                        &attributes,
                                        !success,
                                    )
                                    },
                                }
                            }
                            debug!("serving event stream is done");
                        },
                    );
                    debug!("export serving task done");
                }
                .in_current_span(),
            ),
            annotations: annotations.clone(),
            max_instances,
            image_reference,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    #[instrument(level = "debug", skip_all)]
    async fn start_component<'a>(
        &self,
        entry: hash_map::VacantEntry<'a, String, Arc<Component>>,
        wasm: Vec<u8>,
        claims: Option<jwt::Claims<jwt::Component>>,
        component_ref: Arc<str>,
        component_id: Arc<str>,
        max_instances: NonZeroUsize,
        annotations: &Annotations,
        config: ConfigBundle,
        secrets: HashMap<String, Secret<SecretValue>>,
    ) -> anyhow::Result<&'a mut Arc<Component>> {
        debug!(?component_ref, ?max_instances, "starting new component");

        if let Some(ref claims) = claims {
            self.store_claims(Claims::Component(claims.clone()))
                .await
                .context("failed to store claims")?;
        }

        let component_spec = self
            .get_component_spec(&component_id)
            .await?
            .unwrap_or_else(|| ComponentSpecification::new(&component_ref));
        self.store_component_spec(&component_id, &component_spec)
            .await?;

        // Map the imports to pull out the result types of the functions for lookup when invoking them
        let handler = Handler {
            nats: Arc::clone(&self.rpc_nats),
            config_data: Arc::new(RwLock::new(config)),
            lattice: Arc::clone(&self.name),
            component_id: Arc::clone(&component_id),
            secrets: Arc::new(RwLock::new(secrets)),
            targets: Arc::default(),
            trace_ctx: Arc::default(),
            instance_links: Arc::new(RwLock::new(component_import_links(&component_spec.links))),
            invocation_timeout: Duration::from_secs(10), // TODO: Make this configurable
        };
        let component = wasmcloud_runtime::Component::new(&self.runtime, &wasm)?;
        let component = self
            .instantiate_component(
                annotations,
                Arc::clone(&component_ref),
                Arc::clone(&component_id),
                max_instances,
                component,
                handler,
            )
            .await
            .context("failed to instantiate component")?;

        info!(?component_ref, "component started");
        self.publish_event(
            "component_scaled",
            event::component_scaled(
                claims.as_ref(),
                annotations,
                self.host_key.clone(),
                max_instances,
                &component_ref,
                &component_id,
            ),
        )
        .await?;

        Ok(entry.insert(component))
    }

    #[instrument(level = "debug", skip_all)]
    async fn stop_component(&self, component: &Component, _host_id: &str) -> anyhow::Result<()> {
        trace!(component_id = %component.id, "stopping component");

        component.exports.abort();

        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_auction_component(
        &self,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<Option<CtlResponse<ComponentAuctionAck>>> {
        let req = serde_json::from_slice::<ComponentAuctionRequest>(payload.as_ref())
            .context("failed to deserialize component auction command")?;
        let component_ref = req.component_ref();
        let component_id = req.component_id();
        let constraints = req.constraints();

        info!(
            component_ref,
            component_id,
            ?constraints,
            "handling auction for component"
        );

        let host_labels = self.labels.read().await;
        let constraints_satisfied = constraints
            .iter()
            .all(|(k, v)| host_labels.get(k).is_some_and(|hv| hv == v));
        let component_id_running = self.components.read().await.contains_key(component_id);

        // This host can run the component if all constraints are satisfied and the component is not already running
        if constraints_satisfied && !component_id_running {
            Ok(Some(CtlResponse::ok(
                ComponentAuctionAck::from_component_host_and_constraints(
                    component_ref,
                    component_id,
                    &self.host_key,
                    constraints.clone(),
                ),
            )))
        } else {
            Ok(None)
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_auction_provider(
        &self,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<Option<CtlResponse<ProviderAuctionAck>>> {
        let req = serde_json::from_slice::<ProviderAuctionRequest>(payload.as_ref())
            .context("failed to deserialize provider auction command")?;
        let provider_ref = req.provider_ref();
        let provider_id = req.provider_id();
        let constraints = req.constraints();

        info!(
            provider_ref,
            provider_id,
            ?constraints,
            "handling auction for provider"
        );

        let host_labels = self.labels.read().await;
        let constraints_satisfied = constraints
            .iter()
            .all(|(k, v)| host_labels.get(k).is_some_and(|hv| hv == v));
        let providers = self.providers.read().await;
        let provider_running = providers.contains_key(provider_id);
        if constraints_satisfied && !provider_running {
            Ok(Some(CtlResponse::ok(
                ProviderAuctionAck::builder()
                    .provider_ref(provider_ref.into())
                    .provider_id(provider_id.into())
                    .constraints(constraints.clone())
                    .host_id(self.host_key.clone())
                    .build()
                    .map_err(|e| anyhow!("failed to build provider auction ack: {e}"))?,
            )))
        } else {
            Ok(None)
        }
    }

    #[instrument(level = "trace", skip_all)]
    async fn fetch_component(&self, component_ref: &str) -> anyhow::Result<Vec<u8>> {
        let registry_config = self.registry_config.read().await;
        fetch_component(
            component_ref,
            self.config.allow_file_load,
            &self.config.oci_opts.additional_ca_paths,
            &registry_config,
        )
        .await
        .context("failed to fetch component")
    }

    #[instrument(level = "trace", skip_all)]
    async fn store_component_claims(
        &self,
        claims: jwt::Claims<jwt::Component>,
    ) -> anyhow::Result<()> {
        let mut component_claims = self.component_claims.write().await;
        component_claims.insert(claims.subject.clone(), claims);
        Ok(())
    }

    //#[instrument(level = "debug", skip_all)]
    //async fn handle_stop_host(
    //    &self,
    //    payload: impl AsRef<[u8]>,
    //    transport_host_id: &str,
    //) -> anyhow::Result<CtlResponse<()>> {
    //    // Allow an empty payload to be used for stopping hosts
    //    let timeout = if payload.as_ref().is_empty() {
    //        None
    //    } else {
    //        let cmd = serde_json::from_slice::<StopHostCommand>(payload.as_ref())
    //            .context("failed to deserialize stop command")?;
    //        let timeout = cmd.timeout();
    //        let host_id = cmd.host_id();

    //        // If the Host ID was provided (i..e not the empty string, due to #[serde(default)]), then
    //        // we should check it against the known transport-provided host_id, and this actual host's ID
    //        if !host_id.is_empty() {
    //            anyhow::ensure!(
    //                host_id == transport_host_id && host_id == self.host_key,
    //                "invalid host_id [{host_id}]"
    //            );
    //        }
    //        timeout
    //    };

    //    // It *should* be impossible for the transport-derived host ID to not match at this point
    //    anyhow::ensure!(
    //        transport_host_id == self.host_key,
    //        "invalid host_id [{transport_host_id}]"
    //    );

    //    info!(?timeout, "handling stop host");

    //    self.heartbeat.abort();
    //    self.data_watch.abort();
    //    self.queue.abort();
    //    self.policy_manager.policy_changes.abort();
    //    let deadline =
    //        timeout.and_then(|timeout| Instant::now().checked_add(Duration::from_millis(timeout)));
    //    self.stop_tx.send_replace(deadline);
    //    Ok(CtlResponse::<()>::success(
    //        "successfully handled stop host".into(),
    //    ))
    //}

    #[instrument(level = "debug", skip_all)]
    async fn handle_scale_component(
        self: Arc<Self>,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        let cmd = serde_json::from_slice::<ScaleComponentCommand>(payload.as_ref())
            .context("failed to deserialize component scale command")?;
        let component_ref = cmd.component_ref();
        let component_id = cmd.component_id();
        let annotations = cmd.annotations();
        let max_instances = cmd.max_instances();
        let config = cmd.config().clone();
        let allow_update = cmd.allow_update();

        debug!(
            component_ref,
            max_instances, component_id, "handling scale component"
        );

        let host_id = host_id.to_string();
        let annotations: Annotations = annotations
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();

        // Basic validation to ensure that the component is running and that the image reference matches
        // If it doesn't match, we can still successfully scale, but we won't be updating the image reference
        let (original_ref, ref_changed) = {
            self.components
                .read()
                .await
                .get(component_id)
                .map(|v| {
                    (
                        Some(Arc::clone(&v.image_reference)),
                        &*v.image_reference != component_ref,
                    )
                })
                .unwrap_or_else(|| (None, false))
        };

        let mut perform_post_update: bool = false;
        let message = match (allow_update, original_ref, ref_changed) {
            // Updates are not allowed, original ref changed
            (false, Some(original_ref), true) => {
                let msg = format!(
                    "Requested to scale existing component to a different image reference: {original_ref} != {component_ref}. The component will be scaled but the image reference will not be updated. If you meant to update this component to a new image ref, use the update command."
                );
                warn!(msg);
                msg
            }
            // Updates are allowed, ref changed and we'll do an update later
            (true, Some(original_ref), true) => {
                perform_post_update = true;
                format!(
                    "Requested to scale existing component, with a changed image reference: {original_ref} != {component_ref}. The component will be scaled, and the image reference will be updated afterwards."
                )
            }
            _ => String::with_capacity(0),
        };

        let component_id = Arc::from(component_id);
        let component_ref = Arc::from(component_ref);
        // Spawn a task to perform the scaling and possibly an update of the component afterwards
        spawn(async move {
            // Fetch the component from the reference
            let component_and_claims =
                self.fetch_component(&component_ref)
                    .await
                    .map(|component_bytes| {
                        // Pull the claims token from the component, this returns an error only if claims are embedded
                        // and they are invalid (expired, tampered with, etc)
                        let claims_token =
                            wasmcloud_runtime::component::claims_token(&component_bytes);
                        (component_bytes, claims_token)
                    });
            let (wasm, claims_token) = match component_and_claims {
                Ok((wasm, Ok(claims_token))) => (wasm, claims_token),
                Err(e) | Ok((_, Err(e))) => {
                    if let Err(e) = self
                        .publish_event(
                            "component_scale_failed",
                            event::component_scale_failed(
                                None,
                                &annotations,
                                host_id,
                                &component_ref,
                                &component_id,
                                max_instances,
                                &e,
                            ),
                        )
                        .await
                    {
                        error!(%component_ref, %component_id, err = ?e, "failed to publish component scale failed event");
                    }
                    return;
                }
            };
            // Scale the component
            if let Err(e) = self
                .handle_scale_component_task(
                    Arc::clone(&component_ref),
                    Arc::clone(&component_id),
                    &host_id,
                    max_instances,
                    &annotations,
                    config,
                    wasm,
                    claims_token.as_ref(),
                )
                .await
            {
                error!(%component_ref, %component_id, err = ?e, "failed to scale component");
                if let Err(e) = self
                    .publish_event(
                        "component_scale_failed",
                        event::component_scale_failed(
                            claims_token.map(|c| c.claims).as_ref(),
                            &annotations,
                            host_id,
                            &component_ref,
                            &component_id,
                            max_instances,
                            &e,
                        ),
                    )
                    .await
                {
                    error!(%component_ref, %component_id, err = ?e, "failed to publish component scale failed event");
                }
                return;
            }

            if perform_post_update {
                if let Err(e) = self
                    .handle_update_component_task(
                        Arc::clone(&component_id),
                        Arc::clone(&component_ref),
                        &host_id,
                        None,
                    )
                    .await
                {
                    error!(%component_ref, %component_id, err = ?e, "failed to update component after scale");
                }
            }
        });

        Ok(CtlResponse::<()>::success(message))
    }

    #[instrument(level = "debug", skip_all)]
    /// Handles scaling an component to a supplied number of `max` concurrently executing instances.
    /// Supplying `0` will result in stopping that component instance.
    #[allow(clippy::too_many_arguments)]
    async fn handle_scale_component_task(
        &self,
        component_ref: Arc<str>,
        component_id: Arc<str>,
        host_id: &str,
        max_instances: u32,
        annotations: &Annotations,
        config: Vec<String>,
        wasm: Vec<u8>,
        claims_token: Option<&jwt::Token<jwt::Component>>,
    ) -> anyhow::Result<()> {
        trace!(?component_ref, max_instances, "scale component task");

        let claims = claims_token.map(|c| c.claims.clone());
        match self
            .policy_manager
            .evaluate_start_component(
                &component_id,
                &component_ref,
                max_instances,
                annotations,
                claims.as_ref(),
            )
            .await?
        {
            PolicyResponse {
                permitted: false,
                message: Some(message),
                ..
            } => bail!("Policy denied request to scale component `{component_id}`: `{message:?}`"),
            PolicyResponse {
                permitted: false, ..
            } => bail!("Policy denied request to scale component `{component_id}`"),
            PolicyResponse {
                permitted: true, ..
            } => (),
        };

        let scaled_event = match (
            self.components
                .write()
                .await
                .entry(component_id.to_string()),
            NonZeroUsize::new(max_instances as usize),
        ) {
            // No component is running and we requested to scale to zero, noop.
            // We still publish the event to indicate that the component has been scaled to zero
            (hash_map::Entry::Vacant(_), None) => event::component_scaled(
                claims.as_ref(),
                annotations,
                host_id,
                0_usize,
                &component_ref,
                &component_id,
            ),
            // No component is running and we requested to scale to some amount, start with specified max
            (hash_map::Entry::Vacant(entry), Some(max)) => {
                let (config, secrets) = self
                    .fetch_config_and_secrets(
                        &config,
                        claims_token.as_ref().map(|c| &c.jwt),
                        annotations.get("wasmcloud.dev/appspec"),
                    )
                    .await?;

                self.start_component(
                    entry,
                    wasm,
                    claims.clone(),
                    Arc::clone(&component_ref),
                    Arc::clone(&component_id),
                    max,
                    annotations,
                    config,
                    secrets,
                )
                .await?;

                event::component_scaled(
                    claims.as_ref(),
                    annotations,
                    host_id,
                    max,
                    &component_ref,
                    &component_id,
                )
            }
            // Component is running and we requested to scale to zero instances, stop component
            (hash_map::Entry::Occupied(entry), None) => {
                let component = entry.remove();
                self.stop_component(&component, host_id)
                    .await
                    .context("failed to stop component in response to scale to zero")?;

                info!(?component_ref, "component stopped");
                event::component_scaled(
                    claims.as_ref(),
                    &component.annotations,
                    host_id,
                    0_usize,
                    &component.image_reference,
                    &component.id,
                )
            }
            // Component is running and we requested to scale to some amount or unbounded, scale component
            (hash_map::Entry::Occupied(mut entry), Some(max)) => {
                let component = entry.get_mut();
                let config_changed =
                    &config != component.handler.config_data.read().await.config_names();

                // Create the event first to avoid borrowing the component
                // This event is idempotent.
                let event = event::component_scaled(
                    claims.as_ref(),
                    &component.annotations,
                    host_id,
                    max,
                    &component.image_reference,
                    &component.id,
                );

                // Modify scale only if the requested max differs from the current max or if the configuration has changed
                if component.max_instances != max || config_changed {
                    // We must partially clone the handler as we can't be sharing the targets between components
                    let handler = component.handler.copy_for_new();
                    if config_changed {
                        let (config, secrets) = self
                            .fetch_config_and_secrets(
                                &config,
                                claims_token.as_ref().map(|c| &c.jwt),
                                annotations.get("wasmcloud.dev/appspec"),
                            )
                            .await?;
                        *handler.config_data.write().await = config;
                        *handler.secrets.write().await = secrets;
                    }
                    let instance = self
                        .instantiate_component(
                            annotations,
                            Arc::clone(&component_ref),
                            Arc::clone(&component.id),
                            max,
                            component.component.clone(),
                            handler,
                        )
                        .await
                        .context("failed to instantiate component")?;
                    let component = entry.insert(instance);
                    self.stop_component(&component, host_id)
                        .await
                        .context("failed to stop component after scaling")?;

                    info!(?component_ref, ?max, "component scaled");
                } else {
                    debug!(?component_ref, ?max, "component already at desired scale");
                }
                event
            }
        };

        self.publish_event("component_scaled", scaled_event).await?;

        Ok(())
    }

    // TODO(#1548): With component IDs, new component references, configuration, etc, we're going to need to do some
    // design thinking around how update component should work. Should it be limited to a single host or latticewide?
    // Should it also update configuration, or is that separate? Should scaling be done via an update?
    #[instrument(level = "debug", skip_all)]
    async fn handle_update_component(
        self: Arc<Self>,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        let cmd = serde_json::from_slice::<UpdateComponentCommand>(payload.as_ref())
            .context("failed to deserialize component update command")?;
        let component_id = cmd.component_id();
        let annotations = cmd.annotations().cloned();
        let new_component_ref = cmd.new_component_ref();

        debug!(
            component_id,
            new_component_ref,
            ?annotations,
            "handling update component"
        );

        // Find the component and extract the image reference
        #[allow(clippy::map_clone)]
        // NOTE: clippy thinks, that we can just replace the `.map` below by
        // `.cloned` - we can't, because we need to clone the field
        let Some(component_ref) = self
            .components
            .read()
            .await
            .get(component_id)
            .map(|component| Arc::clone(&component.image_reference))
        else {
            return Ok(CtlResponse::error(&format!(
                "component {component_id} not found"
            )));
        };

        // If the component image reference is the same, respond with an appropriate message
        if &*component_ref == new_component_ref {
            return Ok(CtlResponse::<()>::success(format!(
                "component {component_id} already updated to {new_component_ref}"
            )));
        }

        let host_id = host_id.to_string();
        let message = format!(
            "component {component_id} updating from {component_ref} to {new_component_ref}"
        );
        let component_id = Arc::from(component_id);
        let new_component_ref = Arc::from(new_component_ref);
        spawn(async move {
            if let Err(e) = self
                .handle_update_component_task(
                    Arc::clone(&component_id),
                    Arc::clone(&new_component_ref),
                    &host_id,
                    annotations,
                )
                .await
            {
                error!(%new_component_ref, %component_id, err = ?e, "failed to update component");
            }
        });

        Ok(CtlResponse::<()>::success(message))
    }

    async fn handle_update_component_task(
        &self,
        component_id: Arc<str>,
        new_component_ref: Arc<str>,
        host_id: &str,
        annotations: Option<BTreeMap<String, String>>,
    ) -> anyhow::Result<()> {
        // NOTE: This block is specifically scoped to ensure we drop the read lock on `self.components` before
        // we attempt to grab a write lock.
        let component = {
            let components = self.components.read().await;
            let existing_component = components
                .get(&*component_id)
                .context("component not found")?;
            let annotations = annotations.unwrap_or_default().into_iter().collect();

            // task is a no-op if the component image reference is the same
            if existing_component.image_reference == new_component_ref {
                info!(%component_id, %new_component_ref, "component already updated");
                return Ok(());
            }

            let new_component = self.fetch_component(&new_component_ref).await?;
            let new_component = wasmcloud_runtime::Component::new(&self.runtime, &new_component)
                .context("failed to initialize component")?;
            let new_claims = new_component.claims().cloned();
            if let Some(ref claims) = new_claims {
                self.store_claims(Claims::Component(claims.clone()))
                    .await
                    .context("failed to store claims")?;
            }

            let max = existing_component.max_instances;
            let Ok(component) = self
                .instantiate_component(
                    &annotations,
                    Arc::clone(&new_component_ref),
                    Arc::clone(&component_id),
                    max,
                    new_component,
                    existing_component.handler.copy_for_new(),
                )
                .await
            else {
                bail!("failed to instantiate component from new reference");
            };

            info!(%new_component_ref, "component updated");
            self.publish_event(
                "component_scaled",
                event::component_scaled(
                    new_claims.as_ref(),
                    &component.annotations,
                    host_id,
                    max,
                    new_component_ref,
                    &component_id,
                ),
            )
            .await?;

            // TODO(#1548): If this errors, we need to rollback
            self.stop_component(&component, host_id)
                .await
                .context("failed to stop old component")?;
            self.publish_event(
                "component_scaled",
                event::component_scaled(
                    component.claims(),
                    &component.annotations,
                    host_id,
                    0_usize,
                    &component.image_reference,
                    &component.id,
                ),
            )
            .await?;

            component
        };

        self.components
            .write()
            .await
            .insert(component_id.to_string(), component);
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_start_provider(
        self: Arc<Self>,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        let cmd = serde_json::from_slice::<StartProviderCommand>(payload.as_ref())
            .context("failed to deserialize provider start command")?;

        if self.providers.read().await.contains_key(cmd.provider_id()) {
            return Ok(CtlResponse::error(
                "provider with that ID is already running",
            ));
        }

        info!(
            provider_ref = cmd.provider_ref(),
            provider_id = cmd.provider_id(),
            "handling start provider"
        ); // Log at info since starting providers can take a while

        let host_id = host_id.to_string();
        spawn(async move {
            let config = cmd.config();
            let provider_id = cmd.provider_id();
            let provider_ref = cmd.provider_ref();
            let annotations = cmd.annotations();
            if let Err(err) = self
                .handle_start_provider_task(
                    config,
                    provider_id,
                    provider_ref,
                    annotations.cloned().unwrap_or_default(),
                    &host_id,
                )
                .await
            {
                error!(provider_ref, provider_id, ?err, "failed to start provider");
                if let Err(err) = self
                    .publish_event(
                        "provider_start_failed",
                        event::provider_start_failed(provider_ref, provider_id, &err),
                    )
                    .await
                {
                    error!(?err, "failed to publish provider_start_failed event");
                }
            }
        });
        Ok(CtlResponse::<()>::success(
            "successfully started provider".into(),
        ))
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_start_provider_task(
        &self,
        config: &[String],
        provider_id: &str,
        provider_ref: &str,
        annotations: BTreeMap<String, String>,
        host_id: &str,
    ) -> anyhow::Result<()> {
        trace!(provider_ref, provider_id, "start provider task");

        let registry_config = self.registry_config.read().await;
        let (path, claims_token) = crate::fetch_provider(
            provider_ref,
            host_id,
            self.config.allow_file_load,
            &registry_config,
        )
        .await
        .context("failed to fetch provider")?;
        let claims = claims_token.as_ref().map(|t| t.claims.clone());

        if let Some(claims) = claims.clone() {
            self.store_claims(Claims::Provider(claims))
                .await
                .context("failed to store claims")?;
        }

        let annotations: Annotations = annotations.into_iter().collect();

        let PolicyResponse {
            permitted,
            request_id,
            message,
        } = self
            .policy_manager
            .evaluate_start_provider(provider_id, provider_ref, &annotations, claims.as_ref())
            .await?;
        ensure!(
            permitted,
            "policy denied request to start provider `{request_id}`: `{message:?}`",
        );

        let component_specification = self
            .get_component_spec(provider_id)
            .await?
            .unwrap_or_else(|| ComponentSpecification::new(provider_ref));

        self.store_component_spec(&provider_id, &component_specification)
            .await?;

        let (config, secrets) = self
            .fetch_config_and_secrets(
                config,
                claims_token.as_ref().map(|t| &t.jwt),
                annotations.get("wasmcloud.dev/appspec"),
            )
            .await?;

        let mut providers = self.providers.write().await;
        if let hash_map::Entry::Vacant(entry) = providers.entry(provider_id.into()) {
            let lattice_rpc_user_seed = self
                .config
                .rpc_key
                .as_ref()
                .map(|key| key.seed())
                .transpose()
                .context("private key missing for provider RPC key")?;
            let default_rpc_timeout_ms = Some(
                self.config
                    .rpc_timeout
                    .as_millis()
                    .try_into()
                    .context("failed to convert rpc_timeout to u64")?,
            );
            let otel_config = OtelConfig {
                enable_observability: self.config.otel_config.enable_observability,
                enable_traces: self.config.otel_config.enable_traces,
                enable_metrics: self.config.otel_config.enable_metrics,
                enable_logs: self.config.otel_config.enable_logs,
                observability_endpoint: self.config.otel_config.observability_endpoint.clone(),
                traces_endpoint: self.config.otel_config.traces_endpoint.clone(),
                metrics_endpoint: self.config.otel_config.metrics_endpoint.clone(),
                logs_endpoint: self.config.otel_config.logs_endpoint.clone(),
                protocol: self.config.otel_config.protocol,
                additional_ca_paths: self.config.otel_config.additional_ca_paths.clone(),
                trace_level: self.config.otel_config.trace_level.clone(),
            };

            let provider_xkey = XKey::new();
            // The provider itself needs to know its private key
            let provider_xkey_private_key = if let Ok(seed) = provider_xkey.seed() {
                seed
            } else if self.config.secrets_topic_prefix.is_none() {
                "".to_string()
            } else {
                // This should never happen since this returns an error when an Xkey is
                // created from a public key, but if we can't generate one for whatever
                // reason, we should bail.
                bail!("failed to generate seed for provider xkey")
            };
            // We only need to store the public key of the provider xkey, as the private key is only needed by the provider
            let xkey = XKey::from_public_key(&provider_xkey.public_key())
                .context("failed to create XKey from provider public key xkey")?;

            // Prepare startup links by generating the source and target configs. Note that because the provider may be the source
            // or target of a link, we need to iterate over all links to find the ones that involve the provider.
            let all_links = self.links.read().await;
            let provider_links = all_links
                .values()
                .flatten()
                .filter(|link| link.source_id() == provider_id || link.target() == provider_id);
            let link_definitions = stream::iter(provider_links)
                .filter_map(|link| async {
                    if link.source_id() == provider_id || link.target() == provider_id {
                        match self
                            .resolve_link_config(
                                link.clone(),
                                claims_token.as_ref().map(|t| &t.jwt),
                                annotations.get("wasmcloud.dev/appspec"),
                                &xkey,
                            )
                            .await
                        {
                            Ok(provider_link) => Some(provider_link),
                            Err(e) => {
                                error!(
                                    error = ?e,
                                    provider_id,
                                    source_id = link.source_id(),
                                    target = link.target(),
                                    "failed to resolve link config, skipping link"
                                );
                                None
                            }
                        }
                    } else {
                        None
                    }
                })
                .collect::<Vec<wasmcloud_core::InterfaceLinkDefinition>>()
                .await;

            let secrets = {
                // NOTE(brooksmtownsend): This trait import is used here to ensure we're only exposing secret
                // values when we need them.
                use secrecy::ExposeSecret;
                secrets
                    .iter()
                    .map(|(k, v)| match v.expose_secret() {
                        SecretValue::String(s) => (
                            k.clone(),
                            wasmcloud_core::secrets::SecretValue::String(s.to_owned()),
                        ),
                        SecretValue::Bytes(b) => (
                            k.clone(),
                            wasmcloud_core::secrets::SecretValue::Bytes(b.to_owned()),
                        ),
                    })
                    .collect()
            };

            let host_data = HostData {
                host_id: self.host_key.clone(),
                lattice_rpc_prefix: (*self.name).into(),
                link_name: "default".to_string(),
                lattice_rpc_user_jwt: self.config.rpc_jwt.clone().unwrap_or_default(),
                lattice_rpc_user_seed: lattice_rpc_user_seed.unwrap_or_default(),
                lattice_rpc_url: self.config.rpc_nats_url.to_string(),
                env_values: vec![],
                instance_id: Uuid::new_v4().to_string(),
                provider_key: provider_id.to_string(),
                link_definitions,
                config: config.get_config().await.clone(),
                secrets,
                provider_xkey_private_key,
                host_xkey_public_key: self.secrets_xkey.public_key(),
                cluster_issuers: vec![],
                default_rpc_timeout_ms,
                log_level: Some(self.config.log_level.clone()),
                structured_logging: self.config.enable_structured_logging,
                otel_config,
            };
            let host_data =
                serde_json::to_vec(&host_data).context("failed to serialize provider data")?;

            trace!("spawn provider process");

            let mut child_cmd = process::Command::new(&path);
            // Prevent the provider from inheriting the host's environment, with the exception of
            // the following variables we manually add back
            child_cmd.env_clear();

            if cfg!(windows) {
                // Proxy SYSTEMROOT to providers. Without this, providers on Windows won't be able to start
                child_cmd.env(
                    "SYSTEMROOT",
                    env::var("SYSTEMROOT")
                        .context("SYSTEMROOT is not set. Providers cannot be started")?,
                );
            }

            // Proxy RUST_LOG to (Rust) providers, so they can use the same module-level directives
            if let Ok(rust_log) = env::var("RUST_LOG") {
                let _ = child_cmd.env("RUST_LOG", rust_log);
            }

            let mut child = child_cmd
                .stdin(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .context("failed to spawn provider process")?;
            let mut stdin = child.stdin.take().context("failed to take stdin")?;
            stdin
                .write_all(STANDARD.encode(&host_data).as_bytes())
                .await
                .context("failed to write provider data")?;
            stdin
                .write_all(b"\r\n")
                .await
                .context("failed to write newline")?;
            stdin.shutdown().await.context("failed to close stdin")?;

            // Create a channel for watching for child process exit
            let (exit_tx, exit_rx) = broadcast::channel::<()>(1);
            spawn(async move {
                match child.wait().await {
                    Ok(status) => {
                        debug!("provider @ [{}] exited with `{status:?}`", path.display());
                    }
                    Err(e) => {
                        error!(
                            "failed to wait for provider @ [{}] to execute: {e}",
                            path.display()
                        );
                    }
                }
                if let Err(err) = exit_tx.send(()) {
                    warn!(%err, "failed to send exit tx");
                }
            });
            let mut exit_health_rx = exit_rx.resubscribe();

            // TODO: Change method receiver to Arc<Self> and `move` into the closure
            let rpc_nats = self.rpc_nats.clone();
            //let ctl_nats = self.ctl_nats.clone();
            // NOTE: health_lattice here is to allow us to move the variables into the closure
            let health_lattice = (*self.name).to_string();
            let health_host_id = host_id.to_string();
            let health_provider_id = provider_id.to_string();
            let event_rx = self.event_rx.clone();
            let lattice_name = (*self.name).to_string();
            let health_check_task = spawn(async move {
                // Check the health of the provider every 30 seconds
                let mut health_check = tokio::time::interval(Duration::from_secs(30));
                let mut previous_healthy = false;
                // Allow the provider 5 seconds to initialize
                health_check.reset_after(Duration::from_secs(5));
                let health_topic =
                    format!("wasmbus.rpc.{health_lattice}.{health_provider_id}.health");
                // TODO: Refactor this logic to simplify nesting
                loop {
                    select! {
                        _ = health_check.tick() => {
                            trace!(provider_id=health_provider_id, "performing provider health check");
                            let request = async_nats::Request::new()
                                .payload(Bytes::new())
                                .headers(injector_to_headers(&TraceContextInjector::default_with_span()));
                            if let Ok(async_nats::Message { payload, ..}) = rpc_nats.send_request(
                                health_topic.clone(),
                                request,
                                ).await {
                                    match (serde_json::from_slice::<HealthCheckResponse>(&payload), previous_healthy) {
                                        (Ok(HealthCheckResponse { healthy: true, ..}), false) => {
                                            trace!(provider_id=health_provider_id, "provider health check succeeded");
                                            previous_healthy = true;
                                            if let Err(e)= publish_event(
                                            event_rx.clone(),
                                                "health_check_passed".to_string(),
                                                lattice_name.clone(),
                                                event::provider_health_check(
                                                    &health_host_id,
                                                    &health_provider_id,
                                                )
                                            ).await {
                                                warn!(
                                                    ?e,
                                                    provider_id = health_provider_id,
                                                    "failed to publish provider health check succeeded event",
                                                );
                                            }
                                        },
                                        (Ok(HealthCheckResponse { healthy: false, ..}), true) => {
                                            trace!(provider_id=health_provider_id, "provider health check failed");
                                            previous_healthy = false;
                                            if let Err(e) = publish_event(
                                                event_rx.clone(),
                                                "health_check_failed".to_string(),
                                                lattice_name.clone(),
                                                event::provider_health_check(
                                                    &health_host_id,
                                                    &health_provider_id,
                                                )
                                            ).await {
                                                warn!(
                                                    ?e,
                                                    provider_id = health_provider_id,
                                                    "failed to publish provider health check failed event",
                                                );
                                            }
                                        }
                                        // If the provider health status didn't change, we simply publish a health check status event
                                        (Ok(_), _) => {
                                            if let Err(e) = publish_event(
                                        event_rx.clone(),
                                                "health_check_status".to_string(),
                                                lattice_name.clone(),
                                                event::provider_health_check(
                                                    &health_host_id,
                                                    &health_provider_id,
                                                )
                                            ).await {
                                                warn!(
                                                    ?e,
                                                    provider_id = health_provider_id,
                                                    "failed to publish provider health check status event",
                                                );
                                            }
                                        },
                                        _ => warn!(
                                            provider_id = health_provider_id,
                                            "failed to deserialize provider health check response"
                                        ),
                                    }
                                }
                                else {
                                    warn!(provider_id = health_provider_id, "failed to request provider health, retrying in 30 seconds");
                                }
                        }
                        exit = exit_health_rx.recv() => {
                            if let Err(err) = exit {
                                warn!(%err, provider_id = health_provider_id, "failed to receive exit in health check task");
                            }
                            break;
                        }
                    }
                }
            });
            info!(provider_ref, provider_id, "provider started");
            self.publish_event(
                "provider_started",
                event::provider_started(
                    claims.as_ref(),
                    &annotations,
                    host_id,
                    provider_ref,
                    provider_id,
                ),
            )
            .await?;

            // Spawn off a task to watch for config bundle updates and forward them to
            // the provider that we're spawning and managing
            let mut exit_config_tx = exit_rx;
            let provider_id = provider_id.to_string();
            let lattice = (*self.name).to_string();
            let client = self.rpc_nats.clone();
            let config = Arc::new(RwLock::new(config));
            let update_config = config.clone();
            let config_update_task = spawn(async move {
                let subject = provider_config_update_subject(&lattice, &provider_id);
                trace!(provider_id, "starting config update listener");
                loop {
                    let mut update_config = update_config.write().await;
                    select! {
                        maybe_update = update_config.changed() => {
                            let Ok(update) = maybe_update else {
                                break;
                            };
                            trace!(provider_id, "provider config bundle changed");
                            let bytes = match serde_json::to_vec(&*update) {
                                Ok(bytes) => bytes,
                                Err(err) => {
                                    error!(%err, provider_id, lattice, "failed to serialize configuration update ");
                                    continue;
                                }
                            };
                            trace!(provider_id, subject, "publishing config bundle bytes");
                            if let Err(err) = client.publish(subject.clone(), Bytes::from(bytes)).await {
                                error!(%err, provider_id, lattice, "failed to publish configuration update bytes to component");
                            }
                        }
                        exit = exit_config_tx.recv() => {
                            if let Err(err) = exit {
                                warn!(%err, provider_id, "failed to receive exit in config update task");
                            }
                            break;
                        }
                    }
                }
            });

            // Add the provider
            entry.insert(Provider {
                health_check_task,
                config_update_task,
                annotations,
                claims_token,
                image_ref: provider_ref.to_string(),
                xkey,
                config,
            });
        } else {
            bail!("provider is already running with that ID")
        }

        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_stop_provider(
        &self,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        let cmd = serde_json::from_slice::<StopProviderCommand>(payload.as_ref())
            .context("failed to deserialize provider stop command")?;
        let provider_id = cmd.provider_id();

        debug!(provider_id, "handling stop provider");

        let mut providers = self.providers.write().await;
        let hash_map::Entry::Occupied(entry) = providers.entry(provider_id.into()) else {
            warn!(
                provider_id,
                "received request to stop provider that is not running"
            );
            return Ok(CtlResponse::error("provider with that ID is not running"));
        };
        let Provider {
            ref annotations, ..
        } = entry.remove();

        // Send a request to the provider, requesting a graceful shutdown
        let req = serde_json::to_vec(&json!({ "host_id": host_id }))
            .context("failed to encode provider stop request")?;
        let req = async_nats::Request::new()
            .payload(req.into())
            .timeout(self.config.provider_shutdown_delay)
            .headers(injector_to_headers(
                &TraceContextInjector::default_with_span(),
            ));
        if let Err(e) = self
            .rpc_nats
            .send_request(
                format!(
                    "wasmbus.rpc.{}.{provider_id}.default.shutdown",
                    &(*self.name),
                ),
                req,
            )
            .await
        {
            warn!(
                ?e,
                provider_id,
                "provider did not gracefully shut down in time, shutting down forcefully"
            );
        }
        info!(provider_id, "provider stopped");
        self.publish_event(
            "provider_stopped",
            event::provider_stopped(annotations, host_id, provider_id, "stop"),
        )
        .await?;
        Ok(CtlResponse::<()>::success(
            "successfully stopped provider".into(),
        ))
    }

    #[instrument(level = "trace", skip_all)]
    async fn handle_claims(&self) -> anyhow::Result<CtlResponse<Vec<HashMap<String, String>>>> {
        trace!("handling claims");

        let (component_claims, provider_claims) =
            join!(self.component_claims.read(), self.provider_claims.read());
        let component_claims = component_claims.values().cloned().map(Claims::Component);
        let provider_claims = provider_claims.values().cloned().map(Claims::Provider);
        let claims: Vec<StoredClaims> = component_claims
            .chain(provider_claims)
            .flat_map(TryFrom::try_from)
            .collect();

        Ok(CtlResponse::ok(
            claims.into_iter().map(std::convert::Into::into).collect(),
        ))
    }

    #[instrument(level = "trace", skip_all)]
    async fn handle_links(&self) -> anyhow::Result<Vec<u8>> {
        trace!("handling links");

        let links = self.links.read().await;
        let links: Vec<&Link> = links.values().flatten().collect();
        let res =
            serde_json::to_vec(&CtlResponse::ok(links)).context("failed to serialize response")?;
        Ok(res)
    }

    #[instrument(level = "trace", skip(self))]
    async fn handle_config_get(&self, config_name: &str) -> anyhow::Result<Vec<u8>> {
        trace!(%config_name, "handling get config");
        if let Some(config_bytes) = self.config_data.get(config_name).await? {
            let config_map: HashMap<String, String> = serde_json::from_slice(&config_bytes)
                .context("config data should be a map of string -> string")?;
            serde_json::to_vec(&CtlResponse::ok(config_map)).map_err(anyhow::Error::from)
        } else {
            serde_json::to_vec(&CtlResponse::<()>::success(
                "Configuration not found".into(),
            ))
            .map_err(anyhow::Error::from)
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_label_put(
        &self,
        host_id: &str,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<CtlResponse<()>> {
        let host_label = serde_json::from_slice::<HostLabel>(payload.as_ref())
            .context("failed to deserialize put label request")?;
        let key = host_label.key();
        let value = host_label.value();
        let mut labels = self.labels.write().await;
        match labels.entry(key.into()) {
            BTreeMapEntry::Occupied(mut entry) => {
                info!(key = entry.key(), value, "updated label");
                entry.insert(value.into());
            }
            BTreeMapEntry::Vacant(entry) => {
                info!(key = entry.key(), value, "set label");
                entry.insert(value.into());
            }
        }

        self.publish_event(
            "labels_changed",
            event::labels_changed(host_id, HashMap::from_iter(labels.clone())),
        )
        .await
        .context("failed to publish labels_changed event")?;

        Ok(CtlResponse::<()>::success("successfully put label".into()))
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_label_del(
        &self,
        host_id: &str,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<CtlResponse<()>> {
        let label = serde_json::from_slice::<HostLabel>(payload.as_ref())
            .context("failed to deserialize delete label request")?;
        let key = label.key();
        let mut labels = self.labels.write().await;
        let value = labels.remove(key);

        if value.is_none() {
            warn!(key, "could not remove unset label");
            return Ok(CtlResponse::<()>::success(
                "successfully deleted label (no such label)".into(),
            ));
        };

        info!(key, "removed label");
        self.publish_event(
            "labels_changed",
            event::labels_changed(host_id, HashMap::from_iter(labels.clone())),
        )
        .await
        .context("failed to publish labels_changed event")?;

        Ok(CtlResponse::<()>::success(
            "successfully deleted label".into(),
        ))
    }

    /// Handle a new link by modifying the relevant source [ComponentSpeficication]. Once
    /// the change is written to the LATTICEDATA store, each host in the lattice (including this one)
    /// will handle the new specification and update their own internal link maps via [process_component_spec_put].
    #[instrument(level = "debug", skip_all)]
    async fn handle_link_put(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<CtlResponse<()>> {
        let payload = payload.as_ref();
        let link: Link = serde_json::from_slice(payload)
            .context("failed to deserialize wrpc link definition")?;

        let link_set_result: anyhow::Result<()> = async {
                        let source_id = link.source_id();
            let target = link.target();
            let wit_namespace = link.wit_namespace();
            let wit_package = link.wit_package();
            let interfaces = link.interfaces();
            let name = link.name();

            let ns_and_package = format!("{wit_namespace}:{wit_package}");
                        debug!(
                source_id,
                target,
                ns_and_package,
                name,
                ?interfaces,
                "handling put wrpc link definition"
            );

            // Validate all configurations
              self.validate_config(
                link
                    .source_config()
                    .clone()
                    .iter()
                    .chain(link.target_config())
            ).await?;

            let mut component_spec = self
                .get_component_spec(source_id)
                .await?
                .unwrap_or_default();

            // If the link is defined from this source on the same interface and link name, but to a different target,
            // we need to reject this link and suggest deleting the existing link or using a different link name.
            if let Some(existing_conflict_link) = component_spec.links.iter().find(|link| {
                link.source_id() == source_id
                    && link.wit_namespace() == wit_namespace
                    && link.wit_package() == wit_package
                    && link.name() == name
                    && link.target() != target
            }) {
                error!(source_id, desired_target = target, existing_target = existing_conflict_link.target(), ns_and_package, name, "link already exists with different target, consider deleting the existing link or using a different link name");
                bail!("link already exists with different target, consider deleting the existing link or using a different link name");
            }

            // If we can find an existing link with the same source, target, namespace, package, and name, update it.
            // Otherwise, add the new link to the component specification.
            if let Some(existing_link_index) = component_spec.links.iter().position(|link| {
                link.source_id() == source_id
                    && link.target() == target
                    && link.wit_namespace() == wit_namespace
                    && link.wit_package() == wit_package
                    && link.name() == name
            }) {
                if let Some(existing_link) = component_spec.links.get_mut(existing_link_index) {
                    *existing_link = link.clone();
                }
            } else {
                component_spec.links.push(link.clone());
            };

            // Update component specification with the new link
            self.store_component_spec(&source_id, &component_spec)
                .await?;

            self.put_backwards_compat_provider_link(&link)
                .await?;

            Ok(())
        }
        .await;

        if let Err(e) = link_set_result {
            self.publish_event("linkdef_set_failed", event::linkdef_set_failed(&link, &e))
                .await?;
            Ok(CtlResponse::error(e.to_string().as_ref()))
        } else {
            self.publish_event("linkdef_set", event::linkdef_set(&link))
                .await?;
            Ok(CtlResponse::<()>::success("successfully set link".into()))
        }
    }

    #[instrument(level = "debug", skip_all)]
    /// Remove an interface link on a source component for a specific package
    async fn handle_link_del(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<CtlResponse<()>> {
        let payload = payload.as_ref();
        let req = serde_json::from_slice::<DeleteInterfaceLinkDefinitionRequest>(payload)
            .context("failed to deserialize wrpc link definition")?;
        let source_id = req.source_id();
        let wit_namespace = req.wit_namespace();
        let wit_package = req.wit_package();
        let link_name = req.link_name();

        let ns_and_package = format!("{wit_namespace}:{wit_package}");

        debug!(
            source_id,
            ns_and_package, link_name, "handling del wrpc link definition"
        );

        let Some(mut component_spec) = self.get_component_spec(source_id).await? else {
            // If the component spec doesn't exist, the link is deleted
            return Ok(CtlResponse::<()>::success(
                "successfully deleted link (spec doesn't exist)".into(),
            ));
        };

        // If we can find an existing link with the same source, namespace, package, and name, remove it
        // and update the component specification.
        let deleted_link = if let Some(existing_link_index) =
            component_spec.links.iter().position(|link| {
                link.source_id() == source_id
                    && link.wit_namespace() == wit_namespace
                    && link.wit_package() == wit_package
                    && link.name() == link_name
            }) {
            // Sanity safety check since `swap_remove` will panic if the index is out of bounds
            if existing_link_index < component_spec.links.len() {
                Some(component_spec.links.swap_remove(existing_link_index))
            } else {
                None
            }
        } else {
            None
        };

        if let Some(link) = deleted_link.as_ref() {
            // Update component specification with the deleted link
            self.store_component_spec(&source_id, &component_spec)
                .await?;

            // Send the link to providers for deletion
            self.del_provider_link(link).await?;
        }

        // For idempotency, we always publish the deleted event, even if the link didn't exist
        let deleted_link_target = deleted_link
            .as_ref()
            .map(|link| String::from(link.target()));
        self.publish_event(
            "linkdef_deleted",
            event::linkdef_deleted(
                source_id,
                deleted_link_target.as_ref(),
                link_name,
                wit_namespace,
                wit_package,
                deleted_link.as_ref().map(|link| link.interfaces()),
            ),
        )
        .await?;

        Ok(CtlResponse::<()>::success(
            "successfully deleted link".into(),
        ))
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_registries_put(
        &self,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<CtlResponse<()>> {
        let registry_creds: HashMap<String, RegistryCredential> =
            serde_json::from_slice(payload.as_ref())
                .context("failed to deserialize registries put command")?;

        info!(
            registries = ?registry_creds.keys(),
            "updating registry config",
        );

        let mut registry_config = self.registry_config.write().await;
        for (reg, new_creds) in registry_creds {
            let mut new_config = new_creds.into_registry_config()?;
            match registry_config.entry(reg) {
                hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().set_auth(new_config.auth().clone());
                }
                hash_map::Entry::Vacant(entry) => {
                    new_config.set_allow_latest(self.config.oci_opts.allow_latest);
                    entry.insert(new_config);
                }
            }
        }

        Ok(CtlResponse::<()>::success(
            "successfully put registries".into(),
        ))
    }

    #[instrument(level = "debug", skip_all, fields(%config_name))]
    async fn handle_config_put(
        &self,
        config_name: &str,
        data: Bytes,
    ) -> anyhow::Result<CtlResponse<()>> {
        debug!("handle config entry put");
        // Validate that the data is of the proper type by deserialing it
        serde_json::from_slice::<HashMap<String, String>>(&data)
            .context("config data should be a map of string -> string")?;
        self.config_data
            .put(config_name, data)
            .await
            .context("unable to store config data")?;
        // We don't write it into the cached data and instead let the caching thread handle it as we
        // won't need it immediately.
        self.publish_event("config_set", event::config_set(config_name))
            .await?;

        Ok(CtlResponse::<()>::success("successfully put config".into()))
    }

    #[instrument(level = "debug", skip_all, fields(%config_name))]
    async fn handle_config_delete(&self, config_name: &str) -> anyhow::Result<CtlResponse<()>> {
        debug!("handle config entry deletion");

        self.config_data
            .purge(config_name)
            .await
            .context("Unable to delete config data")?;

        self.publish_event("config_deleted", event::config_deleted(config_name))
            .await?;

        Ok(CtlResponse::<()>::success(
            "successfully deleted config".into(),
        ))
    }

    #[instrument(level = "trace", skip_all, fields(subject = %message.subject))]
    async fn handle_ctl_message(self: Arc<Self>, message: async_nats::Message) {
        // NOTE: if log level is not `trace`, this won't have an effect, since the current span is
        // disabled. In most cases that's fine, since we aren't aware of any control interface
        // requests including a trace context
        opentelemetry_nats::attach_span_context(&message);
        // Skip the topic prefix, the version, and the lattice
        // e.g. `wasmbus.ctl.v1.{prefix}`
        let subject = message.subject;
        let mut parts = subject
            .trim()
            .trim_start_matches(&self.ctl_topic_prefix)
            .trim_start_matches('.')
            .split('.')
            .skip(2);
        trace!(%subject, "handling control interface request");

        // This response is a wrapped Result<Option<Result<Vec<u8>>>> for a good reason.
        // The outer Result is for reporting protocol errors in handling the request, e.g. failing to
        //    deserialize the request payload.
        // The Option is for the case where the request is handled successfully, but the handler
        //    doesn't want to send a response back to the client, like with an auction.
        // The inner Result is purely for the success or failure of serializing the [CtlResponse], which
        //    should never fail but it's a result we must handle.
        // And finally, the Vec<u8> is the serialized [CtlResponse] that we'll send back to the client
        let ctl_response = match (parts.next(), parts.next(), parts.next(), parts.next()) {
            // Component commands
            (Some("component"), Some("auction"), None, None) => self
                .handle_auction_component(message.payload)
                .await
                .map(serialize_ctl_response),
            (Some("component"), Some("scale"), Some(host_id), None) => Arc::clone(&self)
                .handle_scale_component(message.payload, host_id)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("component"), Some("update"), Some(host_id), None) => Arc::clone(&self)
                .handle_update_component(message.payload, host_id)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Provider commands
            (Some("provider"), Some("auction"), None, None) => self
                .handle_auction_provider(message.payload)
                .await
                .map(serialize_ctl_response),
            (Some("provider"), Some("start"), Some(host_id), None) => Arc::clone(&self)
                .handle_start_provider(message.payload, host_id)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("provider"), Some("stop"), Some(host_id), None) => self
                .handle_stop_provider(message.payload, host_id)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Claims commands
            (Some("claims"), Some("get"), None, None) => self
                .handle_claims()
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Link commands
            (Some("link"), Some("del"), None, None) => self
                .handle_link_del(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("link"), Some("get"), None, None) => {
                // Explicitly returning a Vec<u8> for non-cloning efficiency within handle_links
                self.handle_links().await.map(|bytes| Some(Ok(bytes)))
            }
            (Some("link"), Some("put"), None, None) => self
                .handle_link_put(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Registry commands
            (Some("registry"), Some("put"), None, None) => self
                .handle_registries_put(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Config commands
            (Some("config"), Some("get"), Some(config_name), None) => self
                .handle_config_get(config_name)
                .await
                .map(|bytes| Some(Ok(bytes))),
            (Some("config"), Some("put"), Some(config_name), None) => self
                .handle_config_put(config_name, message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("config"), Some("del"), Some(config_name), None) => self
                .handle_config_delete(config_name)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Topic fallback
            _ => {
                warn!(%subject, "received control interface request on unsupported subject");
                Ok(serialize_ctl_response(Some(CtlResponse::error(
                    "unsupported subject",
                ))))
            }
        };

        if let Err(err) = &ctl_response {
            error!(%subject, ?err, "failed to handle control interface request");
        } else {
            trace!(%subject, "handled control interface request");
        }

        if let Some(reply) = message.reply {
            let headers = injector_to_headers(&TraceContextInjector::default_with_span());

            let payload = match ctl_response {
                Ok(Some(Ok(payload))) => Some(payload.into()),
                // No response from the host (e.g. auctioning provider)
                Ok(None) => None,
                Err(e) => Some(
                    serde_json::to_vec(&CtlResponse::error(&e.to_string()))
                        .context("failed to encode control interface response")
                        // This should never fail to serialize, but the fallback ensures that we send
                        // something back to the client even if we somehow fail.
                        .unwrap_or_else(|_| format!(r#"{{"success":false,"error":"{e}"}}"#).into())
                        .into(),
                ),
                // This would only occur if we failed to serialize a valid CtlResponse. This is
                // programmer error.
                Ok(Some(Err(e))) => Some(
                    serde_json::to_vec(&CtlResponse::error(&e.to_string()))
                        .context("failed to encode control interface response")
                        .unwrap_or_else(|_| format!(r#"{{"success":false,"error":"{e}"}}"#).into())
                        .into(),
                ),
            };

            if let Some(payload) = payload {
                if let Err(err) = self
                    .ctl_nats
                    .publish_with_headers(reply.clone(), headers, payload)
                    .err_into::<anyhow::Error>()
                    .and_then(|()| self.ctl_nats.flush().err_into::<anyhow::Error>())
                    .await
                {
                    error!(%subject, ?err, "failed to publish reply to control interface request");
                }
            }
        }
    }

    // TODO: Remove this before wasmCloud 1.2 is released. This is a backwards-compatible
    // provider link definition put that is published to the provider's id, which is what
    // providers built for wasmCloud 1.0 expected.
    //
    // Thankfully, in a lattice where there are no "older" providers running, these publishes
    // will return immediately as there will be no subscribers on those topics.
    async fn put_backwards_compat_provider_link(&self, link: &Link) -> anyhow::Result<()> {
        // Only attempt to publish the backwards-compatible provider link definition if the link
        // does not contain any secret values.
        let source_config_contains_secret = link
            .source_config()
            .iter()
            .any(|c| c.starts_with(SECRET_PREFIX));
        let target_config_contains_secret = link
            .target_config()
            .iter()
            .any(|c| c.starts_with(SECRET_PREFIX));
        if source_config_contains_secret || target_config_contains_secret {
            debug!("link contains secrets and is not backwards compatible, skipping");
            return Ok(());
        }
        let provider_link = self
            .resolve_link_config(link.clone(), None, None, &XKey::new())
            .await
            .context("failed to resolve link config")?;
        let lattice = &self.name;
        let payload: Bytes = serde_json::to_vec(&provider_link)
            .context("failed to serialize provider link definition")?
            .into();

        if let Err(e) = self
            .rpc_nats
            .publish_with_headers(
                format!("wasmbus.rpc.{lattice}.{}.linkdefs.put", link.source_id()),
                injector_to_headers(&TraceContextInjector::default_with_span()),
                payload.clone(),
            )
            .await
        {
            warn!(
                ?e,
                "failed to publish backwards-compatible provider link to source"
            );
        }

        if let Err(e) = self
            .rpc_nats
            .publish_with_headers(
                format!("wasmbus.rpc.{lattice}.{}.linkdefs.put", link.target()),
                injector_to_headers(&TraceContextInjector::default_with_span()),
                payload,
            )
            .await
        {
            warn!(
                ?e,
                "failed to publish backwards-compatible provider link to target"
            );
        }

        Ok(())
    }

    /// Publishes a link to a provider running on this host to handle.
    #[instrument(level = "debug", skip_all)]
    async fn put_provider_link(&self, provider: &Provider, link: &Link) -> anyhow::Result<()> {
        let provider_link = self
            .resolve_link_config(
                link.clone(),
                provider.claims_token.as_ref().map(|t| &t.jwt),
                provider.annotations.get("wasmcloud.dev/appspec"),
                &provider.xkey,
            )
            .await
            .context("failed to resolve link config and secrets")?;
        let lattice = &self.name;
        let payload: Bytes = serde_json::to_vec(&provider_link)
            .context("failed to serialize provider link definition")?
            .into();

        self.rpc_nats
            .publish_with_headers(
                format!(
                    "wasmbus.rpc.{lattice}.{}.linkdefs.put",
                    provider.xkey.public_key()
                ),
                injector_to_headers(&TraceContextInjector::default_with_span()),
                payload.clone(),
            )
            .await
            .context("failed to publish provider link definition put")
    }

    /// Publishes a delete link to the lattice for all instances of a provider to handle
    /// Right now this is publishing _both_ to the source and the target in order to
    /// ensure that the provider is aware of the link delete. This would cause problems if a provider
    /// is linked to a provider (which it should never be.)
    #[instrument(level = "debug", skip(self))]
    async fn del_provider_link(&self, link: &Link) -> anyhow::Result<()> {
        let lattice = &self.name;
        // The provider expects the [`wasmcloud_core::InterfaceLinkDefinition`]
        let link = wasmcloud_core::InterfaceLinkDefinition {
            source_id: link.source_id().to_string(),
            target: link.target().to_string(),
            wit_namespace: link.wit_namespace().to_string(),
            wit_package: link.wit_package().to_string(),
            name: link.name().to_string(),
            interfaces: link.interfaces().clone(),
            // Configuration isn't needed for deletion
            ..Default::default()
        };
        let source_id = &link.source_id;
        let target = &link.target;
        let payload: Bytes = serde_json::to_vec(&link)
            .context("failed to serialize provider link definition for deletion")?
            .into();

        let (source_result, target_result) = futures::future::join(
            self.rpc_nats.publish_with_headers(
                format!("wasmbus.rpc.{lattice}.{source_id}.linkdefs.del"),
                injector_to_headers(&TraceContextInjector::default_with_span()),
                payload.clone(),
            ),
            self.rpc_nats.publish_with_headers(
                format!("wasmbus.rpc.{lattice}.{target}.linkdefs.del"),
                injector_to_headers(&TraceContextInjector::default_with_span()),
                payload,
            ),
        )
        .await;

        source_result
            .and(target_result)
            .context("failed to publish provider link definition delete")
    }

    /// Retrieve a component specification based on the provided ID. The outer Result is for errors
    /// accessing the store, and the inner option indicates if the spec exists.
    #[instrument(level = "debug", skip_all)]
    async fn get_component_spec(&self, id: &str) -> anyhow::Result<Option<ComponentSpecification>> {
        let key = format!("COMPONENT_{id}");
        let spec = self
            .data
            .get(key)
            .await
            .context("failed to get component spec")?
            .map(|spec_bytes| serde_json::from_slice(&spec_bytes))
            .transpose()
            .context(format!(
                "failed to deserialize stored component specification for {id}"
            ))?;
        Ok(spec)
    }

    #[instrument(level = "debug", skip_all)]
    async fn store_component_spec(
        &self,
        id: impl AsRef<str>,
        spec: &ComponentSpecification,
    ) -> anyhow::Result<()> {
        let id = id.as_ref();
        let key = format!("COMPONENT_{id}");
        let bytes = serde_json::to_vec(spec)
            .context("failed to serialize component spec")?
            .into();
        self.data
            .put(key, bytes)
            .await
            .context("failed to put component spec")?;
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn store_claims(&self, claims: Claims) -> anyhow::Result<()> {
        match &claims {
            Claims::Component(claims) => {
                self.store_component_claims(claims.clone()).await?;
            }
            Claims::Provider(claims) => {
                let mut provider_claims = self.provider_claims.write().await;
                provider_claims.insert(claims.subject.clone(), claims.clone());
            }
        };
        let claims: StoredClaims = claims.try_into()?;
        let subject = match &claims {
            StoredClaims::Component(claims) => &claims.subject,
            StoredClaims::Provider(claims) => &claims.subject,
        };
        let key = format!("CLAIMS_{subject}");
        trace!(?claims, ?key, "storing claims");

        let bytes = serde_json::to_vec(&claims)
            .context("failed to serialize claims")?
            .into();
        self.data
            .put(key, bytes)
            .await
            .context("failed to put claims")?;
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn process_component_spec_put(
        &self,
        id: impl AsRef<str>,
        value: impl AsRef<[u8]>,
        _publish: bool,
    ) -> anyhow::Result<()> {
        let id = id.as_ref();
        debug!(id, "process component spec put");

        let spec: ComponentSpecification = serde_json::from_slice(value.as_ref())
            .context("failed to deserialize component specification")?;

        // Compute all new links that do not exist in the host map, which we'll use to
        // publish to any running providers that are the source or target of the link.
        // Computing this ahead of time is a tradeoff to hold only one lock at the cost of
        // allocating an extra Vec. This may be a good place to optimize allocations.
        let new_links = {
            let all_links = self.links.read().await;
            spec.links
                .iter()
                .filter(|spec_link| {
                    // Retain only links that do not exist in the host map
                    !all_links
                        .iter()
                        .filter_map(|(source_id, links)| {
                            // Only consider links that are either the source or target of the new link
                            if source_id == spec_link.source_id() || source_id == spec_link.target()
                            {
                                Some(links)
                            } else {
                                None
                            }
                        })
                        .flatten()
                        .any(|host_link| *spec_link == host_link)
                })
                .collect::<Vec<_>>()
        };

        {
            // Acquire lock once in this block to avoid continually trying to acquire it.
            let providers = self.providers.read().await;
            // For every new link, if a provider is running on this host as the source or target,
            // send the link to the provider for handling based on the xkey public key.
            for link in new_links {
                if let Some(provider) = providers.get(link.source_id()) {
                    if let Err(e) = self.put_provider_link(provider, link).await {
                        error!(?e, "failed to put provider link");
                    }
                }
                if let Some(provider) = providers.get(link.target()) {
                    if let Err(e) = self.put_provider_link(provider, link).await {
                        error!(?e, "failed to put provider link");
                    }
                }
            }
        }

        // If the component is already running, update the links
        if let Some(component) = self.components.write().await.get(id) {
            *component.handler.instance_links.write().await = component_import_links(&spec.links);
            // NOTE(brooksmtownsend): We can consider updating the component if the image URL changes
        };

        // Insert the links into host map
        self.links.write().await.insert(id.to_string(), spec.links);

        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn process_component_spec_delete(
        &self,
        id: impl AsRef<str>,
        _value: impl AsRef<[u8]>,
        _publish: bool,
    ) -> anyhow::Result<()> {
        let id = id.as_ref();
        debug!(id, "process component delete");
        // TODO: TBD: stop component if spec deleted?
        if self.components.write().await.get(id).is_some() {
            warn!(
                component_id = id,
                "component spec deleted, but component is still running"
            );
        }
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn process_claims_put(
        &self,
        pubkey: impl AsRef<str>,
        value: impl AsRef<[u8]>,
    ) -> anyhow::Result<()> {
        let pubkey = pubkey.as_ref();

        debug!(pubkey, "process claim entry put");

        let stored_claims: StoredClaims =
            serde_json::from_slice(value.as_ref()).context("failed to decode stored claims")?;
        let claims = Claims::from(stored_claims);

        ensure!(claims.subject() == pubkey, "subject mismatch");
        match claims {
            Claims::Component(claims) => self.store_component_claims(claims).await,
            Claims::Provider(claims) => {
                let mut provider_claims = self.provider_claims.write().await;
                provider_claims.insert(claims.subject.clone(), claims);
                Ok(())
            }
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn process_claims_delete(
        &self,
        pubkey: impl AsRef<str>,
        value: impl AsRef<[u8]>,
    ) -> anyhow::Result<()> {
        let pubkey = pubkey.as_ref();

        debug!(pubkey, "process claim entry deletion");

        let stored_claims: StoredClaims =
            serde_json::from_slice(value.as_ref()).context("failed to decode stored claims")?;
        let claims = Claims::from(stored_claims);

        ensure!(claims.subject() == pubkey, "subject mismatch");

        match claims {
            Claims::Component(claims) => {
                let mut component_claims = self.component_claims.write().await;
                component_claims.remove(&claims.subject);
            }
            Claims::Provider(claims) => {
                let mut provider_claims = self.provider_claims.write().await;
                provider_claims.remove(&claims.subject);
            }
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all)]
    async fn process_entry(
        &self,
        KvEntry {
            key,
            value,
            operation,
            ..
        }: KvEntry,
        publish: bool,
    ) {
        let key_id = key.split_once('_');
        let res = match (operation, key_id) {
            (Operation::Put, Some(("COMPONENT", id))) => {
                self.process_component_spec_put(id, value, publish).await
            }
            (Operation::Delete, Some(("COMPONENT", id))) => {
                self.process_component_spec_delete(id, value, publish).await
            }
            (Operation::Put, Some(("LINKDEF", _id))) => {
                debug!("ignoring deprecated LINKDEF put operation");
                Ok(())
            }
            (Operation::Delete, Some(("LINKDEF", _id))) => {
                debug!("ignoring deprecated LINKDEF delete operation");
                Ok(())
            }
            (Operation::Put, Some(("CLAIMS", pubkey))) => {
                self.process_claims_put(pubkey, value).await
            }
            (Operation::Delete, Some(("CLAIMS", pubkey))) => {
                self.process_claims_delete(pubkey, value).await
            }
            (operation, Some(("REFMAP", id))) => {
                // TODO: process REFMAP entries
                debug!(?operation, id, "ignoring REFMAP entry");
                Ok(())
            }
            _ => {
                warn!(key, ?operation, "unsupported KV bucket entry");
                Ok(())
            }
        };
        if let Err(error) = &res {
            error!(key, ?operation, ?error, "failed to process KV bucket entry");
        }
    }

    async fn fetch_config_and_secrets(
        &self,
        config_names: &[String],
        entity_jwt: Option<&String>,
        application: Option<&String>,
    ) -> anyhow::Result<(ConfigBundle, HashMap<String, Secret<SecretValue>>)> {
        let (secret_names, config_names) = config_names
            .iter()
            .map(|s| s.to_string())
            .partition(|name| name.starts_with(SECRET_PREFIX));

        let config = self
            .config_generator
            .generate(config_names)
            .await
            .context("Unable to fetch requested config")?;

        let secrets = self
            .secrets_manager
            .fetch_secrets(secret_names, entity_jwt, &self.host_token.jwt, application)
            .await
            .context("Unable to fetch requested secrets")?;

        Ok((config, secrets))
    }

    /// Validates that the provided configuration names exist in the store and are valid.
    ///
    /// For any configuration that starts with `SECRET_`, the configuration is expected to be a secret reference.
    /// For any other configuration, the configuration is expected to be a [`HashMap<String, String>`].
    async fn validate_config<I>(&self, config_names: I) -> anyhow::Result<()>
    where
        I: IntoIterator<Item: AsRef<str>>,
    {
        let config_store = self.config_data.clone();
        let validation_errors =
            futures::future::join_all(config_names.into_iter().map(|config_name| {
                let config_store = config_store.clone();
                let config_name = config_name.as_ref().to_string();
                async move {
                    match config_store.get(&config_name).await {
                        Ok(Some(_)) => None,
                        Ok(None) if config_name.starts_with(SECRET_PREFIX) => Some(format!(
                            "Secret reference {config_name} not found in config store"
                        )),
                        Ok(None) => Some(format!(
                            "Configuration {config_name} not found in config store"
                        )),
                        Err(e) => Some(e.to_string()),
                    }
                }
            }))
            .await;

        // NOTE(brooksmtownsend): Not using `join` here because it requires a `String` and we
        // need to flatten out the `None` values.
        let validation_errors = validation_errors
            .into_iter()
            .flatten()
            .fold(String::new(), |acc, e| acc + &e + ". ");
        if !validation_errors.is_empty() {
            bail!(format!(
                "Failed to validate configuration and secrets. {validation_errors}",
            ));
        }

        Ok(())
    }

    /// Transform a [`wasmcloud_control_interface::Link`] into a [`wasmcloud_core::InterfaceLinkDefinition`]
    /// by fetching the source and target configurations and secrets, and encrypting the secrets.
    async fn resolve_link_config(
        &self,
        link: Link,
        provider_jwt: Option<&String>,
        application: Option<&String>,
        provider_xkey: &XKey,
    ) -> anyhow::Result<wasmcloud_core::InterfaceLinkDefinition> {
        let (source_bundle, raw_source_secrets) = self
            .fetch_config_and_secrets(link.source_config().as_slice(), provider_jwt, application)
            .await?;
        let (target_bundle, raw_target_secrets) = self
            .fetch_config_and_secrets(link.target_config().as_slice(), provider_jwt, application)
            .await?;

        let source_config = source_bundle.get_config().await;
        let target_config = target_bundle.get_config().await;
        // NOTE(brooksmtownsend): This trait import is used here to ensure we're only exposing secret
        // values when we need them.
        use secrecy::ExposeSecret;
        let source_secrets_map: HashMap<String, wasmcloud_core::secrets::SecretValue> =
            raw_source_secrets
                .iter()
                .map(|(k, v)| match v.expose_secret() {
                    SecretValue::String(s) => (
                        k.clone(),
                        wasmcloud_core::secrets::SecretValue::String(s.to_owned()),
                    ),
                    SecretValue::Bytes(b) => (
                        k.clone(),
                        wasmcloud_core::secrets::SecretValue::Bytes(b.to_owned()),
                    ),
                })
                .collect();
        let target_secrets_map: HashMap<String, wasmcloud_core::secrets::SecretValue> =
            raw_target_secrets
                .iter()
                .map(|(k, v)| match v.expose_secret() {
                    SecretValue::String(s) => (
                        k.clone(),
                        wasmcloud_core::secrets::SecretValue::String(s.to_owned()),
                    ),
                    SecretValue::Bytes(b) => (
                        k.clone(),
                        wasmcloud_core::secrets::SecretValue::Bytes(b.to_owned()),
                    ),
                })
                .collect();
        // Serializing & sealing an empty map results in a non-empty Vec, which is difficult to tell the
        // difference between an empty map and an encrypted empty map. To avoid this, we explicitly handle
        // the case where the map is empty.
        let source_secrets = if source_secrets_map.is_empty() {
            None
        } else {
            Some(
                serde_json::to_vec(&source_secrets_map)
                    .map(|secrets| self.secrets_xkey.seal(&secrets, provider_xkey))
                    .context("failed to serialize and encrypt source secrets")??,
            )
        };
        let target_secrets = if target_secrets_map.is_empty() {
            None
        } else {
            Some(
                serde_json::to_vec(&target_secrets_map)
                    .map(|secrets| self.secrets_xkey.seal(&secrets, provider_xkey))
                    .context("failed to serialize and encrypt target secrets")??,
            )
        };

        Ok(wasmcloud_core::InterfaceLinkDefinition {
            source_id: link.source_id().to_string(),
            target: link.target().to_string(),
            name: link.name().to_string(),
            wit_namespace: link.wit_namespace().to_string(),
            wit_package: link.wit_package().to_string(),
            interfaces: link.interfaces().clone(),
            source_config: source_config.clone(),
            target_config: target_config.clone(),
            source_secrets,
            target_secrets,
        })
    }
}

async fn publish_event(
    event_rx: mpsc::Sender<(String, String, serde_json::Value)>,
    name: String,
    lattice: String,
    data: serde_json::Value,
) -> anyhow::Result<()> {
    event_rx
        .send((name, lattice, data))
        .await
        .context("failed to send event")
}
