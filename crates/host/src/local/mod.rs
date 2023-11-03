/// Local host configuration
pub mod config;

pub use config::{
    Actor as ActorConfig, Host as HostConfig, Link as LinkConfig, TcpSocket as TcpSocketConfig,
};

use crate::socket_pair;

use core::future::Future;
use core::pin::Pin;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use async_recursion::async_recursion;
use async_trait::async_trait;
use futures::stream::{AbortHandle, Abortable};
use futures::{stream, try_join, FutureExt, StreamExt, TryStreamExt};
use tokio::io::{stderr, AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use tokio::{fs, spawn};
use tokio_stream::wrappers::TcpListenerStream;
use tracing::{debug, error, instrument, trace};
use wasmcloud_runtime::actor::GuestInstance;
use wasmcloud_runtime::capability::{Bus, TargetEntity, TargetInterface};
use wasmcloud_runtime::{ActorInstance, Runtime};

#[instrument]
fn get_actor<'a>(actors: &'a HashMap<String, Actor>, actor: &str) -> anyhow::Result<&'a Actor> {
    actors
        .get(actor)
        .with_context(|| format!("actor `{actor}` not found"))
}

#[instrument]
fn get_actor_mut<'a>(
    actors: &'a mut HashMap<String, Actor>,
    actor: &str,
) -> anyhow::Result<&'a mut Actor> {
    actors
        .get_mut(actor)
        .with_context(|| format!("actor `{actor}` not found"))
}

#[instrument]
async fn get_actor_link<'a>(
    actors: &'a HashMap<String, Actor>,
    actor: &str,
) -> anyhow::Result<ActorInstance> {
    get_actor(actors, actor)
        .with_context(|| format!("failed to link to `{actor}`"))?
        .instantiate(actors)
        .await
        .with_context(|| format!("failed to instantiate `{actor}` link"))
}

#[derive(Debug)]
struct Actor {
    actor: wasmcloud_runtime::Actor,
    logging: Option<String>,
    incoming_http: Option<String>,
    interfaces: HashMap<String, String>,
}

#[derive(Default)]
struct BusHandler(HashMap<String, GuestInstance>);

#[async_trait]
impl Bus for BusHandler {
    #[instrument(skip(self))]
    async fn identify_wasmbus_target(
        &self,
        _binding: &str,
        _namespace: &str,
    ) -> anyhow::Result<TargetEntity> {
        bail!("not supported by local host")
    }

    #[instrument(skip(self))]
    async fn identify_interface_target(
        &self,
        _interface: &TargetInterface,
    ) -> anyhow::Result<Option<TargetEntity>> {
        bail!("not supported by local host")
    }

    #[instrument(skip(self))]
    async fn set_target(
        &self,
        _target: Option<TargetEntity>,
        _interfaces: Vec<TargetInterface>,
    ) -> anyhow::Result<()> {
        bail!("not supported by local host")
    }

    #[instrument(skip(self))]
    async fn call(
        &self,
        target: Option<TargetEntity>,
        operation: String,
    ) -> anyhow::Result<(
        Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
        Box<dyn AsyncWrite + Send + Sync + Unpin>,
        Box<dyn AsyncRead + Send + Sync + Unpin>,
    )> {
        if target.is_some() {
            bail!("targets not supported by local host")
        }
        let actor = self
            .0
            .get(&operation)
            .with_context(|| format!("no link for `{operation}`"))?;

        trace!("call actor");
        let (req_r, req_w) = socket_pair()?;
        let (res_r, res_w) = socket_pair()?;
        let actor = actor.clone();
        Ok((
            async move {
                actor
                    .call(operation, req_r, res_w)
                    .await
                    .context("failed to call actor")
                    .map_err(|e| e.to_string())?
            }
            .boxed(),
            Box::new(req_w),
            Box::new(res_r),
        ))
    }
}

impl FromIterator<(String, GuestInstance)> for BusHandler {
    fn from_iter<T: IntoIterator<Item = (String, GuestInstance)>>(iter: T) -> Self {
        Self(HashMap::from_iter(iter))
    }
}

impl Extend<(String, GuestInstance)> for BusHandler {
    fn extend<T: IntoIterator<Item = (String, GuestInstance)>>(&mut self, iter: T) {
        self.0.extend(iter);
    }
}

impl Actor {
    #[instrument]
    pub async fn new(rt: &Runtime, ActorConfig { url }: &ActorConfig) -> anyhow::Result<Self> {
        trace!("create actor");
        match url.scheme() {
            "file" => {
                let path = url
                    .to_file_path()
                    .map_err(|()| anyhow!("failed to convert `{url}` to a file path"))?;
                let buf = fs::read(path).await.context("failed to read actor")?;
                let actor = wasmcloud_runtime::Actor::new(rt, buf)
                    .context("failed to initialize local actor")?;
                Ok(Self {
                    actor,
                    logging: None,
                    incoming_http: None,
                    interfaces: HashMap::default(),
                })
            }
            scheme => bail!("`{scheme}` URLs not supported yet"),
        }
    }

    #[instrument]
    #[async_recursion]
    pub async fn instantiate(
        &self,
        actors: &HashMap<String, Actor>,
    ) -> anyhow::Result<ActorInstance> {
        let Self {
            actor,
            logging,
            incoming_http,
            interfaces,
            ..
        } = self;
        trace!("instantiate local actor");
        let mut actor = actor
            .instantiate()
            .await
            .context("failed to instantiate actor")?;
        actor
            .stderr(stderr()) // TODO: Add actor name prefix per-line?
            .await
            .context("failed to set stderr")?;
        let (incoming_http, logging) = try_join!(
            async {
                let Some(incoming_http) = incoming_http else {
                    return Ok(None);
                };
                get_actor_link(actors, incoming_http)
                    .await
                    .map(ActorInstance::from)?
                    .into_incoming_http()
                    .await
                    .with_context(|| {
                        format!("failed to establish `wasi:http/incoming-handler` link to `{incoming_http}`")
                    })
                    .map(Some)
            },
            async {
                let Some(logging) = logging else {
                    return Ok(None);
                };
                get_actor_link(actors, logging)
                    .await
                    .map(ActorInstance::from)?
                    .into_logging()
                    .await
                    .with_context(|| {
                        format!("failed to establish `wasi:logging/logging` link to `{logging}`")
                    })
                    .map(Some)
            },
        )?;
        let interfaces: BusHandler = stream::iter(interfaces)
            .then(|(name, target)| async move {
                get_actor_link(actors, target)
                    .await
                    .map(ActorInstance::from)?
                    .into_guest()
                    .await
                    .map(|target| (name.clone(), target))
                    .with_context(|| format!("failed to establish `{name}` link to `{target}`"))
            })
            .try_collect()
            .await?;
        actor.bus(Arc::new(interfaces));
        if let Some(incoming_http) = incoming_http {
            actor.incoming_http(Arc::new(incoming_http));
        };
        if let Some(logging) = logging {
            actor.logging(Arc::new(logging));
        };
        Ok(actor)
    }

    #[instrument(skip_all)]
    pub async fn call(
        &self,
        actors: &HashMap<String, Actor>,
        operation: impl AsRef<str>,
        request: impl AsyncRead + Send + Sync + Unpin + 'static,
        response: impl AsyncWrite + Send + Sync + Unpin + 'static,
    ) -> anyhow::Result<()> {
        self.instantiate(actors)
            .await
            .context("failed to instantiate actor")?
            .call(operation, request, response)
            .await
            .context("failed to call actor instance")?
            .map_err(|e| anyhow!(e).context("actor call failed"))
    }
}

/// Local host
#[derive(Debug)]
pub struct Host {
    #[allow(unused)] // TODO: Use and remove
    actors: Arc<RwLock<HashMap<String, Actor>>>,
    state: State,
}

/// Local host state
#[derive(Debug, Default)]
pub struct State {
    tcp_listeners: HashMap<String, AbortHandle>,
}

impl Drop for Host {
    fn drop(&mut self) {
        for abort in self.state.tcp_listeners.values() {
            abort.abort();
        }
    }
}

#[instrument]
async fn handle_tcp_stream(
    stream: TcpStream,
    chain: &[String],
    actors: &Arc<RwLock<HashMap<String, Actor>>>,
) -> anyhow::Result<()> {
    const OPERATION: &str = "wasmcloud:actor/stdio-handler.handle";
    let (first_req, last_res) = stream.into_split();
    let actors = actors.read().await;
    match chain {
        [] => bail!("chain cannot be empty"),
        [actor] => {
            get_actor(&actors, actor)?
                .call(&actors, OPERATION, first_req, last_res)
                .await
        }
        [first, last] => {
            let first = get_actor(&actors, first)?;
            let last = get_actor(&actors, last)?;
            let (first_res, last_req) = socket_pair()?;
            try_join!(
                first.call(&actors, OPERATION, first_req, first_res),
                last.call(&actors, OPERATION, last_req, last_res)
            )?;
            Ok(())
        }
        [first, chain @ .., last] => {
            let first = get_actor(&actors, first)?;
            let last = get_actor(&actors, last)?;
            let (res, mut next_req) = socket_pair()?;
            first.call(&actors, OPERATION, first_req, res).await?;
            for actor in chain {
                let actor = get_actor(&actors, actor)?;
                let (res, next) = socket_pair()?;
                actor.call(&actors, OPERATION, next_req, res).await?;
                next_req = next;
            }
            last.call(&actors, OPERATION, next_req, last_res).await
        }
    }
    .context("failed to execute chain")
}

impl Host {
    /// Construct a new [Host]
    #[instrument]
    pub async fn new(HostConfig { actors, links }: HostConfig) -> anyhow::Result<Self> {
        // TODO: Configure
        let rt = Runtime::builder()
            .build()
            .context("failed to build runtime")?;
        let rt = Arc::new(rt);
        trace!("construct actor map");
        let actors: HashMap<_, _> = stream::iter(actors)
            .then(|(name, conf)| {
                let rt = Arc::clone(&rt);
                async move {
                    Actor::new(&rt, &conf)
                        .await
                        .context("failed to create actor")
                        .map(|actor| (name, actor))
                }
            })
            .try_collect()
            .await
            .context("failed to apply actor config")?;
        let actors = Arc::new(RwLock::new(actors));

        let state = stream::iter(links)
            .map(anyhow::Result::<_>::Ok)
            .try_fold(State::default(), |mut state, link| async {
                match link {
                    LinkConfig::Tcp {
                        socket: TcpSocketConfig { addr },
                        chain,
                    } => {
                        trace!("link `{addr}` -> `{chain:?}` via TCP");
                        let listener = TcpListener::bind(&addr)
                            .await
                            .with_context(|| format!("failed to bind on `{addr}`"))?;
                        let actors = Arc::clone(&actors);
                        let chain = chain.clone();
                        let (abort, abort_reg) = AbortHandle::new_pair();
                        spawn(async move {
                            Abortable::new(TcpListenerStream::new(listener), abort_reg)
                                .for_each_concurrent(None, |stream| async {
                                    if let Err(e) = async {
                                        let stream =
                                            stream.context("failed to initialize TCP stream")?;
                                        debug!(
                                            "received TCP connection from {}",
                                            stream.peer_addr().map_or_else(
                                                |_| { "unknown address".into() },
                                                |peer| peer.to_string()
                                            )
                                        );
                                        handle_tcp_stream(stream, &chain, &actors)
                                            .await
                                            .context("failed to handle TCP stream")
                                    }
                                    .await
                                    {
                                        error!("failed to handle request: {e:?}");
                                    }
                                })
                                .await;
                        });
                        if let Some(prev) = state.tcp_listeners.insert(addr, abort) {
                            prev.abort();
                        };
                        Ok(state)
                    }
                    LinkConfig::Interface {
                        name,
                        source,
                        target,
                    } => {
                        trace!("link `{source}` -> `{target}` via `{name}` interface");
                        let mut actors = actors.write().await;
                        let source = get_actor_mut(&mut actors, &source)
                            .context("source actor not found")?;
                        match name.as_str() {
                            "wasi:logging/logging" => {
                                let _ = source.logging.insert(target);
                            }
                            "wasi:http/incoming-handler" => {
                                let _ = source.incoming_http.insert(target);
                            }
                            _ => {
                                let _ = source.interfaces.insert(name, target);
                            }
                        }
                        Ok(state)
                    }
                }
            })
            .await
            .context("failed to apply link config")?;
        Ok(Self { actors, state })
    }
}
