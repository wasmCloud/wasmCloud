mod event;
mod handler;

pub mod config;
/// wasmCloud host configuration
pub mod host_config;

pub use self::host_config::Host as HostConfig;

use std::collections::hash_map::{self, Entry};
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::env::consts::{ARCH, FAMILY, OS};
use std::future::Future;
use std::num::NonZeroUsize;
use std::ops::Deref;
use std::pin::Pin;
use std::process::Stdio;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Context as _};
use async_nats::jetstream::kv::{Entry as KvEntry, Operation, Store};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::{BufMut, Bytes, BytesMut};
use cloudevents::{EventBuilder, EventBuilderV10};
use futures::future::Either;
use futures::stream::{AbortHandle, Abortable, SelectAll};
use futures::{join, stream, try_join, Stream, StreamExt, TryFutureExt, TryStreamExt};
use nkeys::{KeyPair, KeyPairType, XKey};
use secrecy::Secret;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{broadcast, mpsc, watch, RwLock, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{interval_at, timeout_at, Instant};
use tokio::{process, select, spawn};
use tokio_stream::wrappers::IntervalStream;
use tracing::{debug, error, info, instrument, trace, warn, Instrument as _};
use uuid::Uuid;
use wascap::{jwt, prelude::ClaimsBuilder};
use wasmcloud_control_interface::{
    ComponentAuctionAck, ComponentAuctionRequest, ComponentDescription, CtlResponse,
    DeleteInterfaceLinkDefinitionRequest, HostInventory, HostLabel, InterfaceLinkDefinition,
    ProviderAuctionAck, ProviderAuctionRequest, ProviderDescription, RegistryCredential,
    ScaleComponentCommand, StartProviderCommand, StopHostCommand, StopProviderCommand,
    UpdateComponentCommand,
};
use wasmcloud_core::{
    provider_config_update_subject, ComponentId, HealthCheckResponse, HostData, OtelConfig,
    CTL_API_VERSION_1,
};
use wasmcloud_runtime::capability::secrets::store::SecretValue;
use wasmcloud_runtime::component::WrpcServeEvent;
use wasmcloud_runtime::Runtime;
use wasmcloud_secrets_types::SECRET_PREFIX;
use wasmcloud_tracing::context::TraceContextInjector;
use wasmcloud_tracing::{global, KeyValue};

use crate::{
    fetch_component, HostMetrics, OciConfig, PolicyHostInfo, PolicyManager, PolicyResponse,
    RegistryAuth, RegistryConfig, RegistryType, SecretsManager,
};

use self::config::{BundleGenerator, ConfigBundle};
use self::handler::Handler;

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

impl Queue {
    #[instrument]
    async fn new(
        nats: &async_nats::Client,
        topic_prefix: &str,
        lattice: &str,
        host_key: &KeyPair,
    ) -> anyhow::Result<Self> {
        let host_id = host_key.public_key();
        let streams = futures::future::join_all([
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.registry.put",
            ))),
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.host.ping",
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
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.label.*.{host_id}"
            ))),
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.host.*.{host_id}"
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

        let heartbeat_interval = config
            .heartbeat_interval
            .unwrap_or(Self::DEFAULT_HEARTBEAT_INTERVAL);
        let heartbeat_start_at = start_at
            .checked_add(heartbeat_interval)
            .context("failed to compute heartbeat start time")?;
        let heartbeat = IntervalStream::new(interval_at(heartbeat_start_at, heartbeat_interval));

        let (stop_tx, stop_rx) = watch::channel(None);

        // TODO: Configure
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
    async fn inventory(&self) -> HostInventory {
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
            lattice: Arc::clone(&self.host_config.lattice),
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
                self.host_key.public_key(),
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
        let ComponentAuctionRequest {
            component_ref,
            component_id,
            constraints,
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize component auction command")?;

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
        let component_id_running = self.components.read().await.contains_key(&component_id);

        // This host can run the component if all constraints are satisfied and the component is not already running
        if constraints_satisfied && !component_id_running {
            Ok(Some(CtlResponse::ok(ComponentAuctionAck {
                component_ref,
                component_id,
                constraints,
                host_id: self.host_key.public_key(),
            })))
        } else {
            Ok(None)
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_auction_provider(
        &self,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<Option<CtlResponse<ProviderAuctionAck>>> {
        let ProviderAuctionRequest {
            provider_ref,
            provider_id,
            constraints,
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize provider auction command")?;

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
        let provider_running = providers.contains_key(&provider_id);
        if constraints_satisfied && !provider_running {
            Ok(Some(CtlResponse::ok(ProviderAuctionAck {
                provider_ref,
                provider_id,
                constraints,
                host_id: self.host_key.public_key(),
            })))
        } else {
            Ok(None)
        }
    }

    #[instrument(level = "trace", skip_all)]
    async fn fetch_component(&self, component_ref: &str) -> anyhow::Result<Vec<u8>> {
        let registry_config = self.registry_config.read().await;
        fetch_component(
            component_ref,
            self.host_config.allow_file_load,
            &self.host_config.oci_opts.additional_ca_paths,
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
            let StopHostCommand { timeout, host_id } =
                serde_json::from_slice::<StopHostCommand>(payload.as_ref())
                    .context("failed to deserialize stop command")?;

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

        self.heartbeat.abort();
        self.data_watch.abort();
        self.queue.abort();
        self.policy_manager.policy_changes.abort();
        let deadline =
            timeout.and_then(|timeout| Instant::now().checked_add(Duration::from_millis(timeout)));
        self.stop_tx.send_replace(deadline);
        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_scale_component(
        self: Arc<Self>,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        let ScaleComponentCommand {
            component_ref,
            component_id,
            annotations,
            max_instances,
            config,
            allow_update,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize component scale command")?;

        debug!(
            component_ref,
            max_instances, component_id, "handling scale component"
        );

        let host_id = host_id.to_string();
        let annotations: Annotations = annotations.unwrap_or_default().into_iter().collect();

        // Basic validation to ensure that the component is running and that the image reference matches
        // If it doesn't match, we can still successfully scale, but we won't be updating the image reference
        let (original_ref, ref_changed) = {
            self.components
                .read()
                .await
                .get(&component_id)
                .map(|v| {
                    (
                        Some(Arc::clone(&v.image_reference)),
                        &*v.image_reference != component_ref.as_str(),
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

        Ok(CtlResponse {
            success: true,
            message,
            response: None,
        })
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
        let UpdateComponentCommand {
            component_id,
            annotations,
            new_component_ref,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize component update command")?;

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
            .get(&component_id)
            .map(|component| Arc::clone(&component.image_reference))
        else {
            return Ok(CtlResponse::error(&format!(
                "component {component_id} not found"
            )));
        };

        // If the component image reference is the same, respond with an appropriate message
        if &*component_ref == new_component_ref.as_str() {
            return Ok(CtlResponse {
                success: true,
                message: format!("component {component_id} already updated to {new_component_ref}"),
                response: None,
            });
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

        Ok(CtlResponse {
            success: true,
            message,
            response: None,
        })
    }

    async fn handle_update_component_task(
        &self,
        component_id: Arc<str>,
        new_component_ref: Arc<str>,
        host_id: &str,
        annotations: Option<HashMap<String, String>>,
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
        let StartProviderCommand {
            config,
            provider_id,
            provider_ref,
            annotations,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize provider start command")?;

        if self.providers.read().await.contains_key(&provider_id) {
            return Ok(CtlResponse::error(
                "provider with that ID is already running",
            ));
        }

        info!(provider_ref, provider_id, "handling start provider"); // Log at info since starting providers can take a while

        let host_id = host_id.to_string();
        spawn(async move {
            if let Err(err) = self
                .handle_start_provider_task(
                    &config,
                    &provider_id,
                    &provider_ref,
                    annotations.unwrap_or_default(),
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
        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_start_provider_task(
        &self,
        config: &[String],
        provider_id: &str,
        provider_ref: &str,
        annotations: HashMap<String, String>,
        host_id: &str,
    ) -> anyhow::Result<()> {
        trace!(provider_ref, provider_id, "start provider task");

        let registry_config = self.registry_config.read().await;
        let (path, claims_token) = crate::fetch_provider(
            provider_ref,
            host_id,
            self.host_config.allow_file_load,
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
                .host_config
                .rpc_key
                .as_ref()
                .map(|key| key.seed())
                .transpose()
                .context("private key missing for provider RPC key")?;
            let default_rpc_timeout_ms = Some(
                self.host_config
                    .rpc_timeout
                    .as_millis()
                    .try_into()
                    .context("failed to convert rpc_timeout to u64")?,
            );
            let otel_config = OtelConfig {
                enable_observability: self.host_config.otel_config.enable_observability,
                enable_traces: self.host_config.otel_config.enable_traces,
                enable_metrics: self.host_config.otel_config.enable_metrics,
                enable_logs: self.host_config.otel_config.enable_logs,
                observability_endpoint: self.host_config.otel_config.observability_endpoint.clone(),
                traces_endpoint: self.host_config.otel_config.traces_endpoint.clone(),
                metrics_endpoint: self.host_config.otel_config.metrics_endpoint.clone(),
                logs_endpoint: self.host_config.otel_config.logs_endpoint.clone(),
                protocol: self.host_config.otel_config.protocol,
                additional_ca_paths: self.host_config.otel_config.additional_ca_paths.clone(),
                trace_level: self.host_config.otel_config.trace_level.clone(),
            };

            let provider_xkey = XKey::new();
            // The provider itself needs to know its private key
            let provider_xkey_private_key = if let Ok(seed) = provider_xkey.seed() {
                seed
            } else if self.host_config.secrets_topic_prefix.is_none() {
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
                .filter(|link| link.source_id == provider_id || link.target == provider_id);
            let link_definitions = stream::iter(provider_links)
                .filter_map(|link| async {
                    if link.source_id == provider_id || link.target == provider_id {
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
                                    source_id = link.source_id,
                                    target = link.target,
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
                host_id: self.host_key.public_key(),
                lattice_rpc_prefix: self.host_config.lattice.to_string(),
                link_name: "default".to_string(),
                lattice_rpc_user_jwt: self.host_config.rpc_jwt.clone().unwrap_or_default(),
                lattice_rpc_user_seed: lattice_rpc_user_seed.unwrap_or_default(),
                lattice_rpc_url: self.host_config.rpc_nats_url.to_string(),
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
                log_level: Some(self.host_config.log_level.clone()),
                structured_logging: self.host_config.enable_structured_logging,
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
            let ctl_nats = self.ctl_nats.clone();
            let event_builder = self.event_builder.clone();
            // NOTE: health_ prefix here is to allow us to move the variables into the closure
            let health_lattice = self.host_config.lattice.clone();
            let health_host_id = host_id.to_string();
            let health_provider_id = provider_id.to_string();
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
                                            if let Err(e) = event::publish(
                                                &event_builder,
                                                &ctl_nats,
                                                &health_lattice,
                                                "health_check_passed",
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
                                            if let Err(e) = event::publish(
                                                &event_builder,
                                                &ctl_nats,
                                                &health_lattice,
                                                "health_check_failed",
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
                                            if let Err(e) = event::publish(
                                                &event_builder,
                                                &ctl_nats,
                                                &health_lattice,
                                                "health_check_status",
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
            let lattice = self.host_config.lattice.to_string();
            let client = self.rpc_nats.clone();
            let config = Arc::new(RwLock::new(config));
            let update_config = config.clone();
            let config_update_task = spawn(async move {
                let subject = provider_config_update_subject(&lattice, &provider_id);
                trace!(provider_id, "starting config update listener");
                let mut update_config = update_config.write().await;
                loop {
                    select! {
                        update = update_config.changed() => {
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
        let StopProviderCommand { provider_id, .. } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize provider stop command")?;

        debug!(provider_id, "handling stop provider");

        let mut providers = self.providers.write().await;
        let hash_map::Entry::Occupied(entry) = providers.entry(provider_id.clone()) else {
            warn!(
                provider_id,
                "received request to stop provider that is not running"
            );
            return Ok(CtlResponse::error("provider with that ID is not running"));
        };
        let Provider {
            ref health_check_task,
            ref config_update_task,
            ref annotations,
            ..
        } = entry.remove();

        // Send a request to the provider, requesting a graceful shutdown
        let req = serde_json::to_vec(&json!({ "host_id": host_id }))
            .context("failed to encode provider stop request")?;
        let req = async_nats::Request::new()
            .payload(req.into())
            .timeout(self.host_config.provider_shutdown_delay)
            .headers(injector_to_headers(
                &TraceContextInjector::default_with_span(),
            ));
        if let Err(e) = self
            .rpc_nats
            .send_request(
                format!(
                    "wasmbus.rpc.{}.{provider_id}.default.shutdown",
                    self.host_config.lattice
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
        health_check_task.abort();
        config_update_task.abort();
        info!(provider_id, "provider stopped");
        self.publish_event(
            "provider_stopped",
            event::provider_stopped(annotations, host_id, provider_id, "stop"),
        )
        .await?;
        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_inventory(&self) -> anyhow::Result<CtlResponse<HostInventory>> {
        trace!("handling inventory");
        let inventory = self.inventory().await;
        Ok(CtlResponse::ok(inventory))
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
        let links: Vec<&InterfaceLinkDefinition> = links.values().flatten().collect();
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
            serde_json::to_vec(&CtlResponse::<()> {
                success: true,
                response: None,
                message: "Configuration not found".to_string(),
            })
            .map_err(anyhow::Error::from)
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_label_put(
        &self,
        host_id: &str,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<CtlResponse<()>> {
        let HostLabel { key, value } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize put label request")?;
        let mut labels = self.labels.write().await;
        match labels.entry(key) {
            Entry::Occupied(mut entry) => {
                info!(key = entry.key(), value, "updated label");
                entry.insert(value);
            }
            Entry::Vacant(entry) => {
                info!(key = entry.key(), value, "set label");
                entry.insert(value);
            }
        }

        self.publish_event(
            "labels_changed",
            event::labels_changed(host_id, labels.clone()),
        )
        .await
        .context("failed to publish labels_changed event")?;

        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_label_del(
        &self,
        host_id: &str,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<CtlResponse<()>> {
        let HostLabel { key, .. } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize delete label request")?;
        let mut labels = self.labels.write().await;
        let value = labels.remove(&key);

        if value.is_none() {
            warn!(key, "could not remove unset label");
            return Ok(CtlResponse::success());
        };

        info!(key, "removed label");
        self.publish_event(
            "labels_changed",
            event::labels_changed(host_id, labels.clone()),
        )
        .await
        .context("failed to publish labels_changed event")?;

        Ok(CtlResponse::success())
    }

    /// Handle a new link by modifying the relevant source [ComponentSpeficication]. Once
    /// the change is written to the LATTICEDATA store, each host in the lattice (including this one)
    /// will handle the new specification and update their own internal link maps via [process_component_spec_put].
    #[instrument(level = "debug", skip_all)]
    async fn handle_link_put(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<CtlResponse<()>> {
        let payload = payload.as_ref();
        let interface_link_definition: InterfaceLinkDefinition = serde_json::from_slice(payload)
            .context("failed to deserialize wrpc link definition")?;
        let InterfaceLinkDefinition {
            source_id,
            target,
            wit_namespace,
            wit_package,
            interfaces,
            name,
            source_config: _,
            target_config: _,
        } = interface_link_definition.clone();

        let ns_and_package = format!("{wit_namespace}:{wit_package}");
        debug!(
            source_id,
            target,
            ns_and_package,
            name,
            ?interfaces,
            "handling put wrpc link definition"
        );

        self.validate_config(
            interface_link_definition
                .source_config
                .iter()
                .chain(&interface_link_definition.target_config),
        )
        .await?;

        let mut component_spec = self
            .get_component_spec(&source_id)
            .await?
            .unwrap_or_default();

        // If we can find an existing link with the same source, target, namespace, package, and name, update it.
        // Otherwise, add the new link to the component specification.
        if let Some(existing_link_index) = component_spec.links.iter().position(|link| {
            link.source_id == source_id
                && link.target == target
                && link.wit_namespace == wit_namespace
                && link.wit_package == wit_package
                && link.name == name
        }) {
            if let Some(existing_link) = component_spec.links.get_mut(existing_link_index) {
                *existing_link = interface_link_definition.clone();
            }
        } else {
            component_spec.links.push(interface_link_definition.clone());
        };

        // Update component specification with the new link
        self.store_component_spec(&source_id, &component_spec)
            .await?;

        let set_event = event::linkdef_set(&interface_link_definition);
        self.publish_event("linkdef_set", set_event).await?;

        self.put_backwards_compat_provider_link(&interface_link_definition)
            .await?;

        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    /// Remove an interface link on a source component for a specific package
    async fn handle_link_del(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<CtlResponse<()>> {
        let payload = payload.as_ref();
        let DeleteInterfaceLinkDefinitionRequest {
            source_id,
            wit_namespace,
            wit_package,
            name,
        } = serde_json::from_slice(payload)
            .context("failed to deserialize wrpc link definition")?;

        let ns_and_package = format!("{wit_namespace}:{wit_package}");

        debug!(
            source_id,
            ns_and_package, name, "handling del wrpc link definition"
        );

        let Some(mut component_spec) = self.get_component_spec(&source_id).await? else {
            // If the component spec doesn't exist, the link is deleted
            return Ok(CtlResponse::success());
        };

        // If we can find an existing link with the same source, namespace, package, and name, remove it
        // and update the component specification.
        let deleted_link_target = if let Some(existing_link_index) =
            component_spec.links.iter().position(|link| {
                link.source_id == source_id
                    && link.wit_namespace == wit_namespace
                    && link.wit_package == wit_package
                    && link.name == name
            }) {
            // Sanity safety check since `swap_remove` will panic if the index is out of bounds
            if existing_link_index < component_spec.links.len() {
                Some(component_spec.links.swap_remove(existing_link_index).target)
            } else {
                None
            }
        } else {
            None
        };

        // Update component specification with the new link
        self.store_component_spec(&source_id, &component_spec)
            .await?;

        self.publish_event(
            "linkdef_deleted",
            event::linkdef_deleted(&source_id, name, wit_namespace, wit_package),
        )
        .await?;

        self.del_provider_link(&source_id, deleted_link_target, payload.to_owned().into())
            .await?;

        Ok(CtlResponse::success())
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
            let mut new_config = RegistryConfig::from(new_creds);
            match registry_config.entry(reg) {
                hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().auth = new_config.auth;
                }
                hash_map::Entry::Vacant(entry) => {
                    new_config.allow_latest = self.host_config.oci_opts.allow_latest;
                    entry.insert(new_config);
                }
            }
        }

        Ok(CtlResponse::success())
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

        Ok(CtlResponse::success())
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

        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_ping_hosts(
        &self,
        _payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<CtlResponse<wasmcloud_control_interface::Host>> {
        trace!("replying to ping");
        let uptime = self.start_at.elapsed();

        Ok(CtlResponse::ok(wasmcloud_control_interface::Host {
            id: self.host_key.public_key(),
            labels: self.labels.read().await.clone(),
            friendly_name: self.friendly_name.clone(),
            uptime_seconds: uptime.as_secs(),
            uptime_human: Some(human_friendly_uptime(uptime)),
            version: Some(self.host_config.version.clone()),
            js_domain: self.host_config.js_domain.clone(),
            ctl_host: Some(self.host_config.ctl_nats_url.to_string()),
            rpc_host: Some(self.host_config.rpc_nats_url.to_string()),
            lattice: self.host_config.lattice.to_string(),
        }))
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
            // Host commands
            (Some("host"), Some("get"), Some(_host_id), None) => self
                .handle_inventory()
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("host"), Some("ping"), None, None) => self
                .handle_ping_hosts(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("host"), Some("stop"), Some(host_id), None) => self
                .handle_stop_host(message.payload, host_id)
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
            // Label commands
            (Some("label"), Some("del"), Some(host_id), None) => self
                .handle_label_del(host_id, message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("label"), Some("put"), Some(host_id), None) => self
                .handle_label_put(host_id, message.payload)
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
    ///
    /// TODO: Instead of delivering the named config, deliver the actual source and target config to the provider
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
