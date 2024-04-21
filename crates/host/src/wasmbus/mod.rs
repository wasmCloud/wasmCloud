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
use futures::stream::{select_all, AbortHandle, Abortable, SelectAll};
use futures::{join, stream, try_join, Stream, StreamExt, TryFutureExt, TryStreamExt};
use nkeys::{KeyPair, KeyPairType};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::{stderr, AsyncWrite, AsyncWriteExt};
use tokio::sync::{oneshot, watch, RwLock};
use tokio::task::JoinHandle;
use tokio::time::{interval_at, Instant};
use tokio::{process, select, spawn};
use tokio_stream::wrappers::IntervalStream;
use tracing::{debug, error, info, instrument, trace, warn};
use uuid::Uuid;
use wascap::{jwt, prelude::ClaimsBuilder};
use wasmcloud_control_interface::{
    ComponentAuctionAck, ComponentAuctionRequest, ComponentDescription, CtlResponse,
    DeleteInterfaceLinkDefinitionRequest, HostInventory, HostLabel, InterfaceLinkDefinition,
    ProviderAuctionAck, ProviderAuctionRequest, ProviderDescription, RegistryCredential,
    ScaleComponentCommand, StartProviderCommand, StopHostCommand, StopProviderCommand,
    UpdateComponentCommand,
};
use wasmcloud_core::{HealthCheckResponse, HostData, OtelConfig, CTL_API_VERSION_1};
use wasmcloud_runtime::capability::{messaging, IncomingHttp as _, MessagingHandler as _};
use wasmcloud_runtime::Runtime;
use wasmcloud_tracing::context::TraceContextInjector;
use wasmcloud_tracing::{global, KeyValue};
use wasmtime_wasi_http::body::HyperOutgoingBody;
use wrpc_transport::{AcceptedInvocation, Client, Transmitter as _};
use wrpc_types::DynamicFunction;

use crate::bindings::wasmcloud;
use crate::{
    fetch_actor, HostMetrics, OciConfig, PolicyHostInfo, PolicyManager, PolicyResponse,
    RegistryAuth, RegistryConfig, RegistryType,
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
struct Actor {
    component: wasmcloud_runtime::Component,
    /// Unique component identifier for this component
    id: String,
    calls: AbortHandle,
    handler: Handler,
    annotations: Annotations,
    /// Maximum number of instances of this component that can be running at once
    max_instances: NonZeroUsize,
    image_reference: String,
    metrics: Arc<HostMetrics>,
    #[allow(unused)]
    policy_manager: Arc<PolicyManager>,
}

impl Deref for Actor {
    type Target = wasmcloud_runtime::Component;

    fn deref(&self) -> &Self::Target {
        &self.component
    }
}

// This enum is used to differentiate between component export invocations
enum InvocationParams {
    Custom {
        instance: Arc<String>,
        name: Arc<String>,
        params: Vec<wrpc_transport::Value>,
    },
    IncomingHttpHandle(http::Request<wasmtime_wasi_http::body::HyperIncomingBody>),
    MessagingHandleMessage(wasmcloud::messaging::types::BrokerMessage),
}

impl std::fmt::Debug for InvocationParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InvocationParams::Custom { instance, name, .. } => f
                .debug_struct("Custom")
                .field("interface", instance)
                .field("function", name)
                .finish(),
            InvocationParams::IncomingHttpHandle(_) => f.debug_tuple("IncomingHttpHandle").finish(),
            InvocationParams::MessagingHandleMessage(_) => {
                f.debug_tuple("MessagingHandleMessage").finish()
            }
        }
    }
}

impl Actor {
    /// Handle an incoming wRPC request to invoke an export on this component instance.
    #[instrument(
        level = "info",
        skip(self, context, result_subject, transmitter),
        fields(
            component_id = self.id.as_str(),
            component_ref = self.image_reference.as_str())
    )]
    async fn handle_invocation(
        &self,
        context: Option<async_nats::HeaderMap>,
        params: InvocationParams,
        result_subject: wrpc_transport_nats::Subject,
        transmitter: &wasmcloud_core::wrpc::TransmitterWithHeaders,
    ) -> anyhow::Result<()> {
        let (interface, function) = match params {
            InvocationParams::Custom {
                ref instance,
                ref name,
                ..
            } => (instance.to_string(), name.to_string()),
            InvocationParams::IncomingHttpHandle(_) => (
                "wasi:http/incoming-handler".to_string(),
                "handle".to_string(),
            ),
            InvocationParams::MessagingHandleMessage(_) => (
                "wasmcloud:messaging/handler".to_string(),
                "handle-message".to_string(),
            ),
        };
        let PolicyResponse {
            request_id,
            permitted,
            message,
        } = self
            .policy_manager
            .evaluate_perform_invocation(
                &self.id,
                &self.image_reference,
                &self.annotations,
                self.claims(),
                interface,
                function,
            )
            .await?;
        ensure!(
            permitted,
            "policy denied request to invoke component `{request_id}`: `{message:?}`",
        );

        if let Some(ref context) = context {
            // TODO: wasmcloud_tracing take HeaderMap for my own sanity
            // Coerce the HashMap<String, Vec<String>> into a Vec<(String, String)> by
            // flattening the values
            let trace_context = context
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

        // Instantiate component with expected handlers
        let mut actor = self
            .instantiate()
            .context("failed to instantiate component")?;
        actor
            .stderr(stderr())
            .await
            .context("failed to set stderr")?
            .blobstore(Arc::new(self.handler.clone()))
            .bus(Arc::new(self.handler.clone()))
            .config(Arc::new(self.handler.clone()))
            .keyvalue_atomics(Arc::new(self.handler.clone()))
            .keyvalue_store(Arc::new(self.handler.clone()))
            .logging(Arc::new(self.handler.clone()))
            .messaging(Arc::new(self.handler.clone()))
            .outgoing_http(Arc::new(self.handler.clone()));

        // TODO(metrics): insert information about the source once we have concrete context data
        let mut attributes = vec![
            KeyValue::new("component.ref", self.image_reference.clone()),
            KeyValue::new("lattice", self.metrics.lattice_id.clone()),
            KeyValue::new("host", self.metrics.host_id.clone()),
        ];

        let start_at = Instant::now();
        match params {
            InvocationParams::Custom {
                instance,
                name,
                params,
            } => {
                let res = actor
                    .call(&instance, &name, params)
                    .await
                    .context("failed to call component");
                let elapsed = u64::try_from(start_at.elapsed().as_nanos()).unwrap_or_default();
                attributes.push(KeyValue::new("operation", format!("{instance}/{name}")));
                self.metrics
                    .record_component_invocation(elapsed, &attributes, res.is_err());
                let results = res?;
                transmitter
                    .transmit_tuple_dynamic(result_subject, results)
                    .await
            }
            InvocationParams::IncomingHttpHandle(request) => {
                let actor = actor
                    .into_incoming_http()
                    .await
                    .context("failed to instantiate `wasi:http/incoming-handler`")?;
                let (response_tx, response_rx) = oneshot::channel::<
                    Result<
                        http::Response<HyperOutgoingBody>,
                        wasmtime_wasi_http::bindings::http::types::ErrorCode,
                    >,
                >();
                let res = try_join!(
                    async {
                        actor
                            .handle(request, response_tx)
                            .await
                            .context("failed to call `wasi:http/incoming-handler.handle`")
                    },
                    async {
                        let res = match response_rx.await.context("failed to receive response")? {
                            Ok(resp) => {
                                let (resp, errors) =
                                    wrpc_interface_http::try_http_to_outgoing_response(resp)
                                        .context("failed to convert response")?;
                                // TODO: Handle body errors better
                                spawn(errors.for_each(|err| async move {
                                    error!(?err, "body error encountered");
                                }));
                                Result::Ok::<_, wrpc_interface_http::ErrorCode>(resp)
                            }
                            Err(err) => Err(err.into()),
                        };
                        transmitter
                            .transmit_static(result_subject, res)
                            .await
                            .context("failed to transmit response")
                    }
                );
                let elapsed = u64::try_from(start_at.elapsed().as_nanos()).unwrap_or_default();
                attributes.push(KeyValue::new(
                    "operation",
                    "wrpc:http/incoming-handler.handle",
                ));
                self.metrics
                    .record_component_invocation(elapsed, &attributes, res.is_err());
                res?;
                Ok(())
            }
            InvocationParams::MessagingHandleMessage(
                wasmcloud::messaging::types::BrokerMessage {
                    subject,
                    body,
                    reply_to,
                },
            ) => {
                let actor = actor
                    .into_messaging_handler()
                    .await
                    .context("failed to instantiate `wasmcloud:messaging/handler`")?;
                let res = actor
                    .handle_message(&messaging::types::BrokerMessage {
                        subject,
                        body,
                        reply_to,
                    })
                    .await
                    .context("failed to call `wasmcloud:messaging/handler.handle-message`");
                let elapsed = u64::try_from(start_at.elapsed().as_nanos()).unwrap_or_default();
                attributes.push(KeyValue::new(
                    "operation",
                    "wasmcloud:messaging/handler.handle-message",
                ));
                self.metrics
                    .record_component_invocation(elapsed, &attributes, res.is_err());
                let res = res?;
                transmitter.transmit_static(result_subject, res).await
            }
        }
    }
}

#[derive(Debug)]
struct Provider {
    child: JoinHandle<()>,
    annotations: Annotations,
    image_ref: String,
    claims: Option<jwt::Claims<jwt::CapabilityProvider>>,
}

/// wasmCloud Host
pub struct Host {
    /// The actor map is a map of actor component ID to actor
    actors: RwLock<HashMap<String, Arc<Actor>>>,
    event_builder: EventBuilderV10,
    friendly_name: String,
    heartbeat: AbortHandle,
    host_config: HostConfig,
    host_key: Arc<KeyPair>,
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
    actor_claims: Arc<RwLock<HashMap<String, jwt::Claims<jwt::Component>>>>, // TODO: use a single map once Claims is an enum
    provider_claims: Arc<RwLock<HashMap<String, jwt::Claims<jwt::CapabilityProvider>>>>,
    metrics: Arc<HostMetrics>,
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

#[allow(clippy::large_enum_variant)] // Without this clippy complains actor is at least 0 bytes while provider is at least 280 bytes. That doesn't make sense
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

        // TODO: Configure
        let runtime = Runtime::builder()
            .actor_config(wasmcloud_runtime::ActorConfig {
                require_signature: true,
            })
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
                lattice: config.lattice.clone(),
                labels: labels.clone(),
            },
            config.policy_service_config.policy_topic.clone(),
            config.policy_service_config.policy_timeout_ms,
            config.policy_service_config.policy_changes_topic.clone(),
        )
        .await?;

        let meter = global::meter_with_version(
            "wasmcloud-host",
            Some(config.version.clone()),
            None::<&str>,
            Some(vec![
                KeyValue::new("host.id", host_key.public_key()),
                KeyValue::new("host.version", config.version.clone()),
            ]),
        );
        let metrics = HostMetrics::new(
            &meter,
            host_key.public_key().clone(),
            config.lattice.clone(),
        );

        let config_generator = BundleGenerator::new(config_data.clone());

        let host = Host {
            actors: RwLock::default(),
            event_builder,
            friendly_name,
            heartbeat: heartbeat_abort.clone(),
            ctl_topic_prefix: config.ctl_topic_prefix.clone(),
            host_key,
            labels: RwLock::new(labels),
            ctl_nats,
            rpc_nats: Arc::new(rpc_nats),
            host_config: config,
            data: data.clone(),
            data_watch: data_watch_abort.clone(),
            config_data: config_data.clone(),
            config_generator,
            policy_manager,
            providers: RwLock::default(),
            registry_config,
            runtime,
            start_at,
            stop_rx,
            stop_tx,
            queue: queue_abort.clone(),
            links: RwLock::default(),
            actor_claims: Arc::default(),
            provider_claims: Arc::default(),
            metrics: Arc::new(metrics),
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
        let actors = self.actors.read().await;
        let components: Vec<_> = stream::iter(actors.iter())
            .filter_map(|(id, actor)| async move {
                let name = actor
                    .claims()
                    .and_then(|claims| claims.metadata.as_ref())
                    .and_then(|metadata| metadata.name.as_ref())
                    .cloned();
                Some(ComponentDescription {
                    id: id.into(),
                    image_ref: actor.image_reference.clone(),
                    annotations: Some(actor.annotations.clone().into_iter().collect()),
                    max_instances: actor.max_instances.get().try_into().unwrap_or(u32::MAX),
                    revision: actor
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
                        claims,
                        image_ref,
                        ..
                    },
                )| {
                    let name = claims
                        .as_ref()
                        .and_then(|claims| claims.metadata.as_ref())
                        .and_then(|metadata| metadata.name.as_ref())
                        .cloned();
                    let annotations = Some(annotations.clone().into_iter().collect());
                    ProviderDescription {
                        id: provider_id.into(),
                        image_ref: Some(image_ref.clone()),
                        name: name.clone(),
                        annotations,
                        revision: claims
                            .as_ref()
                            .and_then(|claims| claims.metadata.as_ref())
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

    /// Instantiate an actor
    #[allow(clippy::too_many_arguments)] // TODO: refactor into a config struct
    #[instrument(level = "debug", skip_all)]
    async fn instantiate_actor(
        &self,
        annotations: &Annotations,
        component_ref: String,
        component_id: String,
        max_instances: NonZeroUsize,
        component: wasmcloud_runtime::Component,
        handler: Handler,
    ) -> anyhow::Result<Arc<Actor>> {
        trace!(component_ref, max_instances, "instantiating component");

        let wrpc = wasmcloud_core::wrpc::Client::new(
            self.rpc_nats.clone(),
            &self.host_config.lattice,
            &component_id,
            // NOTE(brooksmtownsend): We only use this client for serving functions,
            // and the headers will be set by the incoming invocation.
            async_nats::HeaderMap::new(),
            Duration::default(), // this client should not invoke anything
        );
        let (calls_abort, calls_abort_reg) = AbortHandle::new_pair();
        let actor = Arc::new(Actor {
            component: component.clone(),
            id: component_id,
            calls: calls_abort,
            handler: handler.clone(),
            annotations: annotations.clone(),
            max_instances,
            policy_manager: Arc::clone(&self.policy_manager),
            image_reference: component_ref,
            metrics: Arc::clone(&self.metrics),
        });

        let mut exports: Vec<Pin<Box<dyn Stream<Item = _> + Send>>> = Vec::new();
        for (instance, functions) in component.exports().iter() {
            match instance.as_str() {
                "wasi:http/incoming-handler@0.2.0" => {
                    use wrpc_interface_http::IncomingHandler;

                    let invocations = wrpc
                        .serve_handle_wasmtime()
                        .await
                        .context("failed to serve `wrpc:http/incoming-handler.handle`")?;
                    exports.push(Box::pin(invocations.map(move |invocation| {
                        invocation.map(
                            |AcceptedInvocation {
                                 context,
                                 params,
                                 result_subject,
                                 error_subject,
                                 transmitter,
                             }| AcceptedInvocation {
                                context,
                                params: InvocationParams::IncomingHttpHandle(params.0),
                                result_subject,
                                error_subject,
                                transmitter,
                            },
                        )
                    })));
                }
                "wasmcloud:messaging/handler@0.2.0" => {
                    let invocations = wrpc
                        .serve_static(instance, "handle-message")
                        .await
                        .context("failed to serve `wasmcloud:messaging/handler.handle-message`")?;
                    exports.push(Box::pin(invocations.map(move |invocation| {
                        invocation.map(
                            |AcceptedInvocation {
                                 context,
                                 params,
                                 result_subject,
                                 error_subject,
                                 transmitter,
                             }| AcceptedInvocation {
                                context,
                                params: InvocationParams::MessagingHandleMessage(params),
                                result_subject,
                                error_subject,
                                transmitter,
                            },
                        )
                    })));
                }
                _ => {
                    let instance = Arc::new(instance.to_string());
                    for (name, function) in functions {
                        if let wrpc_types::DynamicFunction::Static { params, .. } = function {
                            // TODO(#1220): In order to implement invocation signing and response verification, we can override the
                            // wrpc_transport::Invocation and wrpc_transport::Client trait in order to wrap the invocation with necessary
                            // logic to verify the incoming invocations and sign the outgoing responses.
                            trace!(?instance, name, "serving wrpc function export");
                            let invocations = wrpc
                                .serve_dynamic(&instance, name, params.clone())
                                .await
                                .context("failed to serve custom function export")?;
                            let name = Arc::new(name.to_string());
                            let instance = Arc::clone(&instance);
                            exports.push(Box::pin(invocations.map(move |invocation| {
                                invocation.map(
                                    |AcceptedInvocation {
                                         context,
                                         params,
                                         result_subject,
                                         error_subject,
                                         transmitter,
                                     }| AcceptedInvocation {
                                        context,
                                        params: InvocationParams::Custom {
                                            instance: Arc::clone(&instance),
                                            name: Arc::clone(&name),
                                            params,
                                        },
                                        result_subject,
                                        error_subject,
                                        transmitter,
                                    },
                                )
                            })));
                        }
                    }
                }
            }
        }

        let _calls = spawn({
            let actor = Arc::clone(&actor);
            Abortable::new(select_all(exports), calls_abort_reg).for_each_concurrent(
                max_instances.get(),
                move |invocation| {
                    let actor = Arc::clone(&actor);
                    async move {
                        let AcceptedInvocation {
                            context,
                            params,
                            result_subject,
                            error_subject,
                            transmitter,
                        } = match invocation {
                            Ok(invocation) => invocation,
                            Err(err) => {
                                error!(?err, "failed to accept invocation");
                                return;
                            }
                        };
                        if let Err(err) = {
                            actor
                                .handle_invocation(context, params, result_subject, &transmitter)
                                .await
                        } {
                            error!(?err, "failed to handle invocation");
                            if let Err(err) = transmitter
                                .transmit_static(error_subject, format!("{err:#}"))
                                .await
                            {
                                error!(?err, "failed to transmit error to invoker");
                            }
                        }
                    }
                },
            )
        });
        Ok(actor)
    }

    #[allow(clippy::too_many_arguments)]
    #[instrument(level = "debug", skip_all)]
    async fn start_actor<'a>(
        &self,
        entry: hash_map::VacantEntry<'a, String, Arc<Actor>>,
        component: wasmcloud_runtime::Component,
        component_ref: String,
        component_id: String,
        max_instances: NonZeroUsize,
        annotations: impl Into<Annotations>,
        config: ConfigBundle,
    ) -> anyhow::Result<&'a mut Arc<Actor>> {
        debug!(component_ref, ?max_instances, "starting new component");

        let annotations = annotations.into();
        let claims = component.claims();
        if let Some(claims) = claims {
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
        let imports = get_import_results(&component);
        let handler = Handler {
            nats: Arc::clone(&self.rpc_nats),
            config_data: Arc::new(RwLock::new(config)),
            lattice: self.host_config.lattice.clone(),
            component_id: component_id.clone(),
            targets: Arc::default(),
            interface_links: Arc::new(RwLock::new(component_import_links(&component_spec.links))),
            polyfilled_imports: imports,
            invocation_timeout: Duration::from_secs(10), // TODO: Make this configurable
        };

        let actor = self
            .instantiate_actor(
                &annotations,
                component_ref.clone(),
                component_id.clone(),
                max_instances,
                component.clone(),
                handler.clone(),
            )
            .await
            .context("failed to instantiate component")?;

        info!(component_ref, "component started");
        self.publish_event(
            "component_scaled",
            event::component_scaled(
                claims,
                &annotations,
                &self.host_key.public_key(),
                max_instances,
                &component_ref,
                &component_id,
            ),
        )
        .await?;

        Ok(entry.insert(actor))
    }

    #[instrument(level = "debug", skip_all)]
    async fn stop_actor(&self, actor: &Actor, _host_id: &str) -> anyhow::Result<()> {
        trace!(component_id = %actor.id, "stopping component");

        // TODO: How to know if abort was successful?
        actor.calls.abort();

        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_auction_actor(
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
        let component_id_running = self.actors.read().await.contains_key(&component_id);

        // This host can run the actor if all constraints are satisfied and the actor is not already running
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
    async fn fetch_actor(
        &self,
        component_ref: &str,
    ) -> anyhow::Result<wasmcloud_runtime::Component> {
        let registry_config = self.registry_config.read().await;
        let actor = fetch_actor(
            component_ref,
            self.host_config.allow_file_load,
            &registry_config,
        )
        .await
        .context("failed to fetch component")?;
        let actor = wasmcloud_runtime::Component::new(&self.runtime, actor)
            .context("failed to initialize component")?;
        Ok(actor)
    }

    #[instrument(level = "trace", skip_all)]
    async fn store_actor_claims(&self, claims: jwt::Claims<jwt::Component>) -> anyhow::Result<()> {
        let mut actor_claims = self.actor_claims.write().await;
        actor_claims.insert(claims.subject.clone(), claims);
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_stop_host(
        &self,
        payload: impl AsRef<[u8]>,
        _host_id: &str,
    ) -> anyhow::Result<CtlResponse<()>> {
        let StopHostCommand { timeout, .. } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize stop command")?;

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
    async fn handle_scale_actor(
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
        let message = match self.actors.read().await.get(&component_id) {
            Some(entry) if entry.image_reference != component_ref => {
                let msg = format!(
                    "Requested to scale existing component to a different image reference: {} != {}. The component will be scaled but the image reference will not be updated. If you meant to update this component to a new image ref, use the update command.",
                    entry.image_reference, component_ref,
                );
                warn!(msg);
                msg
            }
            _ => String::with_capacity(0),
        };

        spawn(async move {
            if let Err(e) = self
                .handle_scale_actor_task(
                    &component_ref,
                    &component_id,
                    &host_id,
                    max_instances,
                    annotations,
                    config,
                )
                .await
            {
                error!(%component_ref, %component_id, err = ?e, "failed to scale component");
            }
        });
        Ok(CtlResponse {
            success: true,
            message,
            response: None,
        })
    }

    #[instrument(level = "debug", skip_all)]
    /// Handles scaling an actor to a supplied number of `max` concurrently executing instances.
    /// Supplying `0` will result in stopping that actor instance.
    async fn handle_scale_actor_task(
        &self,
        component_ref: &str,
        component_id: &str,
        host_id: &str,
        max_instances: u32,
        annotations: Annotations,
        config: Vec<String>,
    ) -> anyhow::Result<()> {
        trace!(component_ref, max_instances, "scale component task");

        let actor = self.fetch_actor(component_ref).await?;
        let claims = actor.claims();
        let resp = self
            .policy_manager
            .evaluate_start_component(
                component_id,
                component_ref,
                max_instances,
                &annotations,
                claims,
            )
            .await?;
        if !resp.permitted {
            bail!(
                "Policy denied request to scale component `{}`: `{:?}`",
                resp.request_id,
                resp.message
            )
        };

        let component_ref = component_ref.to_string();
        match (
            self.actors.write().await.entry(component_id.to_string()),
            NonZeroUsize::new(max_instances as usize),
        ) {
            // No actor is running and we requested to scale to zero, noop
            (hash_map::Entry::Vacant(_), None) => {}
            // No actor is running and we requested to scale to some amount, start with specified max
            (hash_map::Entry::Vacant(entry), Some(max)) => {
                let config = self
                    .config_generator
                    .generate(config)
                    .await
                    .context("Unable to fetch requested config")?;
                if let Err(e) = self
                    .start_actor(
                        entry,
                        actor.clone(),
                        component_ref.clone(),
                        component_id.to_string(),
                        max,
                        annotations.clone(),
                        config,
                    )
                    .await
                {
                    self.publish_event(
                        "component_scale_failed",
                        event::component_scale_failed(
                            claims,
                            &annotations,
                            host_id,
                            &component_ref,
                            component_id,
                            max,
                            &e,
                        ),
                    )
                    .await?;
                    return Err(e);
                }
            }
            // Actor is running and we requested to scale to zero instances, stop actor
            (hash_map::Entry::Occupied(entry), None) => {
                let actor = entry.remove();
                if let Err(err) = self
                    .stop_actor(&actor, host_id)
                    .await
                    .context("failed to stop component in response to scale to zero")
                {
                    self.publish_event(
                        "component_scale_failed",
                        event::component_scale_failed(
                            claims,
                            &actor.annotations,
                            host_id,
                            component_ref,
                            &actor.id,
                            actor.max_instances,
                            &err,
                        ),
                    )
                    .await?;
                    return Err(err);
                };

                info!(component_ref, "component stopped");
                self.publish_event(
                    "component_scaled",
                    event::component_scaled(
                        claims,
                        &actor.annotations,
                        host_id,
                        0_usize,
                        &actor.image_reference,
                        &actor.id,
                    ),
                )
                .await?;
            }
            // Actor is running and we requested to scale to some amount or unbounded, scale actor
            (hash_map::Entry::Occupied(mut entry), Some(max)) => {
                let actor = entry.get_mut();
                let config_changed =
                    &config != actor.handler.config_data.read().await.config_names();

                // Modify scale only if the requested max differs from the current max or if the configuration has changed
                if actor.max_instances != max || config_changed {
                    // We must partially clone the handler as we can't be sharing the targets between actors
                    let handler = actor.handler.copy_for_new();
                    if config_changed {
                        let mut conf = handler.config_data.write().await;
                        *conf = self.config_generator.generate(config).await?;
                    }
                    let instance = self
                        .instantiate_actor(
                            &annotations,
                            component_ref.to_string(),
                            actor.id.to_string(),
                            max,
                            actor.component.clone(),
                            handler,
                        )
                        .await
                        .context("failed to instantiate component")?;
                    let publish_result = match actor.max_instances.cmp(&max) {
                        std::cmp::Ordering::Less | std::cmp::Ordering::Greater => {
                            self.publish_event(
                                "component_scaled",
                                event::component_scaled(
                                    actor.claims(),
                                    &actor.annotations,
                                    host_id,
                                    max,
                                    &actor.image_reference,
                                    &actor.id,
                                ),
                            )
                            .await
                        }
                        std::cmp::Ordering::Equal => Ok(()),
                    };
                    let actor = entry.insert(instance);
                    self.stop_actor(&actor, host_id)
                        .await
                        .context("failed to stop component after scaling")?;

                    info!(component_ref, ?max, "component scaled");

                    // Wait to unwrap the event publish result until after we've processed the instances
                    publish_result?;
                }
            }
        }
        Ok(())
    }

    // TODO(#1548): With component IDs, new actor references, configuration, etc, we're going to need to do some
    // design thinking around how update actor should work. Should it be limited to a single host or latticewide?
    // Should it also update configuration, or is that separate? Should scaling be done via an update?
    #[instrument(level = "debug", skip_all)]
    async fn handle_update_actor(
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

        let component_id = component_id.to_string();
        let new_component_ref = new_component_ref.to_string();
        let host_id = host_id.to_string();
        spawn(async move {
            if let Err(e) = self
                .handle_update_actor_task(&component_id, &new_component_ref, &host_id, annotations)
                .await
            {
                error!(%new_component_ref, %component_id, err = ?e, "failed to update component");
            }
        });

        Ok(CtlResponse::success())
    }

    async fn handle_update_actor_task(
        &self,
        component_id: &str,
        new_component_ref: &str,
        host_id: &str,
        annotations: Option<HashMap<String, String>>,
    ) -> anyhow::Result<()> {
        // NOTE: This block is specifically scoped to ensure we drop the read lock on `self.actors` before
        // we attempt to grab a write lock.
        let new_actor = {
            let actors = self.actors.read().await;
            let actor = actors.get(component_id).context("component not found")?;
            let annotations = annotations.unwrap_or_default().into_iter().collect();

            let new_actor = self.fetch_actor(new_component_ref).await?;
            let new_claims = new_actor.claims();
            if let Some(claims) = new_claims.cloned() {
                self.store_claims(Claims::Component(claims))
                    .await
                    .context("failed to store claims")?;
            }

            let max = actor.max_instances;
            let mut handler = actor.handler.copy_for_new();
            handler.polyfilled_imports = get_import_results(&new_actor);
            let Ok(new_actor) = self
                .instantiate_actor(
                    &annotations,
                    new_component_ref.to_string(),
                    component_id.to_string(),
                    max,
                    new_actor.clone(),
                    handler,
                )
                .await
            else {
                bail!("failed to instantiate component from new reference");
            };

            info!(%new_component_ref, "component updated");
            self.publish_event(
                "component_scaled",
                event::component_scaled(
                    new_claims,
                    &actor.annotations,
                    host_id,
                    max,
                    new_component_ref,
                    component_id,
                ),
            )
            .await?;

            // TODO(#1548): If this errors, we need to rollback
            self.stop_actor(actor, host_id)
                .await
                .context("failed to stop old component")?;
            // TODO: How to differentiate between old and new component if image reference is the same?
            self.publish_event(
                "component_scaled",
                event::component_scaled(
                    actor.claims(),
                    &actor.annotations,
                    host_id,
                    0_usize,
                    &actor.image_reference,
                    &actor.id,
                ),
            )
            .await?;

            new_actor
        };

        self.actors
            .write()
            .await
            .insert(component_id.to_string(), new_actor);
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

        let config = self
            .config_generator
            .generate(config)
            .await
            .context("Unable to fetch requested config")?;
        // TODO(#1648): Implement redelivery of changed configuration when `config.changed()` is true

        let host_id = host_id.to_string();
        spawn(async move {
            if let Err(err) = self
                .handle_start_provider_task(
                    config,
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
        config: ConfigBundle,
        provider_id: &str,
        provider_ref: &str,
        annotations: HashMap<String, String>,
        host_id: &str,
    ) -> anyhow::Result<()> {
        trace!(provider_ref, provider_id, "start provider task");

        let registry_config = self.registry_config.read().await;
        let (path, claims) = crate::fetch_provider(
            provider_ref,
            host_id,
            self.host_config.allow_file_load,
            &registry_config,
        )
        .await
        .context("failed to fetch provider")?;
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
            };
            let config_generator = self.config_generator.clone();

            // Prepare startup links by generating the source and target configs. Note that because the provider may be the source
            // or target of a link, we need to iterate over all links to find the ones that involve the provider.
            let link_definitions = stream::iter(self.links.read().await.values().flatten())
                .filter_map(|link| async {
                    if link.source_id == provider_id || link.target == provider_id {
                        if let Ok(provider_link) =
                            resolve_link_config(&config_generator, link.clone()).await
                        {
                            Some(provider_link)
                        } else {
                            error!(
                                provider_id,
                                source_id = link.source_id,
                                target = link.target,
                                "failed to resolve link config, skipping link"
                            );
                            None
                        }
                    } else {
                        None
                    }
                })
                .collect::<Vec<wasmcloud_core::InterfaceLinkDefinition>>()
                .await;

            let host_data = HostData {
                host_id: self.host_key.public_key(),
                lattice_rpc_prefix: self.host_config.lattice.clone(),
                link_name: "default".to_string(),
                lattice_rpc_user_jwt: self.host_config.rpc_jwt.clone().unwrap_or_default(),
                lattice_rpc_user_seed: lattice_rpc_user_seed.unwrap_or_default(),
                lattice_rpc_url: self.host_config.rpc_nats_url.to_string(),
                env_values: vec![],
                instance_id: Uuid::new_v4().to_string(),
                provider_key: provider_id.to_string(),
                link_definitions,
                config: config.get_config().await.clone(),
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

            // TODO: Change method receiver to Arc<Self> and `move` into the closure
            let rpc_nats = self.rpc_nats.clone();
            let ctl_nats = self.ctl_nats.clone();
            let event_builder = self.event_builder.clone();
            // NOTE: health_ prefix here is to allow us to move the variables into the closure
            let health_lattice = self.host_config.lattice.clone();
            let health_host_id = host_id.to_string();
            let health_provider_id = provider_id.to_string();
            let child = spawn(async move {
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
                        exit_status = child.wait() => match exit_status {
                            Ok(status) => {
                                debug!("`{}` exited with `{status:?}`", path.display());
                                break;
                            }
                            Err(e) => {
                                warn!("failed to wait for `{}` to execute: {e}", path.display());
                                break;
                            }
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
            entry.insert(Provider {
                child,
                annotations,
                claims,
                image_ref: provider_ref.to_string(),
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
            child, annotations, ..
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
        child.abort();
        info!(provider_id, "provider stopped");
        self.publish_event(
            "provider_stopped",
            event::provider_stopped(&annotations, host_id, provider_id, "stop"),
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

    #[instrument(level = "debug", skip_all)]
    async fn handle_claims(&self) -> anyhow::Result<CtlResponse<Vec<HashMap<String, String>>>> {
        trace!("handling claims");

        let (actor_claims, provider_claims) =
            join!(self.actor_claims.read(), self.provider_claims.read());
        let actor_claims = actor_claims.values().cloned().map(Claims::Component);
        let provider_claims = provider_claims.values().cloned().map(Claims::Provider);
        let claims: Vec<StoredClaims> = actor_claims
            .chain(provider_claims)
            .flat_map(TryFrom::try_from)
            .collect();

        Ok(CtlResponse::ok(
            claims.into_iter().map(std::convert::Into::into).collect(),
        ))
    }

    #[instrument(level = "debug", skip_all)]
    async fn handle_links(&self) -> anyhow::Result<Vec<u8>> {
        debug!("handling links");

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
    async fn handle_interface_link_put(
        &self,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<CtlResponse<()>> {
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

        // Before we store the link, we need to ensure the configuration is resolvable
        let provider_link =
            resolve_link_config(&self.config_generator, interface_link_definition.clone())
                .await
                .context("failed to resolve link config for provider")?;

        // Note here that unwrapping to a default is intentional. If the component spec doesn't exist, we want to create it
        // so that when that component does start it can use pre-existing links.
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

        self.put_provider_link(&source_id, &target, provider_link)
            .await?;

        Ok(CtlResponse::success())
    }

    #[instrument(level = "debug", skip_all)]
    /// Remove an interface link on a source component for a specific package
    async fn handle_interface_link_del(
        &self,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<CtlResponse<()>> {
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
            lattice: self.host_config.lattice.clone(),
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
                .handle_auction_actor(message.payload)
                .await
                .map(serialize_ctl_response),
            (Some("component"), Some("scale"), Some(host_id), None) => Arc::clone(&self)
                .handle_scale_actor(message.payload, host_id)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("component"), Some("update"), Some(host_id), None) => Arc::clone(&self)
                .handle_update_actor(message.payload, host_id)
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
                .handle_interface_link_del(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("link"), Some("get"), None, None) => {
                // Explicitly returning a Vec<u8> for non-cloning efficiency within handle_links
                self.handle_links().await.map(|bytes| Some(Ok(bytes)))
            }
            (Some("link"), Some("put"), None, None) => self
                .handle_interface_link_put(message.payload)
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

    /// Publishes a link to the lattice for all instances of a provider to handle
    /// Right now this is publishing _both_ to the source and the target in order to
    /// ensure that the provider is aware of the link. This would cause problems if a provider
    /// is linked to a provider (which it should never be.)
    #[instrument(level = "debug", skip(self, provider_link))]
    async fn put_provider_link(
        &self,
        source_id: &str,
        target: &str,
        provider_link: wasmcloud_core::InterfaceLinkDefinition,
    ) -> anyhow::Result<()> {
        let lattice = &self.host_config.lattice;
        let payload: Bytes = serde_json::to_vec(&provider_link)
            .context("failed to serialize provider link definition")?
            .into();
        let source_provider = self
            .rpc_nats
            .publish_with_headers(
                format!("wasmbus.rpc.{lattice}.{source_id}.linkdefs.put"),
                injector_to_headers(&TraceContextInjector::default_with_span()),
                payload.clone(),
            )
            .await
            .context("failed to publish provider link definition put");
        let target_provider = self
            .rpc_nats
            .publish_with_headers(
                format!("wasmbus.rpc.{lattice}.{target}.linkdefs.put"),
                injector_to_headers(&TraceContextInjector::default_with_span()),
                payload,
            )
            .await
            .context("failed to publish provider link definition put");
        source_provider?;
        target_provider?;
        Ok(())
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
                self.store_actor_claims(claims.clone()).await?;
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

        // If the actor is already running, update the links
        if let Some(actor) = self.actors.write().await.get(id) {
            *actor.handler.interface_links.write().await = component_import_links(&spec.links);
            // NOTE(brooksmtownsend): We can consider updating the actor if the image URL changes
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
        // TODO: TBD: stop actor if spec deleted?
        if let Some(_actor) = self.actors.write().await.get(id) {
            warn!("Component spec deleted but component {} still running", id);
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
            Claims::Component(claims) => self.store_actor_claims(claims).await,
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
                let mut actor_claims = self.actor_claims.write().await;
                actor_claims.remove(&claims.subject);
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
}

/// Transform a [`wasmcloud_control_interface::InterfaceLinkDefinition`] into a [`wasmcloud_core::InterfaceLinkDefinition`]
/// by generating the source and target config for the link
async fn resolve_link_config(
    config_generator: &BundleGenerator,
    link: wasmcloud_control_interface::InterfaceLinkDefinition,
) -> anyhow::Result<wasmcloud_core::InterfaceLinkDefinition> {
    let source_bundle = config_generator.generate(link.source_config).await?;
    let target_bundle = config_generator.generate(link.target_config).await?;

    let source_config = source_bundle.get_config().await;
    let target_config = target_bundle.get_config().await;
    Ok(wasmcloud_core::InterfaceLinkDefinition {
        source_id: link.source_id,
        target: link.target,
        name: link.name,
        wit_namespace: link.wit_namespace,
        wit_package: link.wit_package,
        interfaces: link.interfaces,
        source_config: source_config.clone(),
        target_config: target_config.clone(),
    })
}

/// Helper function to transform a Vec of [`InterfaceLinkDefinition`]s into the structure components expect to be able
/// to quickly look up the desired target for a given interface
///
/// # Arguments
/// - links: A Vec of [`InterfaceLinkDefinition`]s
///
/// # Returns
/// - A `HashMap` in the form of `link_name` -> namespace:package -> interface -> target
fn component_import_links(
    links: &[InterfaceLinkDefinition],
) -> HashMap<String, HashMap<String, HashMap<String, String>>> {
    links.iter().fold(HashMap::new(), |mut acc, link| {
        let ns_and_package = format!("{}:{}", link.wit_namespace, link.wit_package);

        acc.entry(link.name.clone())
            .and_modify(|link_for_name| {
                link_for_name
                    .entry(ns_and_package.clone())
                    .and_modify(|package| {
                        for interface in &link.interfaces {
                            package.insert(interface.clone(), link.target.clone());
                        }
                    })
                    .or_insert({
                        link.interfaces
                            .iter()
                            .map(|interface| (interface.clone(), link.target.clone()))
                            .collect::<HashMap<String, String>>()
                    });
            })
            .or_insert({
                let interfaces_map = link
                    .interfaces
                    .iter()
                    .map(|interface| (interface.clone(), link.target.clone()))
                    .collect::<HashMap<String, String>>();
                HashMap::from_iter([(ns_and_package.clone(), interfaces_map)])
            });
        acc
    })
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

fn get_import_results(
    component: &wasmcloud_runtime::Component,
) -> HashMap<String, HashMap<String, Arc<[wrpc_types::Type]>>> {
    let polyfilled_imports = component.polyfilled_imports().clone();
    // Map the imports to pull out the result types of the functions for lookup when invoking them
    polyfilled_imports
        .iter()
        .map(|(instance, funcs)| {
            (
                instance.clone(),
                funcs
                    .iter()
                    .filter_map(|(name, func)| {
                        match func {
                            DynamicFunction::Static { results, .. } => {
                                Some((name.clone(), results.clone()))
                            }
                            // We do not support method imports (on resources) at this time.
                            DynamicFunction::Method { .. } => None,
                        }
                    })
                    .collect::<HashMap<_, _>>(),
            )
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
                "default".to_string(),
                HashMap::from_iter([
                    (
                        "wasi:keyvalue".to_string(),
                        HashMap::from_iter([
                            ("atomics".to_string(), "kv-redis".to_string()),
                            ("store".to_string(), "kv-redis".to_string()),
                        ]),
                    ),
                    (
                        "wasi:http".to_string(),
                        HashMap::from_iter([
                            (
                                "incoming-handler".to_string(),
                                "source_component".to_string(),
                            ),
                            ("outgoing-handler".to_string(), "httpclient".to_string()),
                        ]),
                    ),
                    (
                        "custom:foo".to_string(),
                        HashMap::from_iter([
                            ("bar".to_string(), "other_component".to_string()),
                            ("baz".to_string(), "other_component".to_string()),
                        ]),
                    ),
                ]),
            ),
            (
                "secret".to_string(),
                HashMap::from_iter([(
                    "wasi:keyvalue".to_string(),
                    HashMap::from_iter([
                        ("atomics".to_string(), "kv-vault-offsite".to_string()),
                        ("store".to_string(), "kv-vault".to_string()),
                    ]),
                )]),
            ),
            (
                "link2".to_string(),
                HashMap::from_iter([(
                    "wit:package".to_string(),
                    HashMap::from_iter([("interface3".to_string(), "target".to_string())]),
                )]),
            ),
        ]);

        assert_eq!(links_map, expected_result);
    }
}
