/// Wasmbus lattice configuration
pub mod config;

pub use config::Lattice as LatticeConfig;

mod event;

use crate::fetch_actor;

use core::future::Future;
use core::num::NonZeroUsize;
use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;

use std::collections::{hash_map, BTreeMap, HashMap};
use std::env;
use std::env::consts::{ARCH, FAMILY, OS};
use std::io::Cursor;
use std::sync::Arc;

use anyhow::{anyhow, bail, ensure, Context as _};
use bytes::{BufMut, Bytes, BytesMut};
use cloudevents::{EventBuilder, EventBuilderV10};
use futures::stream::{AbortHandle, Abortable};
use futures::{stream, try_join, Stream, StreamExt, TryStreamExt};
use nkeys::{KeyPair, KeyPairType};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio::io::{stderr, AsyncWrite};
use tokio::sync::{watch, RwLock};
use tokio::time::{interval_at, Instant};
use tokio::{process, spawn};
use tokio_stream::wrappers::IntervalStream;
use tracing::{debug, error, info, instrument, trace, warn};
use ulid::Ulid;
use uuid::Uuid;
use wascap::jwt;
use wasmcloud_control_interface::{
    ActorAuctionRequest, ActorDescription, ProviderAuctionRequest, ProviderDescription,
    ScaleActorCommand, StartActorCommand, StartProviderCommand, StopActorCommand, StopHostCommand,
    StopProviderCommand, UpdateActorCommand,
};
use wasmcloud_runtime::{ActorInstancePool, Runtime};

const SUCCESS: &str = r#"{"accepted":true,"error":""}"#;

#[derive(Debug)]
struct Queue {
    auction: async_nats::Subscriber,
    commands: async_nats::Subscriber,
    pings: async_nats::Subscriber,
    inventory: async_nats::Subscriber,
    links: async_nats::Subscriber,
    queries: async_nats::Subscriber,
    registries: async_nats::Subscriber,
}

impl Stream for Queue {
    type Item = async_nats::Message;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut pending = false;
        match Pin::new(&mut self.commands).poll_next(cx) {
            Poll::Ready(Some(msg)) => return Poll::Ready(Some(msg)),
            Poll::Ready(None) => {}
            Poll::Pending => pending = true,
        }
        match Pin::new(&mut self.links).poll_next(cx) {
            Poll::Ready(Some(msg)) => return Poll::Ready(Some(msg)),
            Poll::Ready(None) => {}
            Poll::Pending => pending = true,
        }
        match Pin::new(&mut self.queries).poll_next(cx) {
            Poll::Ready(Some(msg)) => return Poll::Ready(Some(msg)),
            Poll::Ready(None) => {}
            Poll::Pending => pending = true,
        }
        match Pin::new(&mut self.registries).poll_next(cx) {
            Poll::Ready(Some(msg)) => return Poll::Ready(Some(msg)),
            Poll::Ready(None) => {}
            Poll::Pending => pending = true,
        }
        match Pin::new(&mut self.inventory).poll_next(cx) {
            Poll::Ready(Some(msg)) => return Poll::Ready(Some(msg)),
            Poll::Ready(None) => {}
            Poll::Pending => pending = true,
        }
        match Pin::new(&mut self.auction).poll_next(cx) {
            Poll::Ready(Some(msg)) => return Poll::Ready(Some(msg)),
            Poll::Ready(None) => {}
            Poll::Pending => pending = true,
        }
        match Pin::new(&mut self.pings).poll_next(cx) {
            Poll::Ready(Some(msg)) => return Poll::Ready(Some(msg)),
            Poll::Ready(None) => {}
            Poll::Pending => pending = true,
        }
        if pending {
            Poll::Pending
        } else {
            Poll::Ready(None)
        }
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
        cluster_key: &KeyPair,
        host_key: &KeyPair,
    ) -> anyhow::Result<Self> {
        let (registries, pings, links, queries, auction, commands, inventory) = try_join!(
            nats.subscribe("wasmbus.ctl.default.registries.put".into()),
            nats.subscribe("wasmbus.ctl.default.ping.hosts".into()),
            nats.subscribe("wasmbus.ctl.default.linkdefs.*".into()),
            nats.subscribe("wasmbus.ctl.default.get.*".into()),
            nats.subscribe("wasmbus.ctl.default.auction.>".into()),
            nats.subscribe(format!(
                "wasmbus.ctl.default.cmd.{}.*",
                host_key.public_key()
            )),
            nats.subscribe(format!(
                "wasmbus.ctl.default.get.{}.inv",
                host_key.public_key()
            )),
        )
        .context("failed to subscribe to queues")?;
        Ok(Self {
            auction,
            commands,
            pings,
            inventory,
            links,
            queries,
            registries,
        })
    }
}

#[derive(Debug)]
struct ActorInstance {
    nats: async_nats::Client,
    pool: ActorInstancePool,
    id: Ulid,
    calls: AbortHandle,
}

impl ActorInstance {
    #[instrument(skip(payload))]
    async fn handle_call(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<Bytes> {
        #[allow(unused)] // TODO: Use fields or remove
        #[derive(Debug, Deserialize)]
        struct Entity {
            link_name: String,
            contract_id: String,
            public_key: String,
        }
        #[allow(unused)] // TODO: Use fields or remove
        #[derive(Debug, Deserialize)]
        struct Invocation {
            origin: Entity,
            target: Entity,
            #[serde(default)]
            operation: String,
            #[serde(with = "serde_bytes")]
            #[serde(default)]
            msg: Vec<u8>,
            #[serde(default)]
            id: String,
            #[serde(default)]
            encoded_claims: String,
            #[serde(default)]
            host_id: String,
            #[serde(default)]
            content_length: Option<u64>,
            #[serde(rename = "traceContext")]
            #[serde(default)]
            trace_context: HashMap<String, String>,
        }
        #[derive(Default, Serialize)]
        struct InvocationResponse {
            #[serde(with = "serde_bytes")]
            msg: Vec<u8>,
            invocation_id: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            error: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            content_length: Option<u64>,
        }
        let Invocation {
            origin,
            target,
            operation,
            msg,
            id: invocation_id,
            ..
        } = rmp_serde::from_slice(payload.as_ref()).context("failed to decode invocation")?;

        debug!(?origin, ?target, operation, "handle actor invocation");

        let res = AsyncBytesMut::default();
        let mut instance = self
            .pool
            .instantiate()
            .await
            .context("failed to instantiate actor")?;
        let res = match instance
            .stderr(stderr())
            .await
            .context("failed to set stderr")?
            .call(operation, Cursor::new(msg), res.clone())
            .await
            .context("failed to call actor")?
        {
            Ok(()) => {
                let msg: Vec<_> = res.try_into()?;
                let content_length = msg.len().try_into().ok();
                InvocationResponse {
                    msg,
                    invocation_id,
                    content_length,
                    ..Default::default()
                }
            }
            Err(e) => InvocationResponse {
                invocation_id,
                error: Some(e),
                ..Default::default()
            },
        };
        rmp_serde::to_vec_named(&res)
            .map(Into::into)
            .context("failed to encode response")
    }

    #[instrument]
    async fn handle_message(
        &self,
        async_nats::Message {
            reply,
            payload,
            subject,
            ..
        }: async_nats::Message,
    ) {
        let res = self.handle_call(payload).await;
        match (reply, res) {
            (Some(reply), Ok(buf)) => {
                if let Err(e) = self.nats.publish(reply, buf).await {
                    error!("failed to publish response to `{subject}` request: {e:?}");
                }
            }
            (_, Err(e)) => {
                warn!("failed to handle `{subject}` request: {e:?}");
            }
            _ => {}
        }
    }
}

type Annotations = BTreeMap<String, String>;

#[derive(Debug)]
struct Actor {
    pool: ActorInstancePool,
    instances: RwLock<HashMap<Option<Annotations>, Vec<Arc<ActorInstance>>>>,
    image_ref: String,
}

#[derive(Debug)]
struct ProviderInstance {
    child: process::Child,
    id: Ulid,
    annotations: Option<Annotations>,
}

#[derive(Debug)]
struct Provider {
    claims: jwt::Claims<jwt::CapabilityProvider>,
    instances: HashMap<String, ProviderInstance>,
    image_ref: String,
}

/// Wasmbus lattice
#[derive(Debug)]
pub struct Lattice {
    // TODO: Clean up actors after stop
    actors: RwLock<HashMap<String, Arc<Actor>>>,
    cluster_key: KeyPair,
    event_builder: EventBuilderV10,
    friendly_name: String,
    heartbeat: AbortHandle,
    host_key: KeyPair,
    labels: HashMap<String, String>,
    nats: async_nats::Client,
    providers: RwLock<HashMap<String, Provider>>,
    runtime: Runtime,
    start_at: Instant,
    stop_tx: watch::Sender<Option<Instant>>,
    stop_rx: watch::Receiver<Option<Instant>>,
    queue: AbortHandle,
}

impl Lattice {
    const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

    /// Construct a new [Lattice] returning a tuple of it's [Arc] and an async shutdown function.
    #[instrument]
    pub async fn new(
        LatticeConfig {
            url,
            cluster_seed,
            host_seed,
        }: LatticeConfig,
    ) -> anyhow::Result<(Arc<Self>, impl Future<Output = anyhow::Result<()>>)> {
        let cluster_key = if let Some(cluster_seed) = cluster_seed {
            let kp = KeyPair::from_seed(&cluster_seed)
                .context("failed to construct key pair from seed")?;
            ensure!(kp.key_pair_type() == KeyPairType::Cluster);
            kp
        } else {
            KeyPair::new(KeyPairType::Cluster)
        };
        let host_key = if let Some(host_seed) = host_seed {
            let kp =
                KeyPair::from_seed(&host_seed).context("failed to construct key pair from seed")?;
            ensure!(kp.key_pair_type() == KeyPairType::Server);
            kp
        } else {
            KeyPair::new(KeyPairType::Server)
        };

        let mut labels = HashMap::from([
            ("hostcore.arch".into(), ARCH.into()),
            ("hostcore.os".into(), OS.into()),
            ("hostcore.osfamily".into(), FAMILY.into()),
        ]);
        labels.extend(env::vars().filter_map(|(k, v)| {
            let k = k.strip_prefix("HOST_")?;
            Some((k.to_lowercase(), v))
        }));
        let friendly_name = names::Generator::default()
            .next()
            .context("failed to generate friendly name")?;

        let start_evt = json!({
            "friendly_name": friendly_name,
            "labels": labels,
            "uptime_seconds": 0,
            "version": env!("CARGO_PKG_VERSION"),
        });

        let nats = async_nats::connect(url.as_str())
            .await
            .context("failed to connect to NATS")?;

        let queue = Queue::new(&nats, &cluster_key, &host_key)
            .await
            .context("failed to initialize queue")?;
        nats.flush().await.context("failed to flush")?;

        let start_at = Instant::now();

        let heartbeat_start_at = start_at
            .checked_add(Self::HEARTBEAT_INTERVAL)
            .context("failed to compute heartbeat start time")?;
        let heartbeat =
            IntervalStream::new(interval_at(heartbeat_start_at, Self::HEARTBEAT_INTERVAL));

        let (stop_tx, stop_rx) = watch::channel(None);

        let (queue_abort, queue_abort_reg) = AbortHandle::new_pair();
        let (heartbeat_abort, heartbeat_abort_reg) = AbortHandle::new_pair();

        // TODO: Configure
        let runtime = Runtime::builder()
            .actor_config(wasmcloud_runtime::ActorConfig {
                require_signature: true,
            })
            .build()
            .context("failed to build runtime")?;
        let event_builder = EventBuilderV10::new().source(host_key.public_key());
        let wasmbus = Lattice {
            actors: RwLock::default(),
            cluster_key,
            event_builder,
            friendly_name,
            heartbeat: heartbeat_abort.clone(),
            host_key,
            labels,
            nats,
            providers: RwLock::default(),
            runtime,
            start_at,
            stop_rx,
            stop_tx,
            queue: queue_abort.clone(),
        };
        wasmbus
            .publish_event("host_started", start_evt)
            .await
            .context("failed to publish start event")?;
        info!("host {} started", wasmbus.host_key.public_key());

        let wasmbus = Arc::new(wasmbus);
        let queue = spawn({
            let wasmbus = Arc::clone(&wasmbus);
            async {
                Abortable::new(queue, queue_abort_reg)
                    .for_each(move |msg| {
                        let wasmbus = Arc::clone(&wasmbus);
                        async move { wasmbus.handle_message(msg).await }
                    })
                    .await;
            }
        });
        let heartbeat = spawn({
            let wasmbus = Arc::clone(&wasmbus);
            Abortable::new(heartbeat, heartbeat_abort_reg).for_each(move |_| {
                let wasmbus = Arc::clone(&wasmbus);
                async move {
                    let heartbeat = wasmbus.heartbeat().await;
                    if let Err(e) = wasmbus.publish_event("host_heartbeat", heartbeat).await {
                        error!("failed to publish heartbeat: {e}");
                    }
                }
            })
        });
        Ok((Arc::clone(&wasmbus), async move {
            heartbeat_abort.abort();
            queue_abort.abort();
            try_join!(heartbeat, queue).context("failed to await tasks")?;
            wasmbus
                .publish_event(
                    "host_stopped",
                    json!({
                        "labels": wasmbus.labels,
                    }),
                )
                .await
                .context("failed to publish stop event")
        }))
    }

    /// Waits for host to be stopped via lattice commands and returns the shutdown deadline on
    /// success
    ///
    /// # Errors
    ///
    /// Returns an error if internal stop channel is closed prematurely
    pub async fn stopped(&self) -> anyhow::Result<Option<Instant>> {
        self.stop_rx
            .clone()
            .changed()
            .await
            .context("failed to wait for stop")?;
        Ok(*self.stop_rx.borrow())
    }

    async fn heartbeat(&self) -> serde_json::Value {
        let actors = self.actors.read().await;
        let actors: HashMap<&String, usize> = stream::iter(actors.iter())
            .filter_map(|(id, actor)| async move {
                let instances = actor.instances.read().await;
                let count = instances.values().map(Vec::len).sum();
                if count == 0 {
                    None
                } else {
                    Some((id, count))
                }
            })
            .collect()
            .await;
        let providers: Vec<_> = self
            .providers
            .read()
            .await
            .iter()
            .flat_map(
                |(
                    public_key,
                    Provider {
                        claims, instances, ..
                    },
                )| {
                    instances.keys().map(move |link_name| {
                        let metadata = claims.metadata.as_ref();
                        let contract_id =
                            metadata.map(|jwt::CapabilityProvider { capid, .. }| capid.as_str());
                        json!({
                            "public_key": public_key,
                            "link_name": link_name,
                            "contract_id": contract_id.unwrap_or("n/a"),
                        })
                    })
                },
            )
            .collect();
        let uptime = self.start_at.elapsed();
        json!({
            "actors": actors,
            "friendly_name": self.friendly_name,
            "labels": self.labels,
            "providers": providers,
            "uptime_human": "TODO", // TODO
            "uptime_seconds": uptime.as_secs(),
            "version": env!("CARGO_PKG_VERSION"),
        })
    }

    #[instrument(skip(name))]
    async fn publish_event(
        &self,
        name: impl AsRef<str>,
        data: serde_json::Value,
    ) -> anyhow::Result<()> {
        let name = name.as_ref();
        let name = format!("com.wasmcloud.lattice.{name}");
        let now = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .context("failed to format current time")?;
        let ev = self
            .event_builder
            .clone()
            .ty(&name)
            .id(Uuid::from_u128(Ulid::new().into()).to_string())
            .time(now)
            .data("application/json", data)
            .build()
            .context("failed to build cloud event")?;
        trace!(?ev, "serialize event");
        let ev = serde_json::to_vec(&ev).context("failed to serialize event")?;
        trace!(?ev, "publish event");
        self.nats
            .publish("wasmbus.evt.default".into(), ev.into())
            .await
            .with_context(|| format!("failed to publish `{name}` event"))
    }

    /// Instantiate an actor and publish the actor start events.
    #[instrument(skip(host_id, actor_ref))]
    async fn instantiate_actor(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        annotations: &Option<Annotations>,
        host_id: impl AsRef<str>,
        actor_ref: impl AsRef<str>,
        count: NonZeroUsize,
        pool: ActorInstancePool,
    ) -> anyhow::Result<Vec<Arc<ActorInstance>>> {
        let actor_ref = actor_ref.as_ref();
        let instances = stream::repeat(format!("wasmbus.rpc.default.{}", claims.subject))
            .take(count.into())
            .then(|topic| {
                let pool = pool.clone();
                async move {
                    let calls = self
                        .nats
                        .queue_subscribe(topic.clone(), topic)
                        .await
                        .context("failed to subscribe to actor call queue")?;

                    let (calls_abort, calls_abort_reg) = AbortHandle::new_pair();
                    let id = Ulid::new();
                    let instance = Arc::new(ActorInstance {
                        nats: self.nats.clone(),
                        pool,
                        id,
                        calls: calls_abort,
                    });

                    let _calls = spawn({
                        let instance = Arc::clone(&instance);
                        Abortable::new(calls, calls_abort_reg).for_each_concurrent(
                            None,
                            move |msg| {
                                let instance = Arc::clone(&instance);
                                async move { instance.handle_message(msg).await }
                            },
                        )
                    });

                    self.publish_event(
                        "actor_started",
                        event::actor_started(
                            claims,
                            annotations,
                            Uuid::from_u128(id.into()),
                            actor_ref,
                        ),
                    )
                    .await?;
                    anyhow::Result::<_>::Ok(instance)
                }
            })
            .try_collect()
            .await
            .context("failed to instantiate actor")?;
        self.publish_event(
            "actors_started",
            event::actors_started(claims, annotations, host_id, count, actor_ref),
        )
        .await?;
        Ok(instances)
    }

    /// Uninstantiate an actor and publish the actor stop events.
    #[instrument(skip(host_id))]
    async fn uninstantiate_actor(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        annotations: &Option<Annotations>,
        host_id: impl AsRef<str>,
        instances: &mut Vec<Arc<ActorInstance>>,
        count: NonZeroUsize,
        remaining: usize,
    ) -> anyhow::Result<()> {
        stream::iter(instances.drain(..usize::from(count)))
            .map(Ok)
            .try_for_each_concurrent(None, |instance| {
                instance.calls.abort();
                async move {
                    self.publish_event(
                        "actor_stopped",
                        event::actor_stopped(
                            claims,
                            annotations,
                            Uuid::from_u128(instance.id.into()),
                        ),
                    )
                    .await
                }
            })
            .await?;
        self.publish_event(
            "actors_stopped",
            event::actors_stopped(claims, annotations, host_id, count, remaining),
        )
        .await
    }

    #[instrument(skip(host_id, annotations))]
    async fn start_actor<'a>(
        &self,
        entry: hash_map::VacantEntry<'a, String, Arc<Actor>>,
        actor: wasmcloud_runtime::Actor,
        actor_ref: String,
        count: NonZeroUsize,
        host_id: impl AsRef<str>,
        annotations: Option<impl Into<Annotations>>,
    ) -> anyhow::Result<&'a mut Arc<Actor>> {
        let annotations = annotations.map(Into::into);
        let claims = actor.claims().context("claims missing")?;

        let pool = ActorInstancePool::new(actor.clone(), Some(count));
        let instances = self
            .instantiate_actor(
                claims,
                &annotations,
                host_id,
                &actor_ref,
                count,
                pool.clone(),
            )
            .await
            .context("failed to instantiate actor")?;
        let actor = Arc::new(Actor {
            pool,
            instances: RwLock::new(HashMap::from([(annotations, instances)])),
            image_ref: actor_ref,
        });
        Ok(entry.insert(actor))
    }

    #[instrument(skip(host_id))]
    async fn stop_actor<'a>(
        &self,
        entry: hash_map::OccupiedEntry<'a, String, Arc<Actor>>,
        host_id: impl AsRef<str>,
    ) -> anyhow::Result<()> {
        let host_id = host_id.as_ref();
        let actor = entry.remove();
        let claims = actor.pool.claims().context("claims missing")?;
        let mut instances = actor.instances.write().await;
        let remaining: usize = instances.values().map(Vec::len).sum();
        stream::iter(instances.drain())
            .map(anyhow::Result::<_>::Ok)
            .try_fold(
                remaining,
                |remaining, (annotations, mut instances)| async move {
                    let Some(count) = NonZeroUsize::new(instances.len()) else {
                        return Ok(remaining)
                    };
                    let remaining = remaining
                        .checked_sub(count.into())
                        .context("invalid instance length")?;
                    self.uninstantiate_actor(
                        claims,
                        &annotations,
                        host_id,
                        &mut instances,
                        count,
                        remaining,
                    )
                    .await
                    .context("failed to uninstantiate actor")?;
                    Ok(remaining)
                },
            )
            .await?;
        Ok(())
    }

    #[instrument(skip(payload))]
    async fn handle_auction_actor(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<Bytes> {
        let ActorAuctionRequest {
            actor_ref,
            constraints,
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize actor auction command")?;

        debug!(actor_ref, ?constraints, "auction actor");

        // TODO: Use `ActorAuctionAck` once https://github.com/wasmCloud/control-interface-client/issues/50 is resolved
        let buf = serde_json::to_vec(&json!({
          "actor_ref": actor_ref,
          "constraints": constraints,
          "host_id": self.host_key.public_key(),
        }))
        .context("failed to encode reply")?;
        Ok(buf.into())
    }

    #[instrument(skip(payload))]
    async fn handle_auction_provider(
        &self,
        payload: impl AsRef<[u8]>,
    ) -> anyhow::Result<Option<Bytes>> {
        let ProviderAuctionRequest {
            provider_ref,
            constraints,
            link_name,
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize provider auction command")?;

        debug!(provider_ref, link_name, ?constraints, "auction provider");

        let providers = self.providers.read().await;
        if providers.values().any(
            |Provider {
                 image_ref,
                 instances,
                 ..
             }| { *image_ref == provider_ref && instances.contains_key(&link_name) },
        ) {
            // Do not reply if the provider is already running
            return Ok(None);
        }

        // TODO: ProviderAuctionAck is missing `constraints` field sent by OTP.
        // Either replace this by ProviderAuctionAck or update upstream.
        let buf = serde_json::to_vec(&json!({
          "provider_ref": provider_ref,
          "link_name": link_name,
          "constraints": constraints,
          "host_id": self.host_key.public_key(),
        }))
        .context("failed to encode reply")?;
        Ok(Some(buf.into()))
    }

    #[instrument(skip(payload))]
    async fn handle_stop(&self, payload: impl AsRef<[u8]>, host_id: &str) -> anyhow::Result<Bytes> {
        let StopHostCommand { timeout, .. } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize stop command")?;

        debug!(?timeout, "stop host");

        self.heartbeat.abort();
        self.queue.abort();
        let deadline =
            timeout.and_then(|timeout| Instant::now().checked_add(Duration::from_millis(timeout)));
        self.stop_tx.send_replace(deadline);
        Ok(SUCCESS.into())
    }

    #[instrument(skip(payload))]
    async fn handle_scale_actor(
        &self,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<Bytes> {
        let ScaleActorCommand {
            actor_id,
            actor_ref,
            count,
            annotations,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize actor scale command")?;

        debug!(actor_id, actor_ref, count, "scale actor");

        let actor_id = if actor_id.is_empty() {
            let actor = fetch_actor(&actor_ref)
                .await
                .context("failed to fetch actor")?;
            let actor = wasmcloud_runtime::Actor::new(&self.runtime, actor)
                .context("failed to initialize actor")?;
            actor.claims().context("claims missing")?.subject.clone()
        } else {
            actor_id
        };

        let annotations = annotations.map(|annotations| annotations.into_iter().collect());
        match (
            self.actors.write().await.entry(actor_id),
            NonZeroUsize::new(count.into()),
        ) {
            (hash_map::Entry::Vacant(_), None) => {}
            (hash_map::Entry::Vacant(entry), Some(count)) => {
                let actor = fetch_actor(&actor_ref)
                    .await
                    .context("failed to fetch actor")?;
                let actor = wasmcloud_runtime::Actor::new(&self.runtime, actor)
                    .context("failed to initialize actor")?;
                self.start_actor(entry, actor, actor_ref, count, host_id, annotations)
                    .await?;
            }
            (hash_map::Entry::Occupied(entry), None) => {
                self.stop_actor(entry, host_id).await?;
            }
            (hash_map::Entry::Occupied(entry), Some(count)) => {
                let actor = entry.get();
                let mut instances = actor.instances.write().await;
                let count = usize::from(count);
                let current = instances.values().map(Vec::len).sum();
                let claims = actor.pool.claims().context("claims missing")?;
                if let Some(delta) = count.checked_sub(current).and_then(NonZeroUsize::new) {
                    let mut delta = self
                        .instantiate_actor(
                            claims,
                            &annotations,
                            host_id,
                            &actor.image_ref,
                            delta,
                            actor.pool.clone(),
                        )
                        .await
                        .context("failed to instantiate actor")?;
                    instances.entry(annotations).or_default().append(&mut delta);
                } else if let Some(delta) = current.checked_sub(count).and_then(NonZeroUsize::new) {
                    let mut remaining = current;
                    let mut delta = usize::from(delta);
                    for (annotations, instances) in instances.iter_mut() {
                        let Some(count) = NonZeroUsize::new(instances.len().min(delta)) else {
                            continue;
                        };
                        remaining = remaining
                            .checked_sub(count.into())
                            .context("invalid instance length")?;
                        delta = delta.checked_sub(count.into()).context("invalid delta")?;
                        self.uninstantiate_actor(
                            claims,
                            annotations,
                            host_id,
                            instances,
                            count,
                            remaining,
                        )
                        .await
                        .context("failed to uninstantiate actor")?;
                        if delta == 0 {
                            break;
                        }
                    }
                    if remaining == 0 {
                        drop(instances);
                        entry.remove();
                    }
                }
            }
        }
        Ok(SUCCESS.into())
    }

    #[instrument(skip(payload))]
    async fn handle_launch_actor(
        &self,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<Bytes> {
        let StartActorCommand {
            actor_ref,
            annotations,
            count,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize actor launch command")?;
        debug!(actor_ref, ?count, ?annotations, "launch actor");

        let actor = fetch_actor(&actor_ref)
            .await
            .context("failed to fetch actor")?;
        let actor = wasmcloud_runtime::Actor::new(&self.runtime, actor)
            .context("failed to initialize actor")?;
        let claims = actor.claims().context("claims missing")?;

        let annotations = annotations.map(|annotations| annotations.into_iter().collect());
        let Some(count) = NonZeroUsize::new(count.into()) else {
            // NOTE: This mimics OTP behavior
            self.publish_event(
                "actors_started",
                event::actors_started(claims, &annotations, host_id, 0usize, actor_ref),
            )
            .await?;
            return Ok(SUCCESS.into())
        };

        match self.actors.write().await.entry(claims.subject.clone()) {
            hash_map::Entry::Vacant(entry) => {
                self.start_actor(entry, actor, actor_ref, count, host_id, annotations)
                    .await?;
            }
            hash_map::Entry::Occupied(entry) => {
                let actor = entry.get();
                let mut instances = actor.instances.write().await;
                let claims = actor.pool.claims().context("claims missing")?;
                let mut delta = self
                    .instantiate_actor(
                        claims,
                        &annotations,
                        host_id,
                        &actor.image_ref,
                        count,
                        actor.pool.clone(),
                    )
                    .await
                    .context("failed to instantiate actor")?;
                instances.entry(annotations).or_default().append(&mut delta);
            }
        }
        Ok(SUCCESS.into())
    }

    #[instrument(skip(payload))]
    async fn handle_stop_actor(
        &self,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<Bytes> {
        let StopActorCommand {
            actor_ref,
            count,
            annotations,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize actor stop command")?;

        debug!(actor_ref, count, ?annotations, "stop actor");

        let annotations = annotations.map(|annotations| annotations.into_iter().collect());
        match (
            self.actors.write().await.entry(actor_ref),
            NonZeroUsize::new(count.into()),
        ) {
            (hash_map::Entry::Occupied(entry), None) => {
                self.stop_actor(entry, host_id).await?;
            }
            (hash_map::Entry::Occupied(entry), Some(count)) => {
                let actor = entry.get();
                let claims = actor.pool.claims().context("claims missing")?;
                let mut instances = actor.instances.write().await;
                if let hash_map::Entry::Occupied(mut entry) = instances.entry(annotations.clone()) {
                    let instances = entry.get_mut();
                    let remaining = instances.len().saturating_sub(count.into());
                    self.uninstantiate_actor(
                        claims,
                        &annotations,
                        host_id,
                        instances,
                        count,
                        remaining,
                    )
                    .await
                    .context("failed to uninstantiate actor")?;
                    if remaining == 0 {
                        entry.remove();
                    }
                }
                if instances.len() == 0 {
                    drop(instances);
                    entry.remove();
                }
            }
            _ => {
                // NOTE: This mimics OTP behavior
                // TODO: What does OTP do?
            }
        }
        Ok(SUCCESS.into())
    }

    #[instrument(skip(payload))]
    async fn handle_update_actor(
        &self,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<Bytes> {
        let UpdateActorCommand {
            actor_id,
            annotations,
            new_actor_ref,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize actor update command")?;

        debug!(actor_id, new_actor_ref, ?annotations, "update actor");

        bail!("TODO");
    }

    #[instrument(skip(payload))]
    async fn handle_launch_provider(
        &self,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<Bytes> {
        let StartProviderCommand {
            configuration,
            link_name,
            provider_ref,
            annotations,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize provider launch command")?;

        debug!(
            configuration,
            link_name,
            provider_ref,
            ?annotations,
            "launch provider"
        );

        let (path, claims) = crate::fetch_provider(&provider_ref, &link_name)
            .await
            .context("failed to fetch provider")?;

        let annotations = annotations.map(|annotations| annotations.into_iter().collect());
        let mut providers = self.providers.write().await;
        let Provider { instances, .. } =
            providers.entry(claims.subject.clone()).or_insert(Provider {
                claims: claims.clone(),
                image_ref: provider_ref.clone(),
                instances: HashMap::default(),
            });
        if let hash_map::Entry::Vacant(entry) = instances.entry(link_name.clone()) {
            debug!(?path, "spawn provider process");
            let child = process::Command::new(&path)
                .kill_on_drop(true)
                .spawn()
                .context("failed to spawn provider process")?;
            let id = Ulid::new();
            self.publish_event(
                "provider_started",
                event::provider_started(
                    &claims,
                    &annotations,
                    Uuid::from_u128(id.into()),
                    host_id,
                    provider_ref,
                    link_name,
                ),
            )
            .await?;
            entry.insert(ProviderInstance {
                child,
                id,
                annotations,
            });
        } else {
            bail!("provider is already running")
        }
        Ok(SUCCESS.into())
    }

    #[instrument(skip(payload))]
    async fn handle_stop_provider(
        &self,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<Bytes> {
        let StopProviderCommand {
            annotations,
            contract_id,
            link_name,
            provider_ref,
            ..
        } = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize provider stop command")?;

        debug!(
            link_name,
            provider_ref,
            contract_id,
            ?annotations,
            "stop provider"
        );

        let annotations = annotations.map(|annotations| annotations.into_iter().collect());
        let mut providers = self.providers.write().await;
        let hash_map::Entry::Occupied(mut entry) = providers.entry(provider_ref) else {
            return Ok(SUCCESS.into());
        };
        let provider = entry.get_mut();
        let instances = &mut provider.instances;
        if let hash_map::Entry::Occupied(entry) = instances.entry(link_name.clone()) {
            if entry.get().annotations == annotations {
                let ProviderInstance { id, mut child, .. } = entry.remove();
                child.kill().await.context("failed to kill child")?;
                self.publish_event(
                    "provider_stopped",
                    event::provider_stopped(
                        &provider.claims,
                        &annotations,
                        Uuid::from_u128(id.into()),
                        host_id,
                        link_name,
                        "stop",
                    ),
                )
                .await?;
            }
        }
        if instances.is_empty() {
            entry.remove();
        }
        Ok(SUCCESS.into())
    }

    #[instrument(skip(_payload))]
    async fn handle_inventory(
        &self,
        _payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<Bytes> {
        let actors = self.actors.read().await;
        let actors: Vec<_> = stream::iter(actors.iter())
            .filter_map(|(id, actor)| async move {
                let instances = actor.instances.read().await;
                let instances: Vec<_> = instances
                    .iter()
                    .flat_map(|(annotations, instances)| {
                        instances.iter().map(move |actor| {
                            let instance_id = Uuid::from_u128(actor.id.into()).to_string();
                            let revision = actor
                                .pool
                                .claims()
                                .and_then(|claims| claims.metadata.as_ref())
                                .and_then(|jwt::Actor { rev, .. }| *rev)
                                .unwrap_or_default();
                            let annotations = annotations
                                .as_ref()
                                .map(|annotations| annotations.clone().into_iter().collect());
                            wasmcloud_control_interface::ActorInstance {
                                annotations,
                                instance_id,
                                revision,
                            }
                        })
                    })
                    .collect();
                if instances.is_empty() {
                    return None;
                }
                let name = actor
                    .pool
                    .claims()
                    .and_then(|claims| claims.metadata.as_ref())
                    .and_then(|metadata| metadata.name.as_ref())
                    .cloned();
                Some(ActorDescription {
                    id: id.into(),
                    image_ref: Some(actor.image_ref.clone()),
                    instances,
                    name,
                })
            })
            .collect()
            .await;
        let providers = self.providers.read().await;
        let providers: Vec<_> = providers
            .iter()
            .filter_map(
                |(
                    id,
                    Provider {
                        claims,
                        instances,
                        image_ref,
                        ..
                    },
                )| {
                    let jwt::CapabilityProvider {
                        capid: contract_id,
                        name,
                        rev: revision,
                        ..
                    } = claims.metadata.as_ref()?;
                    Some(instances.iter().map(
                        move |(link_name, ProviderInstance { annotations, .. })| {
                            let annotations = annotations
                                .as_ref()
                                .map(|annotations| annotations.clone().into_iter().collect());
                            let revision = revision.unwrap_or_default();
                            ProviderDescription {
                                id: id.into(),
                                image_ref: Some(image_ref.clone()),
                                contract_id: contract_id.clone(),
                                link_name: link_name.into(),
                                name: name.clone(),
                                annotations,
                                revision,
                            }
                        },
                    ))
                },
            )
            .flatten()
            .collect();
        // TODO: Use `HostInventory` once https://github.com/wasmCloud/control-interface-client/issues/51 is resolved
        let buf = serde_json::to_vec(&json!({
          "host_id": self.host_key.public_key(),
          "issuer": self.cluster_key.public_key(),
          "labels": self.labels,
          "friendly_name": self.friendly_name,
          "actors": actors,
          "providers": providers,
        }))
        .context("failed to encode reply")?;
        Ok(buf.into())
    }

    #[allow(unused)] // TODO: Remove once implemented
    #[instrument(skip(payload))]
    async fn handle_claims(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<Bytes> {
        bail!("TODO")
    }

    #[allow(unused)] // TODO: Remove once implemented
    #[instrument(skip(payload))]
    async fn handle_links(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<Bytes> {
        bail!("TODO")
    }

    #[allow(unused)] // TODO: Remove once implemented
    #[instrument(skip(payload))]
    async fn handle_linkdef_put(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<Bytes> {
        bail!("TODO")
    }

    #[allow(unused)] // TODO: Remove once implemented
    #[instrument(skip(payload))]
    async fn handle_linkdef_del(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<Bytes> {
        bail!("TODO")
    }

    #[allow(unused)] // TODO: Remove once implemented
    #[instrument(skip(payload))]
    async fn handle_registries_put(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<Bytes> {
        bail!("TODO")
    }

    #[instrument(skip(_payload))]
    async fn handle_ping_hosts(&self, _payload: impl AsRef<[u8]>) -> anyhow::Result<Bytes> {
        let uptime = self.start_at.elapsed();
        // TODO: Fill in the TODOs
        let buf = serde_json::to_vec(&json!({
          "id": self.host_key.public_key(),
          "issuer": self.cluster_key.public_key(),
          "labels": self.labels,
          "friendly_name": self.friendly_name,
          "uptime_seconds": uptime.as_secs(),
          "uptime_human": "TODO",
          "version": env!("CARGO_PKG_VERSION"),
          "cluster_issuers": "TODO",
          "js_domain": "TODO",
          "ctl_host": "TODO",
          "prov_rpc_host": "TODO",
          "rpc_host": "TODO",
          "lattice_prefix": "default",
        }))
        .context("failed to encode reply")?;
        Ok(buf.into())
    }

    #[instrument]
    async fn handle_message(
        &self,
        async_nats::Message {
            subject,
            reply,
            payload,
            headers,
            status,
            description,
            ..
        }: async_nats::Message,
    ) {
        let mut parts = subject.split('.').skip(3); // skip `wasmbus.ctl.{prefix}`
        let res = match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some("auction"), Some("actor"), None, None) => {
                self.handle_auction_actor(payload).await.map(Some)
            }
            (Some("auction"), Some("provider"), None, None) => {
                self.handle_auction_provider(payload).await
            }
            (Some("cmd"), Some(host_id), Some("la"), None) => {
                self.handle_launch_actor(payload, host_id).await.map(Some)
            }
            (Some("cmd"), Some(host_id), Some("lp"), None) => self
                .handle_launch_provider(payload, host_id)
                .await
                .map(Some),
            (Some("cmd"), Some(host_id), Some("sa"), None) => {
                self.handle_stop_actor(payload, host_id).await.map(Some)
            }
            (Some("cmd"), Some(host_id), Some("scale"), None) => {
                self.handle_scale_actor(payload, host_id).await.map(Some)
            }
            (Some("cmd"), Some(host_id), Some("sp"), None) => {
                self.handle_stop_provider(payload, host_id).await.map(Some)
            }
            (Some("cmd"), Some(host_id), Some("stop"), None) => {
                self.handle_stop(payload, host_id).await.map(Some)
            }
            (Some("cmd"), Some(host_id), Some("update"), None) => {
                self.handle_update_actor(payload, host_id).await.map(Some)
            }
            (Some("get"), Some(host_id), Some("inv"), None) => {
                self.handle_inventory(payload, host_id).await.map(Some)
            }
            (Some("get"), Some("claims"), None, None) => {
                self.handle_claims(payload).await.map(Some)
            }
            (Some("get"), Some("links"), None, None) => self.handle_links(payload).await.map(Some),
            (Some("linkdefs"), Some("put"), None, None) => {
                self.handle_linkdef_put(payload).await.map(Some)
            }
            (Some("linkdefs"), Some("del"), None, None) => {
                self.handle_linkdef_del(payload).await.map(Some)
            }
            (Some("registries"), Some("put"), None, None) => {
                self.handle_registries_put(payload).await.map(Some)
            }
            (Some("ping"), Some("hosts"), None, None) => {
                self.handle_ping_hosts(payload).await.map(Some)
            }
            _ => {
                error!("unsupported subject `{subject}`");
                return;
            }
        };
        if let Err(e) = &res {
            warn!("failed to handle `{subject}` request: {e:?}");
        }
        match (reply, res) {
            (Some(reply), Ok(Some(buf))) => {
                if let Err(e) = self.nats.publish(reply, buf).await {
                    error!("failed to publish success in response to `{subject}` request: {e:?}");
                }
            }
            (Some(reply), Err(e)) => {
                if let Err(e) = self
                    .nats
                    .publish(
                        reply,
                        format!(r#"{{"accepted":false,"error":"{e}"}}"#).into(),
                    )
                    .await
                {
                    error!("failed to publish error: {e:?}");
                }
            }
            _ => {}
        }
    }
}
