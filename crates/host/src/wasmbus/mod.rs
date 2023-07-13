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
use tokio::spawn;
use tokio::sync::{watch, RwLock};
use tokio::time::{interval_at, Instant};
use tokio_stream::wrappers::IntervalStream;
use tracing::{debug, error, info, instrument, trace, warn};
use ulid::Ulid;
use uuid::Uuid;
use wascap::jwt;
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
    instances: RwLock<HashMap<Annotations, Vec<Arc<ActorInstance>>>>,
    actor_ref: String,
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
        let uptime = self.start_at.elapsed();
        json!({
            "actors": actors,
            "friendly_name": self.friendly_name,
            "labels": self.labels,
            "providers": [], // TODO
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
        annotations: &Annotations,
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
        annotations: &Annotations,
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
        annotations: impl Into<Annotations>,
    ) -> anyhow::Result<&'a mut Arc<Actor>> {
        let annotations = annotations.into();
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
            actor_ref,
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
        #[derive(Deserialize)]
        struct Command {
            actor_ref: String,
            constraints: HashMap<String, String>,
        }
        let Command {
            actor_ref,
            constraints,
        }: Command = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize actor auction command")?;

        debug!(actor_ref, ?constraints, "auction actor");

        let buf = serde_json::to_vec(&json!({
          "actor_ref": actor_ref,
          "constraints": constraints,
          "host_id": self.host_key.public_key(),
        }))
        .context("failed to encode reply")?;
        Ok(buf.into())
    }

    #[allow(unused)] // TODO: Remove once implemented
    #[instrument(skip(payload))]
    async fn handle_auction_provider(&self, payload: impl AsRef<[u8]>) -> anyhow::Result<Bytes> {
        debug!("auction provider");

        bail!("TODO")
    }

    #[instrument(skip(payload))]
    async fn handle_stop(&self, payload: impl AsRef<[u8]>, host_id: &str) -> anyhow::Result<Bytes> {
        #[derive(Deserialize)]
        struct Command {
            timeout: Option<u64>, // ms
        }
        let Command { timeout }: Command = serde_json::from_slice(payload.as_ref())
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
        #[derive(Deserialize)]
        struct Command {
            actor_id: String,
            actor_ref: String,
            count: usize,
        }
        let Command {
            actor_id,
            actor_ref,
            count,
        }: Command = serde_json::from_slice(payload.as_ref())
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

        let annotations = BTreeMap::default();
        match (
            self.actors.write().await.entry(actor_id),
            NonZeroUsize::new(count),
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
                            &actor.actor_ref,
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
        #[derive(Deserialize)]
        struct Command {
            actor_ref: String,
            #[serde(default)]
            count: Option<usize>,
            #[serde(default)]
            annotations: Annotations,
        }
        let Command {
            actor_ref,
            count,
            annotations,
        }: Command = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize actor launch command")?;
        debug!(actor_ref, ?count, ?annotations, "launch actor");

        let actor = fetch_actor(&actor_ref)
            .await
            .context("failed to fetch actor")?;
        let actor = wasmcloud_runtime::Actor::new(&self.runtime, actor)
            .context("failed to initialize actor")?;
        let claims = actor.claims().context("claims missing")?;

        let count = count.unwrap_or(1);
        let Some(count) = NonZeroUsize::new(count) else {
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
                        &actor.actor_ref,
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
        #[derive(Deserialize)]
        struct Command {
            actor_ref: String,
            count: usize,
            #[serde(default)]
            annotations: Annotations,
        }
        let Command {
            actor_ref,
            count,
            annotations,
        }: Command = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize actor stop command")?;

        debug!(actor_ref, count, ?annotations, "stop actor");

        match (
            self.actors.write().await.entry(actor_ref),
            NonZeroUsize::new(count),
        ) {
            (hash_map::Entry::Occupied(entry), None) => {
                self.stop_actor(entry, host_id).await?;
            }
            (hash_map::Entry::Occupied(entry), Some(count)) => {
                let actor = entry.get();
                let mut instances = actor.instances.write().await;
                let current: usize = instances.values().map(Vec::len).sum();
                let claims = actor.pool.claims().context("claims missing")?;
                let mut remaining = current;
                let mut delta = current.min(count.into());
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
        #[derive(Deserialize)]
        struct Command {
            new_actor_ref: String,
        }
        let Command { new_actor_ref }: Command = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize actor update command")?;

        debug!(new_actor_ref, "update actor");

        bail!("TODO");
    }

    #[instrument(skip(payload))]
    async fn handle_launch_provider(
        &self,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<Bytes> {
        #[derive(Deserialize)]
        struct Command {
            configuration: String,
            link_name: String,
            provider_ref: String,
        }
        let Command {
            configuration,
            link_name,
            provider_ref,
        }: Command = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize provider launch command")?;

        debug!(configuration, link_name, provider_ref, "launch provider");

        bail!("TODO");
    }

    #[allow(unused)] // TODO: Remove once implemented
    #[instrument(skip(payload))]
    async fn handle_stop_provider(
        &self,
        payload: impl AsRef<[u8]>,
        host_id: &str,
    ) -> anyhow::Result<Bytes> {
        bail!("TODO")
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
                            let instance_id = Uuid::from_u128(actor.id.into());
                            if let Some(rev) = actor.pool.claims().and_then(|claims| {
                                claims.metadata.as_ref().map(|jwt::Actor { rev, .. }| rev)
                            }) {
                                json!({
                                    "annotations": annotations,
                                    "instance_id": instance_id,
                                    "revision": rev,
                                })
                            } else {
                                json!({
                                    "annotations": annotations,
                                    "instance_id": instance_id,
                                })
                            }
                        })
                    })
                    .collect();
                if instances.is_empty() {
                    return None;
                }
                if let Some(name) = actor
                    .pool
                    .claims()
                    .and_then(|claims| claims.metadata.as_ref())
                    .and_then(|metadata| metadata.name.as_ref())
                {
                    Some(json!({
                        "id": id,
                        "image_ref": actor.actor_ref,
                        "instances": instances,
                        "name": name,
                    }))
                } else {
                    Some(json!({
                        "id": id,
                        "image_ref": actor.actor_ref,
                        "instances": instances,
                    }))
                }
            })
            .collect()
            .await;
        let buf = serde_json::to_vec(&json!({
          "host_id": self.host_key.public_key(),
          "issuer": self.cluster_key.public_key(),
          "labels": self.labels,
          "friendly_name": self.friendly_name,
          "actors": actors,
          "providers": [], // TODO
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
                self.handle_auction_actor(payload).await
            }
            (Some("auction"), Some("provider"), None, None) => {
                self.handle_auction_provider(payload).await
            }
            (Some("cmd"), Some(host_id), Some("la"), None) => {
                self.handle_launch_actor(payload, host_id).await
            }
            (Some("cmd"), Some(host_id), Some("lp"), None) => {
                self.handle_launch_provider(payload, host_id).await
            }
            (Some("cmd"), Some(host_id), Some("sa"), None) => {
                self.handle_stop_actor(payload, host_id).await
            }
            (Some("cmd"), Some(host_id), Some("scale"), None) => {
                self.handle_scale_actor(payload, host_id).await
            }
            (Some("cmd"), Some(host_id), Some("sp"), None) => {
                self.handle_stop_provider(payload, host_id).await
            }
            (Some("cmd"), Some(host_id), Some("stop"), None) => {
                self.handle_stop(payload, host_id).await
            }
            (Some("cmd"), Some(host_id), Some("update"), None) => {
                self.handle_update_actor(payload, host_id).await
            }
            (Some("get"), Some(host_id), Some("inv"), None) => {
                self.handle_inventory(payload, host_id).await
            }
            (Some("get"), Some("claims"), None, None) => self.handle_claims(payload).await,
            (Some("get"), Some("links"), None, None) => self.handle_links(payload).await,
            (Some("linkdefs"), Some("put"), None, None) => self.handle_linkdef_put(payload).await,
            (Some("linkdefs"), Some("del"), None, None) => self.handle_linkdef_del(payload).await,
            (Some("registries"), Some("put"), None, None) => {
                self.handle_registries_put(payload).await
            }
            (Some("ping"), Some("hosts"), None, None) => self.handle_ping_hosts(payload).await,
            _ => {
                error!("unsupported subject `{subject}`");
                return;
            }
        };
        if let Err(e) = &res {
            warn!("failed to handle `{subject}` request: {e:?}");
        }
        match (reply, res) {
            (Some(reply), Ok(buf)) => {
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
