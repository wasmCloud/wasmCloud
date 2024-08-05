mod claims;
mod ctl;
mod event;
mod handler;

/// Component and provider configuration
pub mod config;
/// wasmCloud host configuration
pub mod host_config;

pub use self::host_config::Host as HostConfig;

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::env::consts::{ARCH, FAMILY, OS};
use std::future::Future;
use std::num::NonZeroUsize;
use std::ops::Deref;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Context as _};
use async_nats::jetstream::kv::{Entry as KvEntry, Operation, Store};
use bytes::{BufMut, Bytes, BytesMut};
use claims::{Claims, StoredClaims};
use cloudevents::{EventBuilder, EventBuilderV10};
use ctl::Queue;
use futures::stream::{AbortHandle, Abortable};
use futures::{join, stream, try_join, Stream, StreamExt, TryStreamExt};
use nkeys::{KeyPair, KeyPairType, XKey};
use secrecy::Secret;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::AsyncWrite;
use tokio::sync::{mpsc, watch, RwLock, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{interval_at, timeout_at, Instant};
use tokio::{select, spawn};
use tokio_stream::wrappers::IntervalStream;
use tracing::{debug, error, info, instrument, trace, warn, Instrument as _};
use wascap::jwt;
use wasmcloud_control_interface::{
    ComponentDescription, HostInventory, InterfaceLinkDefinition, ProviderDescription,
    RegistryCredential,
};
use wasmcloud_core::ComponentId;
use wasmcloud_runtime::capability::secrets::store::SecretValue;
use wasmcloud_runtime::component::WrpcServeEvent;
use wasmcloud_runtime::Runtime;
use wasmcloud_secrets_types::SECRET_PREFIX;
use wasmcloud_tracing::context::TraceContextInjector;
use wasmcloud_tracing::{global, KeyValue};

use crate::wasmbus::config::{BundleGenerator, ConfigBundle};
use crate::wasmbus::handler::Handler;
use crate::{
    HostMetrics, OciConfig, PolicyHostInfo, PolicyManager, PolicyResponse, RegistryAuth,
    RegistryConfig, RegistryType, SecretsManager,
};

#[derive(Clone, Default)]
struct AsyncBytesMut(Arc<std::sync::Mutex<BytesMut>>);

impl AsyncWrite for AsyncBytesMut {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Poll::Ready({
            self.0
                .lock()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?
                .put_slice(buf);
            Ok(buf.len())
        })
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }
}

impl TryFrom<AsyncBytesMut> for Vec<u8> {
    type Error = anyhow::Error;

    fn try_from(buf: AsyncBytesMut) -> Result<Self, Self::Error> {
        buf.0
            .lock()
            .map(|buf| buf.clone().into())
            .map_err(|e| anyhow!(e.to_string()).context("failed to lock"))
    }
}

type Annotations = BTreeMap<String, String>;

#[derive(Debug)]
struct Component {
    component: wasmcloud_runtime::Component<Handler>,
    /// Unique component identifier for this component
    id: Arc<str>,
    handler: Handler,
    exports: JoinHandle<()>,
    annotations: Annotations,
    /// Maximum number of instances of this component that can be running at once
    max_instances: NonZeroUsize,
    image_reference: Arc<str>,
}

impl Deref for Component {
    type Target = wasmcloud_runtime::Component<Handler>;

    fn deref(&self) -> &Self::Target {
        &self.component
    }
}

#[derive(Clone)]
struct WrpcServer {
    nats: wrpc_transport_nats::Client,
    claims: Option<Arc<jwt::Claims<jwt::Component>>>,
    id: Arc<str>,
    image_reference: Arc<str>,
    annotations: Arc<Annotations>,
    policy_manager: Arc<PolicyManager>,
    trace_ctx: Arc<RwLock<Vec<(String, String)>>>,
    metrics: Arc<HostMetrics>,
}

impl wrpc_transport::Serve for WrpcServer {
    type Context = (Instant, Vec<KeyValue>);
    type Outgoing = <wrpc_transport_nats::Client as wrpc_transport::Serve>::Outgoing;
    type Incoming = <wrpc_transport_nats::Client as wrpc_transport::Serve>::Incoming;

    #[instrument(
        level = "info",
        skip(self, paths),
        fields(
            component_id = ?self.id,
            component_ref = ?self.image_reference)
    )]
    async fn serve(
        &self,
        instance: &str,
        func: &str,
        paths: impl Into<Arc<[Box<[Option<usize>]>]>> + Send,
    ) -> anyhow::Result<
        impl Stream<Item = anyhow::Result<(Self::Context, Self::Outgoing, Self::Incoming)>>
            + Send
            + 'static,
    > {
        debug!("serving invocations");
        let invocations = self.nats.serve(instance, func, paths).await?;

        let func: Arc<str> = Arc::from(func);
        let instance: Arc<str> = Arc::from(instance);
        let annotations = Arc::clone(&self.annotations);
        let id = Arc::clone(&self.id);
        let image_reference = Arc::clone(&self.image_reference);
        let metrics = Arc::clone(&self.metrics);
        let policy_manager = Arc::clone(&self.policy_manager);
        let trace_ctx = Arc::clone(&self.trace_ctx);
        let claims = self.claims.clone();
        Ok(invocations.and_then(move |(cx, tx, rx)| {
            {
                let annotations = Arc::clone(&annotations);
                let claims = claims.clone();
                let func = Arc::clone(&func);
                let id = Arc::clone(&id);
                let image_reference = Arc::clone(&image_reference);
                let instance = Arc::clone(&instance);
                let metrics = Arc::clone(&metrics);
                let policy_manager = Arc::clone(&policy_manager);
                let trace_ctx = Arc::clone(&trace_ctx);
                async move {
                    let PolicyResponse {
                        request_id,
                        permitted,
                        message,
                    } = policy_manager
                        .evaluate_perform_invocation(
                            &id,
                            &image_reference,
                            &annotations,
                            claims.as_deref(),
                            instance.to_string(),
                            func.to_string(),
                        )
                        .await?;
                    ensure!(
                        permitted,
                        "policy denied request to invoke component `{request_id}`: `{message:?}`",
                    );

                    if let Some(ref cx) = cx {
                        // TODO: wasmcloud_tracing take HeaderMap for my own sanity
                        // Coerce the HashMap<String, Vec<String>> into a Vec<(String, String)> by
                        // flattening the values
                        let trace_context = cx
                            .iter()
                            .flat_map(|(key, value)| {
                                value
                                    .iter()
                                    .map(|v| (key.to_string(), v.to_string()))
                                    .collect::<Vec<_>>()
                            })
                            .collect::<Vec<(String, String)>>();
                        wasmcloud_tracing::context::attach_span_context(&trace_context);
                    }

                    // Associate the current context with the span
                    let injector = TraceContextInjector::default_with_span();
                    *trace_ctx.write().await = injector
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();
                    Ok((
                        (
                            Instant::now(),
                            // TODO(metrics): insert information about the source once we have concrete context data
                            vec![
                                KeyValue::new("component.ref", image_reference),
                                KeyValue::new("lattice", metrics.lattice_id.clone()),
                                KeyValue::new("host", metrics.host_id.clone()),
                                KeyValue::new("operation", format!("{instance}/name")),
                            ],
                        ),
                        tx,
                        rx,
                    ))
                }
            }
            .in_current_span()
        }))
    }
}

#[derive(Debug)]
struct Provider {
    child: JoinHandle<()>,
    annotations: Annotations,
    image_ref: String,
    claims_token: Option<jwt::Token<jwt::CapabilityProvider>>,
    xkey: XKey,
}

/// wasmCloud Host
pub struct Host {
    components: RwLock<HashMap<ComponentId, Arc<Component>>>,
    event_builder: EventBuilderV10,
    friendly_name: String,
    heartbeat: AbortHandle,
    host_config: HostConfig,
    host_key: Arc<KeyPair>,
    host_token: Arc<jwt::Token<jwt::Host>>,
    /// The Xkey used to encrypt secrets when sending them over NATS
    secrets_xkey: Arc<XKey>,
    labels: RwLock<HashMap<String, String>>,
    ctl_topic_prefix: String,
    /// NATS client to use for control interface subscriptions and jetstream queries
    ctl_nats: async_nats::Client,
    /// NATS client to use for RPC calls
    rpc_nats: Arc<async_nats::Client>,
    data: Store,
    /// Task to watch for changes in the LATTICEDATA store
    data_watch: AbortHandle,
    config_data: Store,
    config_generator: BundleGenerator,
    policy_manager: Arc<PolicyManager>,
    secrets_manager: Arc<SecretsManager>,
    /// The provider map is a map of provider component ID to provider
    providers: RwLock<HashMap<String, Provider>>,
    registry_config: RwLock<HashMap<String, RegistryConfig>>,
    runtime: Runtime,
    start_at: Instant,
    stop_tx: watch::Sender<Option<Instant>>,
    stop_rx: watch::Receiver<Option<Instant>>,
    queue: AbortHandle,
    // Component ID -> All Links
    links: RwLock<HashMap<String, Vec<InterfaceLinkDefinition>>>,
    component_claims: Arc<RwLock<HashMap<ComponentId, jwt::Claims<jwt::Component>>>>, // TODO: use a single map once Claims is an enum
    provider_claims: Arc<RwLock<HashMap<String, jwt::Claims<jwt::CapabilityProvider>>>>,
    metrics: Arc<HostMetrics>,
    max_execution_time: Duration,
}

#[derive(Debug, Serialize, Deserialize, Default)]
/// The specification of a component that is or did run in the lattice. This contains all of the information necessary to
/// instantiate a component in the lattice (url and digest) as well as configuration and links in order to facilitate
/// runtime execution of the component. Each `import` in a component's WIT world will need a corresponding link for the
/// host runtime to route messages to the correct component.
pub struct ComponentSpecification {
    /// The URL of the component, file, OCI, or otherwise
    url: String,
    /// All outbound links from this component to other components, used for routing when calling a component `import`
    links: Vec<InterfaceLinkDefinition>,
    ////
    // Possible additions in the future, left in as comments to facilitate discussion
    ////
    // /// The claims embedded in the component, if present
    // claims: Option<Claims>,
    // /// SHA256 digest of the component, used for checking uniqueness of component IDs
    // digest: String
    // /// (Advanced) Additional routing topics to subscribe on in addition to the component ID.
    // routing_groups: Vec<String>,
}

impl ComponentSpecification {
    /// Create a new empty component specification with the given ID and URL
    pub fn new(url: impl AsRef<str>) -> Self {
        Self {
            url: url.as_ref().to_string(),
            links: Vec::new(),
        }
    }
}

#[instrument(level = "debug", skip_all)]
async fn create_bucket(
    jetstream: &async_nats::jetstream::Context,
    bucket: &str,
) -> anyhow::Result<Store> {
    // Don't create the bucket if it already exists
    if let Ok(store) = jetstream.get_key_value(bucket).await {
        info!(%bucket, "bucket already exists. Skipping creation.");
        return Ok(store);
    }

    match jetstream
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: bucket.to_string(),
            ..Default::default()
        })
        .await
    {
        Ok(store) => {
            info!(%bucket, "created bucket with 1 replica");
            Ok(store)
        }
        Err(err) => Err(anyhow!(err).context(format!("failed to create bucket '{bucket}'"))),
    }
}

/// Given the NATS address, authentication jwt, seed, tls requirement and optional request timeout,
/// attempt to establish connection.
///
///
/// # Errors
///
/// Returns an error if:
/// - Only one of JWT or seed is specified, as we cannot authenticate with only one of them
/// - Connection fails
async fn connect_nats(
    addr: impl async_nats::ToServerAddrs,
    jwt: Option<&String>,
    key: Option<Arc<KeyPair>>,
    require_tls: bool,
    request_timeout: Option<Duration>,
) -> anyhow::Result<async_nats::Client> {
    let opts = async_nats::ConnectOptions::new().require_tls(require_tls);
    let opts = match (jwt, key) {
        (Some(jwt), Some(key)) => opts.jwt(jwt.to_string(), {
            move |nonce| {
                let key = key.clone();
                async move { key.sign(&nonce).map_err(async_nats::AuthError::new) }
            }
        }),
        (Some(_), None) | (None, Some(_)) => {
            bail!("cannot authenticate if only one of jwt or seed is specified")
        }
        _ => opts,
    };
    let opts = if let Some(timeout) = request_timeout {
        opts.request_timeout(Some(timeout))
    } else {
        opts
    };
    opts.connect(addr)
        .await
        .context("failed to connect to NATS")
}

#[derive(Debug, Default)]
struct SupplementalConfig {
    registry_config: Option<HashMap<String, RegistryConfig>>,
}

#[instrument(level = "debug", skip_all)]
async fn load_supplemental_config(
    ctl_nats: &async_nats::Client,
    lattice: &str,
    labels: &HashMap<String, String>,
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
                    registry_config: ser_cfg.registry_credentials.map(|creds| {
                        creds
                            .into_iter()
                            .map(|(k, v)| {
                                debug!(registry_url = %k, "set registry config");
                                (k, v.into())
                            })
                            .collect()
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

#[instrument(level = "debug", skip_all)]
async fn merge_registry_config(
    registry_config: &RwLock<HashMap<String, RegistryConfig>>,
    oci_opts: OciConfig,
) -> () {
    let mut registry_config = registry_config.write().await;
    let allow_latest = oci_opts.allow_latest;
    let additional_ca_paths = oci_opts.additional_ca_paths;

    // update auth for specific registry, if provided
    if let Some(reg) = oci_opts.oci_registry {
        match registry_config.entry(reg.clone()) {
            Entry::Occupied(_entry) => {
                // note we don't update config here, since the config service should take priority
                warn!(oci_registry_url = %reg, "ignoring OCI registry config, overriden by config service");
            }
            Entry::Vacant(entry) => {
                debug!(oci_registry_url = %reg, "set registry config");
                entry.insert(RegistryConfig {
                    reg_type: RegistryType::Oci,
                    auth: RegistryAuth::from((oci_opts.oci_user, oci_opts.oci_password)),
                    ..Default::default()
                });
            }
        }
    }

    // update or create entry for all registries in allowed_insecure
    oci_opts.allowed_insecure.into_iter().for_each(|reg| {
        match registry_config.entry(reg.clone()) {
            Entry::Occupied(mut entry) => {
                debug!(oci_registry_url = %reg, "set allowed_insecure");
                entry.get_mut().allow_insecure = true;
            }
            Entry::Vacant(entry) => {
                debug!(oci_registry_url = %reg, "set allowed_insecure");
                entry.insert(RegistryConfig {
                    reg_type: RegistryType::Oci,
                    allow_insecure: true,
                    ..Default::default()
                });
            }
        }
    });

    // update allow_latest for all registries
    registry_config.iter_mut().for_each(|(url, config)| {
        if !additional_ca_paths.is_empty() {
            config.additional_ca_paths.clone_from(&additional_ca_paths);
        }
        if allow_latest {
            debug!(oci_registry_url = %url, "set allow_latest");
        }
        config.allow_latest = allow_latest;
    });
}

impl Host {
    const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

    const NAME_ADJECTIVES: &'static str = "
    autumn hidden bitter misty silent empty dry dark summer
    icy delicate quiet white cool spring winter patient
    twilight dawn crimson wispy weathered blue billowing
    broken cold damp falling frosty green long late lingering
    bold little morning muddy old red rough still small
    sparkling bouncing shy wandering withered wild black
    young holy solitary fragrant aged snowy proud floral
    restless divine polished ancient purple lively nameless
    gray orange mauve
    ";

    const NAME_NOUNS: &'static str = "
    waterfall river breeze moon rain wind sea morning
    snow lake sunset pine shadow leaf dawn glitter forest
    hill cloud meadow sun glade bird brook butterfly
    bush dew dust field fire flower firefly ladybug feather grass
    haze mountain night pond darkness snowflake silence
    sound sky shape stapler surf thunder violet water wildflower
    wave water resonance sun timber dream cherry tree fog autocorrect
    frost voice paper frog smoke star hamster ocean emoji robot
    ";

    /// Generate a friendly name for the host based on a random number.
    /// Names we pulled from a list of friendly or neutral adjectives and nouns suitable for use in
    /// public and on hosts/domain names
    fn generate_friendly_name() -> Option<String> {
        let adjectives: Vec<_> = Self::NAME_ADJECTIVES.split_whitespace().collect();
        let nouns: Vec<_> = Self::NAME_NOUNS.split_whitespace().collect();
        names::Generator::new(&adjectives, &nouns, names::Name::Numbered).next()
    }

    /// Construct a new [Host] returning a tuple of its [Arc] and an async shutdown function.
    #[instrument(level = "debug", skip_all)]
    pub async fn new(
        config: HostConfig,
    ) -> anyhow::Result<(Arc<Self>, impl Future<Output = anyhow::Result<()>>)> {
        let host_key = if let Some(host_key) = &config.host_key {
            ensure!(host_key.key_pair_type() == KeyPairType::Server);
            Arc::clone(host_key)
        } else {
            Arc::new(KeyPair::new(KeyPairType::Server))
        };

        let mut labels = HashMap::from([
            ("hostcore.arch".into(), ARCH.into()),
            ("hostcore.os".into(), OS.into()),
            ("hostcore.osfamily".into(), FAMILY.into()),
        ]);
        labels.extend(config.labels.clone().into_iter());
        let friendly_name =
            Self::generate_friendly_name().context("failed to generate friendly name")?;

        let host_issuer = Arc::new(KeyPair::new_account());
        let claims = jwt::Claims::<jwt::Host>::new(
            friendly_name.clone(),
            host_issuer.public_key(),
            host_key.public_key().clone(),
            Some(HashMap::from_iter([(
                "self_signed".to_string(),
                "true".to_string(),
            )])),
        );
        let jwt = claims
            .encode(&host_issuer)
            .context("failed to encode host claims")?;
        let host_token = Arc::new(jwt::Token { jwt, claims });

        let start_evt = json!({
            "friendly_name": friendly_name,
            "labels": labels,
            "uptime_seconds": 0,
            "version": config.version,
        });

        let ((ctl_nats, queue), rpc_nats) = try_join!(
            async {
                debug!(
                    ctl_nats_url = config.ctl_nats_url.as_str(),
                    "connecting to NATS control server"
                );
                let ctl_nats = connect_nats(
                    config.ctl_nats_url.as_str(),
                    config.ctl_jwt.as_ref(),
                    config.ctl_key.clone(),
                    config.ctl_tls,
                    None,
                )
                .await
                .context("failed to establish NATS control server connection")?;
                let queue = Queue::new(
                    &ctl_nats,
                    &config.ctl_topic_prefix,
                    &config.lattice,
                    &host_key,
                )
                .await
                .context("failed to initialize queue")?;
                ctl_nats.flush().await.context("failed to flush")?;
                Ok((ctl_nats, queue))
            },
            async {
                debug!(
                    rpc_nats_url = config.rpc_nats_url.as_str(),
                    "connecting to NATS RPC server"
                );
                connect_nats(
                    config.rpc_nats_url.as_str(),
                    config.rpc_jwt.as_ref(),
                    config.rpc_key.clone(),
                    config.rpc_tls,
                    Some(config.rpc_timeout),
                )
                .await
                .context("failed to establish NATS RPC server connection")
            }
        )?;

        let start_at = Instant::now();

        let heartbeat_start_at = start_at
            .checked_add(Self::HEARTBEAT_INTERVAL)
            .context("failed to compute heartbeat start time")?;
        let heartbeat =
            IntervalStream::new(interval_at(heartbeat_start_at, Self::HEARTBEAT_INTERVAL));

        let (stop_tx, stop_rx) = watch::channel(None);

        let (runtime, epoch, epoch_end) = Runtime::builder()
            .max_execution_time(config.max_execution_time)
            .build()
            .context("failed to build runtime")?;
        let event_builder = EventBuilderV10::new().source(host_key.public_key());

        let ctl_jetstream = if let Some(domain) = config.js_domain.as_ref() {
            async_nats::jetstream::with_domain(ctl_nats.clone(), domain)
        } else {
            async_nats::jetstream::new(ctl_nats.clone())
        };
        let bucket = format!("LATTICEDATA_{}", config.lattice);
        let data = create_bucket(&ctl_jetstream, &bucket).await?;

        let config_bucket = format!("CONFIGDATA_{}", config.lattice);
        let config_data = create_bucket(&ctl_jetstream, &config_bucket).await?;

        let (queue_abort, queue_abort_reg) = AbortHandle::new_pair();
        let (heartbeat_abort, heartbeat_abort_reg) = AbortHandle::new_pair();
        let (data_watch_abort, data_watch_abort_reg) = AbortHandle::new_pair();

        let supplemental_config = if config.config_service_enabled {
            load_supplemental_config(&ctl_nats, &config.lattice, &labels).await?
        } else {
            SupplementalConfig::default()
        };

        let registry_config = RwLock::new(supplemental_config.registry_config.unwrap_or_default());
        merge_registry_config(&registry_config, config.oci_opts.clone()).await;

        let policy_manager = PolicyManager::new(
            ctl_nats.clone(),
            PolicyHostInfo {
                public_key: host_key.public_key(),
                lattice: config.lattice.to_string(),
                labels: labels.clone(),
            },
            config.policy_service_config.policy_topic.clone(),
            config.policy_service_config.policy_timeout_ms,
            config.policy_service_config.policy_changes_topic.clone(),
        )
        .await?;

        // If provided, secrets topic must be non-empty
        // TODO(#2411): Validate secrets topic prefix as a valid NATS subject
        ensure!(
            config.secrets_topic_prefix.is_none()
                || config
                    .secrets_topic_prefix
                    .as_ref()
                    .is_some_and(|topic| !topic.is_empty()),
            "secrets topic prefix must be non-empty"
        );

        let secrets_manager = Arc::new(SecretsManager::new(
            &config_data,
            config.secrets_topic_prefix.as_ref(),
            &ctl_nats,
        ));

        let meter = global::meter_with_version(
            "wasmcloud-host",
            Some(config.version.clone()),
            None::<&str>,
            Some(vec![
                KeyValue::new("host.id", host_key.public_key()),
                KeyValue::new("host.version", config.version.clone()),
            ]),
        );
        let metrics = HostMetrics::new(&meter, host_key.public_key(), config.lattice.to_string());

        let config_generator = BundleGenerator::new(config_data.clone());

        let max_execution_time_ms = config.max_execution_time;

        let host = Host {
            components: RwLock::default(),
            event_builder,
            friendly_name,
            heartbeat: heartbeat_abort.clone(),
            ctl_topic_prefix: config.ctl_topic_prefix.clone(),
            host_key,
            host_token,
            secrets_xkey: Arc::new(XKey::new()),
            labels: RwLock::new(labels),
            ctl_nats,
            rpc_nats: Arc::new(rpc_nats),
            host_config: config,
            data: data.clone(),
            data_watch: data_watch_abort.clone(),
            config_data: config_data.clone(),
            config_generator,
            policy_manager,
            secrets_manager,
            providers: RwLock::default(),
            registry_config,
            runtime,
            start_at,
            stop_rx,
            stop_tx,
            queue: queue_abort.clone(),
            links: RwLock::default(),
            component_claims: Arc::default(),
            provider_claims: Arc::default(),
            metrics: Arc::new(metrics),
            max_execution_time: max_execution_time_ms,
        };

        let host = Arc::new(host);
        let queue = spawn({
            let host = Arc::clone(&host);
            async move {
                let mut queue = Abortable::new(queue, queue_abort_reg);
                queue
                    .by_ref()
                    .for_each_concurrent(None, {
                        let host = Arc::clone(&host);
                        move |msg| {
                            let host = Arc::clone(&host);
                            async move { host.handle_ctl_message(msg).await }
                        }
                    })
                    .await;
                let deadline = { *host.stop_rx.borrow() };
                host.stop_tx.send_replace(deadline);
                if queue.is_aborted() {
                    info!("control interface queue task gracefully stopped");
                } else {
                    error!("control interface queue task unexpectedly stopped");
                }
            }
        });

        let data_watch: JoinHandle<anyhow::Result<_>> = spawn({
            let data = data.clone();
            let host = Arc::clone(&host);
            async move {
                let data_watch = data
                    .watch_all()
                    .await
                    .context("failed to watch lattice data bucket")?;
                let mut data_watch = Abortable::new(data_watch, data_watch_abort_reg);
                data_watch
                    .by_ref()
                    .for_each({
                        let host = Arc::clone(&host);
                        move |entry| {
                            let host = Arc::clone(&host);
                            async move {
                                match entry {
                                    Err(error) => {
                                        error!("failed to watch lattice data bucket: {error}");
                                    }
                                    Ok(entry) => host.process_entry(entry, true).await,
                                }
                            }
                        }
                    })
                    .await;
                let deadline = { *host.stop_rx.borrow() };
                host.stop_tx.send_replace(deadline);
                if data_watch.is_aborted() {
                    info!("data watch task gracefully stopped");
                } else {
                    error!("data watch task unexpectedly stopped");
                }
                Ok(())
            }
        });

        let heartbeat = spawn({
            let host = Arc::clone(&host);
            async move {
                let mut heartbeat = Abortable::new(heartbeat, heartbeat_abort_reg);
                heartbeat
                    .by_ref()
                    .for_each({
                        let host = Arc::clone(&host);
                        move |_| {
                            let host = Arc::clone(&host);
                            async move {
                                let heartbeat = match host.heartbeat().await {
                                    Ok(heartbeat) => heartbeat,
                                    Err(e) => {
                                        error!("failed to generate heartbeat: {e}");
                                        return;
                                    }
                                };

                                if let Err(e) =
                                    host.publish_event("host_heartbeat", heartbeat).await
                                {
                                    error!("failed to publish heartbeat: {e}");
                                }
                            }
                        }
                    })
                    .await;
                let deadline = { *host.stop_rx.borrow() };
                host.stop_tx.send_replace(deadline);
                if heartbeat.is_aborted() {
                    info!("heartbeat task gracefully stopped");
                } else {
                    error!("heartbeat task unexpectedly stopped");
                }
            }
        });

        // Process existing data without emitting events
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
                    Ok(entry) => host.process_entry(entry, false).await,
                    Err(err) => error!(%err, "failed to read entry from lattice data bucket"),
                }
            })
            .await;

        host.publish_event("host_started", start_evt)
            .await
            .context("failed to publish start event")?;
        info!(
            host_id = host.host_key.public_key(),
            "wasmCloud host started"
        );

        Ok((Arc::clone(&host), async move {
            heartbeat_abort.abort();
            queue_abort.abort();
            data_watch_abort.abort();
            host.policy_manager.policy_changes.abort();
            let _ = try_join!(queue, data_watch, heartbeat).context("failed to await tasks")?;
            host.publish_event(
                "host_stopped",
                json!({
                    "labels": *host.labels.read().await,
                }),
            )
            .await
            .context("failed to publish stop event")?;
            // Before we exit, make sure to flush all messages or we may lose some that we've
            // thought were sent (like the host_stopped event)
            try_join!(host.ctl_nats.flush(), host.rpc_nats.flush(),)
                .context("failed to flush NATS clients")?;
            let deadline = host.stop_rx.borrow().unwrap_or_else(|| {
                let now = Instant::now();
                // epoch ticks operate on a second precision
                now.checked_add(Duration::from_secs(1)).unwrap_or(now)
            });
            // NOTE: Epoch interrupt thread will only stop once there are no more references to the engine
            drop(host);
            match timeout_at(deadline, epoch_end).await {
                Err(_) => bail!("epoch interrupt thread timed out"),
                Ok(Err(_)) => bail!("epoch interrupt end receiver dropped"),
                Ok(Ok(())) => {
                    if let Err(_err) = epoch.join() {
                        bail!("epoch interrupt thread panicked")
                    }
                }
            }
            Ok(())
        }))
    }

    /// Waits for host to be stopped via lattice commands and returns the shutdown deadline on
    /// success
    ///
    /// # Errors
    ///
    /// Returns an error if internal stop channel is closed prematurely
    #[instrument(level = "debug", skip_all)]
    pub async fn stopped(&self) -> anyhow::Result<Option<Instant>> {
        self.stop_rx
            .clone()
            .changed()
            .await
            .context("failed to wait for stop")?;
        Ok(*self.stop_rx.borrow())
    }

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn inventory(&self) -> HostInventory {
        trace!("generating host inventory");
        let components = self.components.read().await;
        let components: Vec<_> = stream::iter(components.iter())
            .filter_map(|(id, component)| async move {
                let name = component
                    .claims()
                    .and_then(|claims| claims.metadata.as_ref())
                    .and_then(|metadata| metadata.name.as_ref())
                    .cloned();
                Some(ComponentDescription {
                    id: id.into(),
                    image_ref: component.image_reference.to_string(),
                    annotations: Some(component.annotations.clone().into_iter().collect()),
                    max_instances: component.max_instances.get().try_into().unwrap_or(u32::MAX),
                    revision: component
                        .claims()
                        .and_then(|claims| claims.metadata.as_ref())
                        .and_then(|jwt::Component { rev, .. }| *rev)
                        .unwrap_or_default(),
                    name,
                })
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
                    let name = claims_token
                        .as_ref()
                        .and_then(|claims| claims.claims.metadata.as_ref())
                        .and_then(|metadata| metadata.name.as_ref())
                        .cloned();
                    let annotations = Some(annotations.clone().into_iter().collect());
                    ProviderDescription {
                        id: provider_id.into(),
                        image_ref: Some(image_ref.clone()),
                        name: name.clone(),
                        annotations,
                        revision: claims_token
                            .as_ref()
                            .and_then(|claims| claims.claims.metadata.as_ref())
                            .and_then(|jwt::CapabilityProvider { rev, .. }| *rev)
                            .unwrap_or_default(),
                    }
                },
            )
            .collect();
        let uptime = self.start_at.elapsed();
        HostInventory {
            components,
            providers,
            friendly_name: self.friendly_name.clone(),
            labels: self.labels.read().await.clone(),
            uptime_human: human_friendly_uptime(uptime),
            uptime_seconds: uptime.as_secs(),
            version: self.host_config.version.clone(),
            host_id: self.host_key.public_key(),
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn heartbeat(&self) -> anyhow::Result<serde_json::Value> {
        trace!("generating heartbeat");
        Ok(serde_json::to_value(self.inventory().await)?)
    }

    #[instrument(level = "debug", skip(self))]
    async fn publish_event(&self, name: &str, data: serde_json::Value) -> anyhow::Result<()> {
        event::publish(
            &self.event_builder,
            &self.ctl_nats,
            &self.host_config.lattice,
            name,
            data,
        )
        .await
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

        let (events_tx, mut events_rx) = mpsc::channel(256);
        let prefix = Arc::from(format!("{}.{id}", &self.host_config.lattice));
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
        let metrics = Arc::clone(&self.metrics);
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
                                                let permit = permits.acquire_owned().await;
                                                tasks.spawn(async move {
                                                    let _permit = permit;
                                                    fut.await
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
                                        metrics.record_component_invocation(
                                            u64::try_from(start_at.elapsed().as_nanos())
                                                .unwrap_or_default(),
                                            attributes,
                                            !success,
                                        );
                                    }
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

    #[instrument(level = "trace", skip_all)]
    async fn store_component_claims(
        &self,
        claims: jwt::Claims<jwt::Component>,
    ) -> anyhow::Result<()> {
        let mut component_claims = self.component_claims.write().await;
        component_claims.insert(claims.subject.clone(), claims);
        Ok(())
    }

    // TODO: Remove this before wasmCloud 1.2 is released. This is a backwards-compatible
    // provider link definition put that is published to the provider's id, which is what
    // providers built for wasmCloud 1.0 expected.
    //
    // Thankfully, in a lattice where there are no "older" providers running, these publishes
    // will return immediately as there will be no subscribers on those topics.
    async fn put_backwards_compat_provider_link(
        &self,
        link: &InterfaceLinkDefinition,
    ) -> anyhow::Result<()> {
        // Only attempt to publish the backwards-compatible provider link definition if the link
        // does not contain any secret values.
        let source_config_contains_secret = link
            .source_config
            .iter()
            .any(|c| c.starts_with(SECRET_PREFIX));
        let target_config_contains_secret = link
            .target_config
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
        let lattice = &self.host_config.lattice;
        let payload: Bytes = serde_json::to_vec(&provider_link)
            .context("failed to serialize provider link definition")?
            .into();

        if let Err(e) = self
            .rpc_nats
            .publish_with_headers(
                format!("wasmbus.rpc.{lattice}.{}.linkdefs.put", link.source_id),
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
                format!("wasmbus.rpc.{lattice}.{}.linkdefs.put", link.target),
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
    async fn put_provider_link(
        &self,
        provider: &Provider,
        link: &InterfaceLinkDefinition,
    ) -> anyhow::Result<()> {
        let provider_link = self
            .resolve_link_config(
                link.clone(),
                provider.claims_token.as_ref().map(|t| &t.jwt),
                provider.annotations.get("wasmcloud.dev/appspec"),
                &provider.xkey,
            )
            .await
            .context("failed to resolve link config and secrets")?;
        let lattice = &self.host_config.lattice;
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
    #[instrument(level = "debug", skip(self, payload))]
    async fn del_provider_link(
        &self,
        source_id: &str,
        target: Option<String>,
        payload: Bytes,
    ) -> anyhow::Result<()> {
        let lattice = &self.host_config.lattice;
        let source_provider = self
            .rpc_nats
            .publish_with_headers(
                format!("wasmbus.rpc.{lattice}.{source_id}.linkdefs.del"),
                injector_to_headers(&TraceContextInjector::default_with_span()),
                payload.clone(),
            )
            .await
            .context("failed to publish provider link definition del");
        if let Some(target) = target {
            self.rpc_nats
                .publish_with_headers(
                    format!("wasmbus.rpc.{lattice}.{target}.linkdefs.del"),
                    injector_to_headers(&TraceContextInjector::default_with_span()),
                    payload,
                )
                .await
                .context("failed to publish provider link definition del")?;
        }

        source_provider?;
        Ok(())
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
                            if source_id == &spec_link.source_id || source_id == &spec_link.target {
                                Some(links)
                            } else {
                                None
                            }
                        })
                        .flatten()
                        .any(|host_link| spec_link == &host_link)
                })
                .collect::<Vec<_>>()
        };

        {
            // Acquire lock once in this block to avoid continually trying to acquire it.
            let providers = self.providers.read().await;
            // For every new link, if a provider is running on this host as the source or target,
            // send the link to the provider for handling based on the xkey public key.
            for link in new_links {
                if let Some(provider) = providers.get(&link.source_id) {
                    if let Err(e) = self.put_provider_link(provider, link).await {
                        error!(?e, "failed to put provider link");
                    }
                }
                if let Some(provider) = providers.get(&link.target) {
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

    /// Transform a [`wasmcloud_control_interface::InterfaceLinkDefinition`] into a [`wasmcloud_core::InterfaceLinkDefinition`]
    /// by fetching the source and target configurations and secrets, and encrypting the secrets.
    async fn resolve_link_config(
        &self,
        link: wasmcloud_control_interface::InterfaceLinkDefinition,
        provider_jwt: Option<&String>,
        application: Option<&String>,
        provider_xkey: &XKey,
    ) -> anyhow::Result<wasmcloud_core::InterfaceLinkDefinition> {
        let (source_bundle, raw_source_secrets) = self
            .fetch_config_and_secrets(&link.source_config, provider_jwt, application)
            .await?;
        let (target_bundle, raw_target_secrets) = self
            .fetch_config_and_secrets(&link.target_config, provider_jwt, application)
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
            source_id: link.source_id,
            target: link.target,
            name: link.name,
            wit_namespace: link.wit_namespace,
            wit_package: link.wit_package,
            interfaces: link.interfaces,
            source_config: source_config.clone(),
            target_config: target_config.clone(),
            source_secrets,
            target_secrets,
        })
    }
}

/// Helper function to transform a Vec of [`InterfaceLinkDefinition`]s into the structure components expect to be able
/// to quickly look up the desired target for a given interface
///
/// # Arguments
/// - links: A Vec of [`InterfaceLinkDefinition`]s
///
/// # Returns
/// - A `HashMap` in the form of `link_name` -> `instance` -> target
fn component_import_links(
    links: &[InterfaceLinkDefinition],
) -> HashMap<Box<str>, HashMap<Box<str>, Box<str>>> {
    let mut m = HashMap::new();
    for InterfaceLinkDefinition {
        name,
        target,
        wit_namespace,
        wit_package,
        interfaces,
        ..
    } in links
    {
        let instances: &mut HashMap<Box<str>, Box<str>> =
            m.entry(name.clone().into_boxed_str()).or_default();
        for interface in interfaces {
            instances.insert(
                format!("{wit_namespace}:{wit_package}/{interface}").into_boxed_str(),
                target.clone().into_boxed_str(),
            );
        }
    }
    m
}

pub(crate) fn human_friendly_uptime(uptime: Duration) -> String {
    // strip sub-seconds, then convert to human-friendly format
    humantime::format_duration(
        uptime.saturating_sub(Duration::from_nanos(uptime.subsec_nanos().into())),
    )
    .to_string()
}

fn injector_to_headers(injector: &TraceContextInjector) -> async_nats::header::HeaderMap {
    injector
        .iter()
        .filter_map(|(k, v)| {
            // There's not really anything we can do about headers that don't parse
            let name = async_nats::header::HeaderName::from_str(k.as_str()).ok()?;
            let value = async_nats::header::HeaderValue::from_str(v.as_str()).ok()?;
            Some((name, value))
        })
        .collect()
}

#[cfg(test)]
mod test {
    // Ensure that the helper function to translate a list of links into a map of imports works as expected
    #[test]
    fn can_compute_component_links() {
        use std::collections::HashMap;
        use wasmcloud_control_interface::InterfaceLinkDefinition;

        let links = vec![
            InterfaceLinkDefinition {
                source_id: "source_component".to_string(),
                target: "kv-redis".to_string(),
                wit_namespace: "wasi".to_string(),
                wit_package: "keyvalue".to_string(),
                interfaces: vec!["atomics".to_string(), "store".to_string()],
                name: "default".to_string(),
                source_config: vec![],
                target_config: vec![],
            },
            InterfaceLinkDefinition {
                source_id: "source_component".to_string(),
                target: "kv-vault".to_string(),
                wit_namespace: "wasi".to_string(),
                wit_package: "keyvalue".to_string(),
                interfaces: vec!["atomics".to_string(), "store".to_string()],
                name: "secret".to_string(),
                source_config: vec![],
                target_config: vec!["my-secret".to_string()],
            },
            InterfaceLinkDefinition {
                source_id: "source_component".to_string(),
                target: "kv-vault-offsite".to_string(),
                wit_namespace: "wasi".to_string(),
                wit_package: "keyvalue".to_string(),
                interfaces: vec!["atomics".to_string()],
                name: "secret".to_string(),
                source_config: vec![],
                target_config: vec!["my-secret".to_string()],
            },
            InterfaceLinkDefinition {
                source_id: "http".to_string(),
                target: "source_component".to_string(),
                wit_namespace: "wasi".to_string(),
                wit_package: "http".to_string(),
                interfaces: vec!["incoming-handler".to_string()],
                name: "default".to_string(),
                source_config: vec!["some-port".to_string()],
                target_config: vec![],
            },
            InterfaceLinkDefinition {
                source_id: "source_component".to_string(),
                target: "httpclient".to_string(),
                wit_namespace: "wasi".to_string(),
                wit_package: "http".to_string(),
                interfaces: vec!["outgoing-handler".to_string()],
                name: "default".to_string(),
                source_config: vec![],
                target_config: vec!["some-port".to_string()],
            },
            InterfaceLinkDefinition {
                source_id: "source_component".to_string(),
                target: "other_component".to_string(),
                wit_namespace: "custom".to_string(),
                wit_package: "foo".to_string(),
                interfaces: vec!["bar".to_string(), "baz".to_string()],
                name: "default".to_string(),
                source_config: vec![],
                target_config: vec![],
            },
            InterfaceLinkDefinition {
                source_id: "other_component".to_string(),
                target: "target".to_string(),
                wit_namespace: "wit".to_string(),
                wit_package: "package".to_string(),
                interfaces: vec!["interface3".to_string()],
                name: "link2".to_string(),
                source_config: vec![],
                target_config: vec![],
            },
        ];

        let links_map = super::component_import_links(&links);

        // Expected structure:
        // {
        //     "default": {
        //         "wasi:keyvalue": {
        //             "atomics": "kv-redis",
        //             "store": "kv-redis"
        //         },
        //         "wasi:http": {
        //             "incoming-handler": "source_component"
        //         },
        //         "custom:foo": {
        //             "bar": "other_component",
        //             "baz": "other_component"
        //         }
        //     },
        //     "secret": {
        //         "wasi:keyvalue": {
        //             "atomics": "kv-vault-offsite",
        //             "store": "kv-vault"
        //         }
        //     },
        //     "link2": {
        //         "wit:package": {
        //             "interface3": "target"
        //         }
        //     }
        // }
        let expected_result = HashMap::from_iter([
            (
                "default".into(),
                HashMap::from([
                    ("wasi:keyvalue/atomics".into(), "kv-redis".into()),
                    ("wasi:keyvalue/store".into(), "kv-redis".into()),
                    (
                        "wasi:http/incoming-handler".into(),
                        "source_component".into(),
                    ),
                    ("wasi:http/outgoing-handler".into(), "httpclient".into()),
                    ("custom:foo/bar".into(), "other_component".into()),
                    ("custom:foo/baz".into(), "other_component".into()),
                ]),
            ),
            (
                "secret".into(),
                HashMap::from([
                    ("wasi:keyvalue/atomics".into(), "kv-vault-offsite".into()),
                    ("wasi:keyvalue/store".into(), "kv-vault".into()),
                ]),
            ),
            (
                "link2".into(),
                HashMap::from([("wit:package/interface3".into(), "target".into())]),
            ),
        ]);

        assert_eq!(links_map, expected_result);
    }
}
