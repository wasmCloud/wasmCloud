/// Wasmbus lattice configuration
pub mod config;

pub use config::Lattice as LatticeConfig;

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;

use std::collections::{BTreeMap, HashMap};
use std::env;
use std::env::consts::{ARCH, FAMILY, OS};
use std::sync::Arc;

use anyhow::{bail, ensure, Context as _};
use bytes::Bytes;
use cloudevents::{EventBuilder, EventBuilderV10};
use futures::stream::{AbortHandle, Abortable};
use futures::{try_join, Stream, StreamExt};
use nkeys::{KeyPair, KeyPairType};
use serde::Deserialize;
use serde_json::json;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio::spawn;
use tokio::sync::watch;
use tokio::time::{interval_at, Instant};
use tokio_stream::wrappers::IntervalStream;
use tracing::{debug, error, info, instrument, trace, warn};
use ulid::Ulid;
use uuid::Uuid;

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

/// Wasmbus lattice
#[derive(Debug)]
pub struct Lattice {
    cluster_key: KeyPair,
    event_builder: EventBuilderV10,
    friendly_name: String,
    heartbeat: AbortHandle,
    host_key: KeyPair,
    labels: HashMap<String, String>,
    nats: async_nats::Client,
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

        let event_builder = EventBuilderV10::new().source(host_key.public_key());
        let wasmbus = Lattice {
            cluster_key,
            event_builder,
            friendly_name,
            heartbeat: heartbeat_abort.clone(),
            host_key,
            labels,
            nats,
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
                    if let Err(e) = wasmbus
                        .publish_event("host_heartbeat", wasmbus.heartbeat())
                        .await
                    {
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

    fn heartbeat(&self) -> serde_json::Value {
        let uptime = self.start_at.elapsed();
        json!({
            "actors": {}, // TODO
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

        bail!("TODO");
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

        bail!("TODO");
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
            count: usize,
            #[serde(default)]
            annotations: BTreeMap<String, String>,
        }
        let Command {
            actor_ref,
            count,
            annotations,
        }: Command = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize actor launch command")?;

        debug!(actor_ref, count, ?annotations, "launch actor");

        bail!("TODO");
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
            annotations: BTreeMap<String, String>,
        }
        let Command {
            actor_ref,
            count,
            annotations,
        }: Command = serde_json::from_slice(payload.as_ref())
            .context("failed to deserialize actor stop command")?;

        debug!(actor_ref, count, ?annotations, "stop actor");

        bail!("TODO");
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
        let buf = serde_json::to_vec(&json!({
          "host_id": self.host_key.public_key(),
          "issuer": self.cluster_key.public_key(),
          "labels": self.labels,
          "friendly_name": self.friendly_name,
          "actors": [], // TODO
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
