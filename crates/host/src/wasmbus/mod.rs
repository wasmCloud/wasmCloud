use std::collections::btree_map::Entry as BTreeMapEntry;
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
use async_nats::jetstream::kv::Store;
use bytes::{BufMut, BytesMut};
use cloudevents::{EventBuilder, EventBuilderV10};
use futures::future::Either;
use futures::stream::{AbortHandle, Abortable, SelectAll};
use futures::{try_join, Stream, StreamExt, TryFutureExt, TryStreamExt};
use nkeys::{KeyPair, KeyPairType, XKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::AsyncWrite;
use tokio::sync::{mpsc, watch, RwLock};
use tokio::task::JoinHandle;
use tokio::time::{interval_at, timeout_at, Instant};
use tokio::{select, spawn};
use tracing::{debug, error, info, instrument, trace, warn, Instrument as _};
use wascap::{jwt, prelude::ClaimsBuilder};
use wasmcloud_control_interface::{
    CtlResponse, HostInventory, HostLabel, Link, RegistryCredential, StopHostCommand,
};
use wasmcloud_core::CTL_API_VERSION_1;
use wasmcloud_runtime::Runtime;
use wasmcloud_tracing::context::TraceContextInjector;
use wasmcloud_tracing::{global, KeyValue};

use crate::registry::RegistryCredentialExt;
use crate::{
    fetch_component, HostMetrics, OciConfig, PolicyManager, PolicyResponse, RegistryAuth,
    RegistryConfig, RegistryType,
};

mod event;
mod handler;
mod lattice;

pub mod config;
/// wasmCloud host configuration
pub mod host_config;

pub use self::host_config::Host as HostConfig;

use self::config::ConfigBundle;
use self::handler::Handler;
use self::lattice::{Lattice, LatticeConfig};

const MAX_INVOCATION_CHANNEL_SIZE: usize = 5000;
const MIN_INVOCATION_CHANNEL_SIZE: usize = 256;

#[derive(Debug)]
struct Queue {
    all_streams: SelectAll<async_nats::Subscriber>,
}

impl Stream for Queue {
    type Item = async_nats::Message;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.all_streams.poll_next_unpin(cx)
    }
}

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

// TODO enum?!
impl Queue {
    #[instrument]
    async fn new_host(
        nats: &async_nats::Client,
        topic_prefix: &str,
        lattices: Arc<[Box<str>]>,
        host_id: &str,
    ) -> anyhow::Result<Self> {
        let mut subscriptions = Vec::new();
        for lattice in lattices.iter() {
            subscriptions.push(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.host.ping",
            )));
            subscriptions.push(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.label.*.{host_id}"
            )));
            subscriptions.push(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.host.*.{host_id}"
            )));
        }

        let streams = futures::future::join_all(subscriptions)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, async_nats::SubscribeError>>()
            .context("failed to subscribe to queues")?;
        Ok(Self {
            all_streams: futures::stream::select_all(streams),
        })
    }

    #[instrument]
    async fn new_lattice(
        nats: &async_nats::Client,
        topic_prefix: &str,
        lattice: &str,
        host_id: &str,
    ) -> anyhow::Result<Self> {
        let streams = futures::future::join_all([
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.registry.put",
            ))),
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.*.auction",
            ))),
            Either::Right(nats.queue_subscribe(
                format!("{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.link.*"),
                format!("{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.link",),
            )),
            Either::Right(nats.queue_subscribe(
                format!("{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.claims.get"),
                format!("{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.claims"),
            )),
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.component.*.{host_id}"
            ))),
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.provider.*.{host_id}"
            ))),
            Either::Right(nats.queue_subscribe(
                format!("{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.config.>"),
                format!("{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.config"),
            )),
        ])
        .await
        .into_iter()
        .collect::<Result<Vec<_>, async_nats::SubscribeError>>()
        .context("failed to subscribe to queues")?;
        Ok(Self {
            all_streams: futures::stream::select_all(streams),
        })
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
                                KeyValue::new("host", metrics.host_id.clone()),
                                KeyValue::new("operation", format!("{instance}/{func}")),
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

/// An Provider instance
#[derive(Debug)]
struct Provider {
    image_ref: String,
    claims_token: Option<jwt::Token<jwt::CapabilityProvider>>,
    xkey: XKey,
    annotations: Annotations,
    /// Task that continuously performs health checks against the provider
    health_check_task: JoinHandle<()>,
    /// Task that continuously forwards configuration updates to the provider
    config_update_task: JoinHandle<()>,
    /// Config bundle for the aggregated configuration being watched by the provider
    #[allow(unused)]
    config: Arc<RwLock<ConfigBundle>>,
}

impl Drop for Provider {
    fn drop(&mut self) {
        self.health_check_task.abort();
        self.config_update_task.abort();
    }
}

/// wasmCloud Host
pub struct Host {
    event_builder: EventBuilderV10,
    friendly_name: String,
    heartbeats: Vec<AbortHandle>,
    host_config: HostConfig,
    host_key: Arc<KeyPair>,
    #[allow(unused)]
    host_token: Arc<jwt::Token<jwt::Host>>,
    labels: Arc<RwLock<BTreeMap<String, String>>>,
    ctl_topic_prefix: String,
    /// NATS client to use for control interface subscriptions and jetstream queries
    ctl_nats: async_nats::Client,
    /// NATS client to use for RPC calls
    rpc_nats: Arc<async_nats::Client>,
    #[allow(unused)]
    runtime: Runtime,
    start_at: Instant,
    stop_tx: watch::Sender<Option<Instant>>,
    stop_rx: watch::Receiver<Option<Instant>>,
    queue: AbortHandle,
    #[allow(unused)]
    max_execution_time: Duration,
    /// Map of lattice names to their respective lattice configurations
    lattices: HashMap<Box<str>, Arc<Lattice>>,
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
    links: Vec<Link>,
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

#[allow(clippy::large_enum_variant)] // Without this clippy complains component is at least 0 bytes while provider is at least 280 bytes. That doesn't make sense
enum Claims {
    Component(jwt::Claims<jwt::Component>),
    Provider(jwt::Claims<jwt::CapabilityProvider>),
}

impl Claims {
    fn subject(&self) -> &str {
        match self {
            Claims::Component(claims) => &claims.subject,
            Claims::Provider(claims) => &claims.subject,
        }
    }
}

impl From<StoredClaims> for Claims {
    fn from(claims: StoredClaims) -> Self {
        match claims {
            StoredClaims::Component(claims) => {
                let name = (!claims.name.is_empty()).then_some(claims.name);
                let rev = claims.revision.parse().ok();
                let ver = (!claims.version.is_empty()).then_some(claims.version);
                let tags = (!claims.tags.is_empty()).then_some(claims.tags);
                let call_alias = (!claims.call_alias.is_empty()).then_some(claims.call_alias);
                let metadata = jwt::Component {
                    name,
                    tags,
                    rev,
                    ver,
                    call_alias,
                    ..Default::default()
                };
                let claims = ClaimsBuilder::new()
                    .subject(&claims.subject)
                    .issuer(&claims.issuer)
                    .with_metadata(metadata)
                    .build();
                Claims::Component(claims)
            }
            StoredClaims::Provider(claims) => {
                let name = (!claims.name.is_empty()).then_some(claims.name);
                let rev = claims.revision.parse().ok();
                let ver = (!claims.version.is_empty()).then_some(claims.version);
                let config_schema: Option<serde_json::Value> = claims
                    .config_schema
                    .and_then(|schema| serde_json::from_str(&schema).ok());
                let metadata = jwt::CapabilityProvider {
                    name,
                    rev,
                    ver,
                    config_schema,
                    ..Default::default()
                };
                let claims = ClaimsBuilder::new()
                    .subject(&claims.subject)
                    .issuer(&claims.issuer)
                    .with_metadata(metadata)
                    .build();
                Claims::Provider(claims)
            }
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
                entry.insert(
                    RegistryConfig::builder()
                        .reg_type(RegistryType::Oci)
                        .auth(RegistryAuth::from((
                            oci_opts.oci_user,
                            oci_opts.oci_password,
                        )))
                        .build()
                        .expect("failed to build registry config"),
                );
            }
        }
    }

    // update or create entry for all registries in allowed_insecure
    oci_opts.allowed_insecure.into_iter().for_each(|reg| {
        match registry_config.entry(reg.clone()) {
            Entry::Occupied(mut entry) => {
                debug!(oci_registry_url = %reg, "set allowed_insecure");
                entry.get_mut().set_allow_insecure(true);
            }
            Entry::Vacant(entry) => {
                debug!(oci_registry_url = %reg, "set allowed_insecure");
                entry.insert(
                    RegistryConfig::builder()
                        .reg_type(RegistryType::Oci)
                        .allow_insecure(true)
                        .build()
                        .expect("failed to build registry config"),
                );
            }
        }
    });

    // update allow_latest for all registries
    registry_config.iter_mut().for_each(|(url, config)| {
        if !additional_ca_paths.is_empty() {
            config.set_additional_ca_paths(additional_ca_paths.clone());
        }
        if allow_latest {
            debug!(oci_registry_url = %url, "set allow_latest");
        }
        config.set_allow_latest(allow_latest);
    });
}

impl Host {
    const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

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

        let mut labels = BTreeMap::from([
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
                let queue = Queue::new_host(
                    &ctl_nats,
                    &config.ctl_topic_prefix,
                    config.lattices.clone(),
                    &host_key.public_key(),
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

        let heartbeat_interval = config
            .heartbeat_interval
            .unwrap_or(Self::DEFAULT_HEARTBEAT_INTERVAL);
        let heartbeat_start_at = start_at
            .checked_add(heartbeat_interval)
            .context("failed to compute heartbeat start time")?;

        let (stop_tx, stop_rx) = watch::channel(None);

        let (runtime, epoch, epoch_end) = Runtime::builder()
            .max_execution_time(config.max_execution_time)
            .max_linear_memory(config.max_linear_memory)
            .max_components(config.max_components)
            .max_component_size(config.max_component_size)
            .build()
            .context("failed to build runtime")?;
        let event_builder = EventBuilderV10::new().source(host_key.public_key());

        let (queue_abort, queue_abort_reg) = AbortHandle::new_pair();

        let meter = global::meter_with_version(
            "wasmcloud-host",
            Some(config.version.clone()),
            None::<&str>,
            Some(vec![
                KeyValue::new("host.id", host_key.public_key()),
                KeyValue::new("host.version", config.version.clone()),
            ]),
        );
        let ctl_jetstream = if let Some(domain) = config.js_domain.as_ref() {
            async_nats::jetstream::with_domain(ctl_nats.clone(), domain)
        } else {
            async_nats::jetstream::new(ctl_nats.clone())
        };

        let labels = Arc::new(RwLock::new(labels));
        let metrics = Arc::new(HostMetrics::new(&meter, host_key.public_key()));
        let mut lattices = HashMap::new();
        let exec_time = config.max_execution_time;
        let (event_tx, mut event_rx) = mpsc::channel(100);
        for lattice_name in config.lattices.iter() {
            let heartbeat_interval = interval_at(heartbeat_start_at, heartbeat_interval);
            let cfg = LatticeConfig::from(config.clone());
            let lattice = Lattice::new(
                lattice_name.to_string(),
                rpc_nats.clone(),
                ctl_nats.clone(),
                ctl_jetstream.clone(),
                host_key.public_key(),
                labels.clone(),
                metrics.clone(),
                exec_time,
                runtime.clone(),
                event_tx.clone(),
                cfg,
                heartbeat_interval,
                (*host_token).clone(),
            )
            .await
            .context("failed to initialize lattice")?;
            lattices.insert(lattice_name.clone(), lattice);
        }

        let max_execution_time_ms = config.max_execution_time;

        let host = Host {
            event_builder,
            friendly_name,
            heartbeats: Vec::new(),
            ctl_topic_prefix: config.ctl_topic_prefix.clone(),
            host_key,
            host_token,
            labels,
            ctl_nats,
            rpc_nats: Arc::new(rpc_nats),
            host_config: config,
            runtime,
            start_at,
            stop_rx,
            stop_tx,
            queue: queue_abort.clone(),
            max_execution_time: max_execution_time_ms,
            lattices,
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

        let event_relay = spawn({
            let host = Arc::clone(&host);
            async move {
                loop {
                    select! {
                        Some((lattice, name, payload)) = event_rx.recv() => {
                            host.publish_event(&name, &lattice, payload).await.map_err(|e| error!(%e, "failed to publish event")).ok();
                        }
                            // TODO graceful shutdown?
                        _ = host.stopped() => {
                            break;
                        }
                    }
                }
            }
        });

        let lattice_names = host.lattices.keys().cloned().collect::<Vec<_>>();
        for lattice in lattice_names.clone() {
            host.publish_event("host_started", &lattice, start_evt.clone())
                .await
                .context("failed to publish start event")?;
        }
        info!(
            host_id = host.host_key.public_key(),
            "wasmCloud host started"
        );

        Ok((Arc::clone(&host), async move {
            queue_abort.abort();
            let _ = try_join!(queue, event_relay).context("failed to await tasks")?;
            for lattice in lattice_names {
                let deadline = host.stop_rx.borrow().unwrap_or_else(|| {
                    let now = Instant::now();
                    // epoch ticks operate on a second precision
                    now.checked_add(Duration::from_secs(1)).unwrap_or(now)
                });
                host.lattices.values().for_each(|lattice| {
                    lattice.stop_tx.send_replace(Some(deadline));
                });
                host.publish_event("host_stopped", &lattice, json!({}))
                    .await
                    .context("failed to publish stop event")?;
            }
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
    async fn inventory(&self, lattice: &str) -> anyhow::Result<HostInventory> {
        trace!(lattice = lattice, "generating host inventory");

        let lattice = self.lattices.get(lattice).context("lattice not found")?;
        let (components, providers) = lattice.inventory().await;
        let uptime = self.start_at.elapsed();
        Ok(HostInventory::builder()
            .components(components)
            .providers(providers)
            .friendly_name(self.friendly_name.clone())
            .labels(self.labels.read().await.clone())
            .uptime_human(human_friendly_uptime(uptime))
            .uptime_seconds(uptime.as_secs())
            .version(self.host_config.version.clone())
            .host_id(self.host_key.public_key())
            .build()
            .expect("failed to build host inventory"))
    }

    #[instrument(level = "debug", skip_all)]
    async fn heartbeat(&self, lattice: &str) -> anyhow::Result<serde_json::Value> {
        trace!("generating heartbeat");
        Ok(serde_json::to_value(self.inventory(lattice).await?)?)
    }

    #[instrument(level = "debug", skip(self))]
    async fn publish_event(
        &self,
        name: &str,
        lattice: &str,
        data: serde_json::Value,
    ) -> anyhow::Result<()> {
        event::publish(&self.event_builder, &self.ctl_nats, lattice, name, data).await
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_stop_host(
        &self,
        payload: impl AsRef<[u8]>,
        transport_host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        // Allow an empty payload to be used for stopping hosts
        let timeout = if payload.as_ref().is_empty() {
            None
        } else {
            let cmd = serde_json::from_slice::<StopHostCommand>(payload.as_ref())
                .context("failed to deserialize stop command")?;
            let timeout = cmd.timeout();
            let host_id = cmd.host_id();

            // If the Host ID was provided (i..e not the empty string, due to #[serde(default)]), then
            // we should check it against the known transport-provided host_id, and this actual host's ID
            if !host_id.is_empty() {
                anyhow::ensure!(
                    host_id == transport_host_id && host_id == self.host_key.public_key(),
                    "invalid host_id [{host_id}]"
                );
            }
            timeout
        };

        // It *should* be impossible for the transport-derived host ID to not match at this point
        anyhow::ensure!(
            transport_host_id == self.host_key.public_key(),
            "invalid host_id [{transport_host_id}]"
        );

        info!(?timeout, "handling stop host");

        self.heartbeats.iter().for_each(|abort| abort.abort());
        self.queue.abort();
        let deadline =
            timeout.and_then(|timeout| Instant::now().checked_add(Duration::from_millis(timeout)));
        self.stop_tx.send_replace(deadline);
        Ok(CtlResponse::<()>::success(
            "successfully handled stop host".into(),
        ))
    }

    async fn handle_inventory(&self, lattice: &str) -> anyhow::Result<CtlResponse<HostInventory>> {
        trace!("handling inventory");
        let inventory = self.inventory(lattice).await?;
        Ok(CtlResponse::ok(inventory))
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_label_put(
        &self,
        host_id: &str,
        lattice: &str,
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
            lattice,
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
        lattice: &str,
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
            lattice,
            event::labels_changed(host_id, HashMap::from_iter(labels.clone())),
        )
        .await
        .context("failed to publish labels_changed event")?;

        Ok(CtlResponse::<()>::success(
            "successfully deleted label".into(),
        ))
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_ping_hosts(
        &self,
        lattice: &str,
        _payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<CtlResponse<wasmcloud_control_interface::Host>> {
        trace!("replying to ping");
        let uptime = self.start_at.elapsed();

        let mut host = wasmcloud_control_interface::Host::builder()
            .id(self.host_key.public_key())
            .labels(self.labels.read().await.clone())
            .friendly_name(self.friendly_name.clone())
            .uptime_seconds(uptime.as_secs())
            .uptime_human(human_friendly_uptime(uptime))
            .version(self.host_config.version.clone())
            .ctl_host(self.host_config.ctl_nats_url.to_string())
            .rpc_host(self.host_config.rpc_nats_url.to_string())
            .lattice(lattice.to_string());

        if let Some(ref js_domain) = self.host_config.js_domain {
            host = host.js_domain(js_domain.clone());
        }

        let host = host
            .build()
            .map_err(|e| anyhow!("failed to build host message: {e}"))?;

        Ok(CtlResponse::ok(host))
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
            .skip(1);
        trace!(%subject, "handling control interface request");

        // This response is a wrapped Result<Option<Result<Vec<u8>>>> for a good reason.
        // The outer Result is for reporting protocol errors in handling the request, e.g. failing to
        //    deserialize the request payload.
        // The Option is for the case where the request is handled successfully, but the handler
        //    doesn't want to send a response back to the client, like with an auction.
        // The inner Result is purely for the success or failure of serializing the [CtlResponse], which
        //    should never fail but it's a result we must handle.
        // And finally, the Vec<u8> is the serialized [CtlResponse] that we'll send back to the client
        let ctl_response = match (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) {
            // Host commands
            (Some(lattice), Some("host"), Some("get"), Some(_host_id), None) => self
                .handle_inventory(lattice)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some(lattice), Some("host"), Some("ping"), None, None) => self
                .handle_ping_hosts(lattice, message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some(_lattice), Some("host"), Some("stop"), Some(host_id), None) => self
                .handle_stop_host(message.payload, host_id)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Label commands
            (Some(lattice), Some("label"), Some("del"), Some(host_id), None) => self
                .handle_label_del(host_id, lattice, message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some(lattice), Some("label"), Some("put"), Some(host_id), None) => self
                .handle_label_put(host_id, lattice, message.payload)
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
}

/// Helper function to transform a Vec of [`Link`]s into the structure components expect to be able
/// to quickly look up the desired target for a given interface
///
/// # Arguments
/// - links: A Vec of [`Link`]s
///
/// # Returns
/// - A `HashMap` in the form of `link_name` -> `instance` -> target
fn component_import_links(links: &[Link]) -> HashMap<Box<str>, HashMap<Box<str>, Box<str>>> {
    let mut m = HashMap::new();
    for link in links {
        let instances: &mut HashMap<Box<str>, Box<str>> = m
            .entry(link.name().to_string().into_boxed_str())
            .or_default();
        for interface in link.interfaces() {
            instances.insert(
                format!(
                    "{}:{}/{interface}",
                    link.wit_namespace(),
                    link.wit_package(),
                )
                .into_boxed_str(),
                link.target().to_string().into_boxed_str(),
            );
        }
    }
    m
}

/// Helper function to serialize `CtlResponse`<T> into a Vec<u8> if the response is Some
fn serialize_ctl_response<T: Serialize>(
    ctl_response: Option<CtlResponse<T>>,
) -> Option<anyhow::Result<Vec<u8>>> {
    ctl_response.map(|resp| serde_json::to_vec(&resp).map_err(anyhow::Error::from))
}

// TODO: remove StoredClaims in #1093
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum StoredClaims {
    Component(StoredComponentClaims),
    Provider(StoredProviderClaims),
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoredComponentClaims {
    call_alias: String,
    #[serde(alias = "iss")]
    issuer: String,
    name: String,
    #[serde(alias = "rev")]
    revision: String,
    #[serde(alias = "sub")]
    subject: String,
    #[serde(deserialize_with = "deserialize_messy_vec")]
    tags: Vec<String>,
    version: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoredProviderClaims {
    #[serde(alias = "iss")]
    issuer: String,
    name: String,
    #[serde(alias = "rev")]
    revision: String,
    #[serde(alias = "sub")]
    subject: String,
    version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    config_schema: Option<String>,
}

impl TryFrom<Claims> for StoredClaims {
    type Error = anyhow::Error;

    fn try_from(claims: Claims) -> Result<Self, Self::Error> {
        match claims {
            Claims::Component(jwt::Claims {
                issuer,
                subject,
                metadata,
                ..
            }) => {
                let jwt::Component {
                    name,
                    tags,
                    rev,
                    ver,
                    call_alias,
                    ..
                } = metadata.context("no metadata found on component claims")?;
                Ok(StoredClaims::Component(StoredComponentClaims {
                    call_alias: call_alias.unwrap_or_default(),
                    issuer,
                    name: name.unwrap_or_default(),
                    revision: rev.unwrap_or_default().to_string(),
                    subject,
                    tags: tags.unwrap_or_default(),
                    version: ver.unwrap_or_default(),
                }))
            }
            Claims::Provider(jwt::Claims {
                issuer,
                subject,
                metadata,
                ..
            }) => {
                let jwt::CapabilityProvider {
                    name,
                    rev,
                    ver,
                    config_schema,
                    ..
                } = metadata.context("no metadata found on provider claims")?;
                Ok(StoredClaims::Provider(StoredProviderClaims {
                    issuer,
                    name: name.unwrap_or_default(),
                    revision: rev.unwrap_or_default().to_string(),
                    subject,
                    version: ver.unwrap_or_default(),
                    config_schema: config_schema.map(|schema| schema.to_string()),
                }))
            }
        }
    }
}

impl TryFrom<&Claims> for StoredClaims {
    type Error = anyhow::Error;

    fn try_from(claims: &Claims) -> Result<Self, Self::Error> {
        match claims {
            Claims::Component(jwt::Claims {
                issuer,
                subject,
                metadata,
                ..
            }) => {
                let jwt::Component {
                    name,
                    tags,
                    rev,
                    ver,
                    call_alias,
                    ..
                } = metadata
                    .as_ref()
                    .context("no metadata found on component claims")?;
                Ok(StoredClaims::Component(StoredComponentClaims {
                    call_alias: call_alias.clone().unwrap_or_default(),
                    issuer: issuer.clone(),
                    name: name.clone().unwrap_or_default(),
                    revision: rev.unwrap_or_default().to_string(),
                    subject: subject.clone(),
                    tags: tags.clone().unwrap_or_default(),
                    version: ver.clone().unwrap_or_default(),
                }))
            }
            Claims::Provider(jwt::Claims {
                issuer,
                subject,
                metadata,
                ..
            }) => {
                let jwt::CapabilityProvider {
                    name,
                    rev,
                    ver,
                    config_schema,
                    ..
                } = metadata
                    .as_ref()
                    .context("no metadata found on provider claims")?;
                Ok(StoredClaims::Provider(StoredProviderClaims {
                    issuer: issuer.clone(),
                    name: name.clone().unwrap_or_default(),
                    revision: rev.unwrap_or_default().to_string(),
                    subject: subject.clone(),
                    version: ver.clone().unwrap_or_default(),
                    config_schema: config_schema.as_ref().map(ToString::to_string),
                }))
            }
        }
    }
}

#[allow(clippy::implicit_hasher)]
impl From<StoredClaims> for HashMap<String, String> {
    fn from(claims: StoredClaims) -> Self {
        match claims {
            StoredClaims::Component(claims) => HashMap::from([
                ("call_alias".to_string(), claims.call_alias),
                ("iss".to_string(), claims.issuer.clone()), // TODO: remove in #1093
                ("issuer".to_string(), claims.issuer),
                ("name".to_string(), claims.name),
                ("rev".to_string(), claims.revision.clone()), // TODO: remove in #1093
                ("revision".to_string(), claims.revision),
                ("sub".to_string(), claims.subject.clone()), // TODO: remove in #1093
                ("subject".to_string(), claims.subject),
                ("tags".to_string(), claims.tags.join(",")),
                ("version".to_string(), claims.version),
            ]),
            StoredClaims::Provider(claims) => HashMap::from([
                ("iss".to_string(), claims.issuer.clone()), // TODO: remove in #1093
                ("issuer".to_string(), claims.issuer),
                ("name".to_string(), claims.name),
                ("rev".to_string(), claims.revision.clone()), // TODO: remove in #1093
                ("revision".to_string(), claims.revision),
                ("sub".to_string(), claims.subject.clone()), // TODO: remove in #1093
                ("subject".to_string(), claims.subject),
                ("version".to_string(), claims.version),
                (
                    "config_schema".to_string(),
                    claims.config_schema.unwrap_or_default(),
                ),
            ]),
        }
    }
}

fn deserialize_messy_vec<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<String>, D::Error> {
    MessyVec::deserialize(deserializer).map(|messy_vec| messy_vec.0)
}
// Helper struct to deserialize either a comma-delimited string or an actual array of strings
struct MessyVec(pub Vec<String>);

struct MessyVecVisitor;

// Since this is "temporary" code to preserve backwards compatibility with already-serialized claims,
// we use fully-qualified names instead of importing
impl<'de> serde::de::Visitor<'de> for MessyVecVisitor {
    type Value = MessyVec;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("string or array of strings")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut values = Vec::new();

        while let Some(value) = seq.next_element()? {
            values.push(value);
        }

        Ok(MessyVec(values))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(MessyVec(value.split(',').map(String::from).collect()))
    }
}

impl<'de> Deserialize<'de> for MessyVec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::de::Deserializer<'de>,
    {
        deserializer.deserialize_any(MessyVecVisitor)
    }
}

fn human_friendly_uptime(uptime: Duration) -> String {
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
        use wasmcloud_control_interface::Link;

        let links = vec![
            Link::builder()
                .source_id("source_component")
                .target("kv-redis")
                .wit_namespace("wasi")
                .wit_package("keyvalue")
                .interfaces(vec!["atomics".into(), "store".into()])
                .name("default")
                .build()
                .expect("failed to build link"),
            Link::builder()
                .source_id("source_component")
                .target("kv-vault")
                .wit_namespace("wasi")
                .wit_package("keyvalue")
                .interfaces(vec!["atomics".into(), "store".into()])
                .name("secret")
                .source_config(vec![])
                .target_config(vec!["my-secret".into()])
                .build()
                .expect("failed to build link"),
            Link::builder()
                .source_id("source_component")
                .target("kv-vault-offsite")
                .wit_namespace("wasi")
                .wit_package("keyvalue")
                .interfaces(vec!["atomics".into()])
                .name("secret")
                .source_config(vec![])
                .target_config(vec!["my-secret".into()])
                .build()
                .expect("failed to build link"),
            Link::builder()
                .source_id("http")
                .target("source_component")
                .wit_namespace("wasi")
                .wit_package("http")
                .interfaces(vec!["incoming-handler".into()])
                .name("default")
                .source_config(vec!["some-port".into()])
                .target_config(vec![])
                .build()
                .expect("failed to build link"),
            Link::builder()
                .source_id("source_component")
                .target("httpclient")
                .wit_namespace("wasi")
                .wit_package("http")
                .interfaces(vec!["outgoing-handler".into()])
                .name("default")
                .source_config(vec![])
                .target_config(vec!["some-port".into()])
                .build()
                .expect("failed to build link"),
            Link::builder()
                .source_id("source_component")
                .target("other_component")
                .wit_namespace("custom")
                .wit_package("foo")
                .interfaces(vec!["bar".into(), "baz".into()])
                .name("default")
                .source_config(vec![])
                .target_config(vec![])
                .build()
                .expect("failed to build link"),
            Link::builder()
                .source_id("other_component")
                .target("target")
                .wit_namespace("wit")
                .wit_package("package")
                .interfaces(vec!["interface3".into()])
                .name("link2")
                .source_config(vec![])
                .target_config(vec![])
                .build()
                .expect("failed to build link"),
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
