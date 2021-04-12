use super::*;
use crate::actors::ActorHost;
use crate::auth::Authorizer;
use crate::capability::extras::ExtrasCapabilityProvider;
use crate::capability::native_host::NativeCapabilityHost;
use crate::dispatch::Invocation;
use crate::hlreg::HostLocalSystemService;
use crate::messagebus::{GetClaims, LatticeCacheClient, MessageBus, SetCacheClient, Unsubscribe};
use crate::middleware::Middleware;
use crate::{NativeCapability, Result, WasmCloudEntity, SYSTEM_ACTOR};
use std::collections::HashMap;

use std::time::Instant;

use crate::messagebus::latticecache_client::{CACHE_CONTRACT_ID, CACHE_PROVIDER_LINK_NAME};
use crate::messagebus::utils::{generate_link_invocation_and_call, system_actor_claims};
use wascap::jwt::Claims;
use wascap::prelude::KeyPair;
use wasmcloud_nats_kvcache::NatsReplicatedKVProvider;

#[derive(Debug, PartialEq, Clone, Eq, Hash)]
struct ProviderKey {
    pub id: String,
    pub link_name: String,
}

impl ProviderKey {
    pub fn new(id: &str, link_name: &str) -> Self {
        ProviderKey {
            id: id.to_string(),
            link_name: link_name.to_string(),
        }
    }
}

pub struct HostController {
    host_labels: HashMap<String, String>,
    mw_chain: Vec<Box<dyn Middleware>>,
    kp: Option<KeyPair>,
    actors: HashMap<String, Addr<ActorHost>>,
    providers: HashMap<ProviderKey, Addr<NativeCapabilityHost>>,
    authorizer: Option<Box<dyn Authorizer>>,
    started: Instant,
    allow_live_updates: bool,
    latticecache: Option<LatticeCacheClient>,
    strict_update_check: bool,
}

impl Default for HostController {
    fn default() -> Self {
        HostController {
            host_labels: HashMap::new(),
            mw_chain: vec![],
            kp: None,
            actors: HashMap::new(),
            providers: HashMap::new(),
            authorizer: None,
            started: Instant::now(),
            latticecache: None,
            allow_live_updates: false,
            strict_update_check: true,
        }
    }
}

impl Supervised for HostController {}

impl SystemService for HostController {
    fn service_started(&mut self, ctx: &mut Context<Self>) {
        info!("Host Controller started");

        // TODO: make this value configurable
        ctx.set_mailbox_capacity(1000);
    }
}

impl HostLocalSystemService for HostController {}

impl Actor for HostController {
    type Context = Context<Self>;
}

impl HostController {
    fn bus(&self) -> Addr<MessageBus> {
        MessageBus::from_hostlocal_registry(&self.kp.as_ref().unwrap().public_key())
    }
}

impl Handler<AuctionActor> for HostController {
    type Result = ResponseActFuture<Self, bool>;

    // Indicate if the specified actor can be launched on this host. Returns
    // true only if the actor is not running and the host satisfies the indicated
    // constraints.
    fn handle(&mut self, msg: AuctionActor, _ctx: &mut Context<Self>) -> Self::Result {
        trace!("Received actor auction {}", msg.actor_ref);
        let lc = self.latticecache.clone().unwrap();
        let host_labels = self.host_labels.clone();
        let actor_ref = msg.actor_ref.to_string();

        Box::pin(
            async move {
                if let Some(pk) = lc.lookup_oci_mapping(&actor_ref).await.unwrap_or(None) {
                    pk
                } else {
                    actor_ref
                }
            }
            .into_actor(self)
            .map(move |pk, act, _ctx| {
                !act.actors.contains_key(&pk)
                    && satisfies_constraints(&host_labels, &msg.constraints)
            }),
        )
    }
}

impl Handler<AuctionProvider> for HostController {
    type Result = ResponseActFuture<Self, bool>;

    // Indicate if the specified provider can be launched on this host. Returns true
    // only if the provider is not running and the host satisfies the indicated
    // constraints.
    fn handle(&mut self, msg: AuctionProvider, _ctx: &mut Context<Self>) -> Self::Result {
        trace!("Received provider auction {}", msg.provider_ref);
        let lc = self.latticecache.clone().unwrap();
        let host_labels = self.host_labels.clone();
        let provider_ref = msg.provider_ref.to_string();
        Box::pin(
            async move {
                if let Some(pid) = lc.lookup_oci_mapping(&provider_ref).await.unwrap_or(None) {
                    pid
                } else {
                    provider_ref
                }
            }
            .into_actor(self)
            .map(move |pk, act, _ctx| {
                !act.providers.contains_key(&ProviderKey {
                    id: pk,
                    link_name: msg.link_name.to_string(),
                }) && satisfies_constraints(&host_labels, &msg.constraints)
            }),
        )
    }
}

fn satisfies_constraints(
    host_labels: &HashMap<String, String>,
    constraints: &HashMap<String, String>,
) -> bool {
    // All constraints must exist and match exactly to respond positively to auction
    for (constraint, reqval) in constraints {
        if let Some(v) = host_labels.get(constraint) {
            if v != reqval {
                return false;
            }
        } else {
            return false;
        }
    }

    true
}

impl Handler<QueryActorRunning> for HostController {
    type Result = ResponseActFuture<Self, bool>;

    fn handle(&mut self, msg: QueryActorRunning, _ctx: &mut Context<Self>) -> Self::Result {
        trace!("Received ActorRunning query {}", msg.actor_ref);
        let lc = self.latticecache.clone().unwrap();
        Box::pin(
            async move {
                if let Some(pid) = lc.lookup_oci_mapping(&msg.actor_ref).await.unwrap_or(None) {
                    pid
                } else {
                    msg.actor_ref
                }
            }
            .into_actor(self)
            .map(|pk, act, _ctx| act.actors.contains_key(&pk)),
        )
    }
}

// This returns the messaging address of the actor host that corresponds to a -public key-
// this handler does NOT examine image refs
impl Handler<GetRunningActor> for HostController {
    type Result = Option<Addr<ActorHost>>;

    fn handle(&mut self, msg: GetRunningActor, _ctx: &mut Context<Self>) -> Self::Result {
        trace!("Getting running actor {}", msg.actor_id);
        self.actors.get(&msg.actor_id).cloned()
    }
}

impl Handler<QueryUptime> for HostController {
    type Result = u64;

    fn handle(&mut self, _msg: QueryUptime, _ctx: &mut Context<Self>) -> Self::Result {
        self.started.elapsed().as_secs()
    }
}

impl Handler<QueryProviderRunning> for HostController {
    type Result = ResponseActFuture<Self, bool>;

    fn handle(&mut self, msg: QueryProviderRunning, _ctx: &mut Context<Self>) -> Self::Result {
        let lc = self.latticecache.clone().unwrap();
        let provider_ref = msg.provider_ref.to_string();
        Box::pin(
            async move {
                if let Some(pid) = lc.lookup_oci_mapping(&provider_ref).await.unwrap_or(None) {
                    pid
                } else {
                    provider_ref
                }
            }
            .into_actor(self)
            .map(move |pk, act, _ctx| {
                act.providers
                    .contains_key(&ProviderKey::new(&pk, &msg.link_name))
            }),
        )
    }
}

// If an incoming link definition relates to a provider currently
// running in this host, then re-invoke the link call
// to ensure this provider is aware of it. NOTE that all of the link
// actor RPC calls MUST be considered idempotent because they WILL get
// called multiple times.
impl Handler<CheckLink> for HostController {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: CheckLink, _ctx: &mut Context<Self>) -> Self::Result {
        let key = ProviderKey::new(&msg.linkdef.provider_id, &msg.linkdef.link_name);
        if self.providers.contains_key(&key) {
            let mb = MessageBus::from_hostlocal_registry(&self.kp.as_ref().unwrap().public_key());
            let target = self.providers.get(&key).cloned().unwrap();
            let recip = target.recipient::<Invocation>();
            let actor = msg.linkdef.actor_id.to_string();
            let prov_entity = WasmCloudEntity::Capability {
                id: msg.linkdef.provider_id.to_string(),
                contract_id: msg.linkdef.contract_id,
                link_name: msg.linkdef.link_name,
            };
            let key = KeyPair::from_seed(&self.kp.as_ref().unwrap().seed().unwrap()).unwrap();
            let values = msg.linkdef.values;
            Box::pin(
                async move {
                    let claims = mb.send(GetClaims).await;
                    if claims.is_err() {
                        error!("Could not get claims from message bus");
                        return;
                    }
                    let cr = claims.unwrap();
                    let claims = cr.claims.get(&actor);
                    if claims.is_none() {
                        error!(
                            "No matching actor claims found in actor cache for establishing link"
                        );
                        return;
                    }
                    let claims = claims.unwrap();
                    // We use this utils function so that it's guaranteed to be the same
                    // link invocation as if they'd called `set_link` in the host
                    #[allow(clippy::redundant_pattern_matching)] // .is_err() does not work here
                    if let Err(_) = generate_link_invocation_and_call(
                        &recip,
                        &actor,
                        values,
                        &key,
                        prov_entity,
                        claims.clone(),
                    )
                    .await
                    {
                        error!("Capability provider failed to handle link enable call");
                    }
                }
                .into_actor(self),
            )
        } else {
            Box::pin(async {}.into_actor(self))
        }
    }
}

impl Handler<SetLabels> for HostController {
    type Result = ();

    fn handle(&mut self, msg: SetLabels, _ctx: &mut Context<Self>) -> Self::Result {
        trace!("Setting host labels");
        self.host_labels = msg.labels
    }
}

impl Handler<GetHostID> for HostController {
    type Result = String;

    fn handle(&mut self, _msg: GetHostID, _ctx: &mut Context<Self>) -> Self::Result {
        self.kp.as_ref().unwrap().public_key()
    }
}

impl Handler<StopActor> for HostController {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: StopActor, _ctx: &mut Context<Self>) -> Self::Result {
        trace!("Stopping actor {} per request.", msg.actor_ref);
        let lc = self.latticecache.clone().unwrap();
        let b = MessageBus::from_hostlocal_registry(&self.kp.as_ref().unwrap().public_key());
        Box::pin(
            async move {
                if let Some(pk) = lc.lookup_oci_mapping(&msg.actor_ref).await.unwrap_or(None) {
                    pk
                } else {
                    msg.actor_ref
                }
            }
            .into_actor(self)
            .map(move |pk, act, _ctx| {
                let _ = b.do_send(Unsubscribe {
                    interest: WasmCloudEntity::Actor(pk.to_string()),
                });
                act.actors.remove(&pk);
            }),
        )
    }
}

impl Handler<StopProvider> for HostController {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: StopProvider, _ctx: &mut Context<Self>) -> Self::Result {
        trace!("Stopping provider {} per request", msg.provider_ref);
        let lc = self.latticecache.clone().unwrap();
        let provider_ref = msg.provider_ref.to_string();
        let b = self.bus();
        Box::pin(
            async move {
                if let Some(pk) = lc.lookup_oci_mapping(&provider_ref).await.unwrap_or(None) {
                    pk
                } else {
                    provider_ref
                }
            }
            .into_actor(self)
            .map(move |pk, act, _ctx| {
                act.providers.remove(&ProviderKey::new(&pk, &msg.link_name));
                b.do_send(Unsubscribe {
                    interest: WasmCloudEntity::Capability {
                        id: pk,
                        contract_id: msg.contract_id,
                        link_name: msg.link_name,
                    },
                });
            }),
        )
    }
}

// IMPORTANT NOTE: the message bus needs to have been properly initialized before
// the host controller can be initialized, since the HC sends several messages
// to the message bus
impl Handler<Initialize> for HostController {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: Initialize, _ctx: &mut Context<Self>) -> Self::Result {
        self.host_labels = msg.labels.clone();
        self.authorizer = Some(msg.auth.clone());
        let host_id = msg.kp.public_key();
        trace!("Initializing host controller {}", host_id);

        let claims = crate::capability::extras::get_claims();
        let pk = claims.subject;

        // Start wasmcloud:extras
        let extras = SyncArbiter::start(1, NativeCapabilityHost::new);
        let claims = crate::capability::extras::get_claims();
        let ex = ExtrasCapabilityProvider::default();
        let cap = NativeCapability::from_instance(ex, Some("default".to_string()), claims).unwrap();
        let init = crate::capability::native_host::Initialize {
            cap,
            mw_chain: vec![],
            seed: msg.kp.seed().unwrap(),
            image_ref: None,
        };
        extras.do_send(init);
        let key = ProviderKey::new(&pk, "default");
        self.providers.insert(key, extras); // can't let this provider go out of scope, or the actix actor will stop
        let seed = msg.kp.seed().unwrap();
        self.kp = Some(KeyPair::from_seed(&seed).unwrap());
        self.allow_live_updates = msg.allow_live_updates;
        self.strict_update_check = msg.strict_update_check;
        info!(
            "Host controller initialized - {} (Hot Updating - {})",
            host_id, self.allow_live_updates
        );

        trace!("Host labels: {:?}", &self.host_labels);
        Box::pin(
            async move {
                let cache = SyncArbiter::start(1, NativeCapabilityHost::new);
                let (nativecache, claims) = create_cache_provider(
                    msg.lattice_cache_provider.clone(),
                    msg.allow_latest,
                    &msg.allowed_insecure,
                )
                .await;
                let init = crate::capability::native_host::Initialize {
                    cap: nativecache,
                    mw_chain: vec![],
                    seed: seed.to_string(),
                    image_ref: msg.lattice_cache_provider.clone(),
                };
                // as always, send is a Result<T> which can be a mailbox failure, so results
                // that also return Result end up coming back from a send as Result<Result<T>>...
                let entity = cache.send(init).await;
                match entity {
                    Ok(Ok(_e)) => info!("Initialized lattice cache provider"),
                    Ok(Err(e)) => error!("Failed to initialize lattice cache provider: {}", e),
                    Err(_e) => error!("Lattice cache provider failed to respond to initialization"),
                }
                let kp = KeyPair::from_seed(&seed).unwrap();
                let sysclaims = system_actor_claims();
                let res = generate_link_invocation_and_call(
                    &cache.clone().recipient(),
                    SYSTEM_ACTOR,
                    get_kvcache_values_from_environment(),
                    &kp,
                    WasmCloudEntity::Capability {
                        id: claims.subject.to_string(),
                        contract_id: CACHE_CONTRACT_ID.to_string(),
                        link_name: CACHE_PROVIDER_LINK_NAME.to_string(),
                    },
                    sysclaims,
                )
                .await;
                if res.is_err() {
                    error!("Failed to properly initialize key-value cache provider");
                } else {
                    info!("Cache provider successfully configured");
                }

                info!(
                    "Host controller initialized - {} (Hot Updating - {})",
                    host_id, msg.allow_live_updates
                );
                (
                    claims.subject.to_string(),
                    cache,
                    KeyPair::from_seed(&seed).unwrap(),
                )
            }
            .into_actor(self)
            .map(move |(id, cache, kp), act, _ctx| {
                let pk = kp.public_key();
                let lc = LatticeCacheClient::new(kp, cache.clone().recipient(), &id);
                act.latticecache = Some(lc.clone());

                act.providers.insert(
                    ProviderKey::new(
                        &id,
                        crate::messagebus::latticecache_client::CACHE_PROVIDER_LINK_NAME,
                    ),
                    cache,
                );
                (lc, pk)
            })
            .then(|(lc, pk), act, _ctx| {
                async move {
                    MessageBus::from_hostlocal_registry(&pk)
                        .send(SetCacheClient { client: lc })
                        .await
                        .unwrap(); // if this doesn't succeed, panic is ok
                }
                .into_actor(act)
            }),
        )
    }
}

impl Handler<StartActor> for HostController {
    type Result = ResponseActFuture<Self, Result<()>>;

    fn handle(&mut self, msg: StartActor, _ctx: &mut Context<Self>) -> Self::Result {
        let sub = msg.actor.claims().subject;
        let claims = msg.actor.claims();
        info!("Starting actor {}", sub);

        if self.actors.contains_key(&sub) {
            error!("Aborting attempt to start already running actor {}", sub);
            return Box::pin(
                async move { Err(format!("Cannot start already running actor {}", sub).into()) }
                    .into_actor(self),
            );
        }

        if !self.authorizer.as_ref().unwrap().can_load(&claims) {
            return Box::pin(
                async move { Err("Permission denied starting actor.".into()) }.into_actor(self),
            );
        }
        let init = crate::actors::Initialize {
            actor_bytes: msg.actor.bytes.clone(),
            mw_chain: self.mw_chain.clone(),
            signing_seed: self.kp.as_ref().unwrap().seed().unwrap(),
            image_ref: msg.image_ref.clone(),
            host_id: self.kp.as_ref().unwrap().public_key(),
            can_update: self.allow_live_updates,
            strict_update_check: self.strict_update_check,
        };

        let new_actor = SyncArbiter::start(1, ActorHost::default);
        let na = new_actor.clone();
        let lc = self.latticecache.clone().unwrap();
        let image_ref = msg.image_ref.clone();
        let pk = msg.actor.public_key();

        Box::pin(
            async move {
                new_actor.send(init).await??;
                if let Some(imageref) = image_ref {
                    lc.put_oci_mapping(&imageref, &pk).await?;
                }
                Ok(())
            }
            .into_actor(self)
            .map(move |_res: Result<()>, act, _ctx| {
                act.actors.insert(msg.actor.public_key(), na);
                Ok(())
            }),
        )
    }
}

impl Handler<QueryHostInventory> for HostController {
    type Result = ResponseActFuture<Self, HostInventory>;

    fn handle(&mut self, _msg: QueryHostInventory, _ctx: &mut Context<Self>) -> Self::Result {
        let host_labels = self.host_labels.clone();
        let lc = self.latticecache.clone().unwrap();
        let actors = self.actors.clone();
        let providers = self.providers.clone();
        let host_id = self.kp.as_ref().unwrap().public_key();
        Box::pin(
            async move {
                let image_refs = lc.collect_oci_references().await;
                HostInventory {
                    actors: actors
                        .iter()
                        .map(|(k, _v)| ActorSummary {
                            id: k.to_string(),
                            image_ref: find_imageref(k, &image_refs),
                        })
                        .collect(),
                    host_id,
                    providers: providers
                        .iter()
                        .map(|(k, _v)| ProviderSummary {
                            image_ref: find_imageref(&k.id, &image_refs),
                            id: k.id.to_string(),
                            link_name: k.link_name.to_string(),
                        })
                        .collect(),
                    labels: host_labels,
                }
            }
            .into_actor(self),
        )
    }
}

fn find_imageref(target: &str, image_refs: &HashMap<String, String>) -> Option<String> {
    image_refs
        .iter()
        .find(|(_ir, pk)| pk.to_string() == *target)
        .map(|(ir, _pk)| ir.to_string())
}

impl Handler<StartProvider> for HostController {
    type Result = ResponseActFuture<Self, Result<()>>;

    fn handle(&mut self, msg: StartProvider, _ctx: &mut Context<Self>) -> Self::Result {
        let sub = msg.provider.claims.subject.to_string();
        let key = ProviderKey::new(&sub, &msg.provider.link_name);
        if self.providers.contains_key(&key) {
            error!("Aborting attempt to start already running provider {}", sub);
            return Box::pin(
                async move { Err(format!("Cannot start already running provider {}", sub).into()) }
                    .into_actor(self),
            );
        }

        info!("Starting provider {}", msg.provider.claims.subject);

        let seed = self.kp.as_ref().unwrap().seed().unwrap();
        let mw = self.mw_chain.clone();
        let provider = msg.provider;
        let provider_id = provider.claims.subject.to_string();
        let link_name = provider.link_name.to_string();
        let imageref = msg.image_ref;
        let ir2 = imageref.clone();
        let pid = provider_id.to_string();
        let auther = self.authorizer.as_ref().unwrap().clone();

        let k = KeyPair::from_seed(&seed).unwrap();
        Box::pin(
            async move {
                initialize_provider(
                    provider.clone(),
                    mw.clone(),
                    k.public_key(),
                    seed.to_string(),
                    imageref.clone(),
                    provider_id.to_string(),
                    link_name.to_string(),
                    auther,
                )
                .await
            }
            .into_actor(self)
            .map(move |res, act, _| {
                if let Ok(new_provider) = res {
                    act.providers.insert(key, new_provider);
                }
            })
            .then(move |_, act, _ctx| {
                let lc = act.latticecache.clone().unwrap();
                async move {
                    if let Some(imageref) = ir2 {
                        lc.put_oci_mapping(&imageref, &pid).await?;
                    }
                    Ok(())
                }
                .into_actor(act)
            }),
        )
    }
}

#[allow(clippy::too_many_arguments)]
async fn initialize_provider(
    provider: NativeCapability,
    mw: Vec<Box<dyn Middleware>>,
    _host_id: String,
    seed: String,
    image_ref: Option<String>,
    _provider_id: String,
    _link_name: String,
    _authorizer: Box<dyn Authorizer>,
) -> Result<Addr<NativeCapabilityHost>> {
    let new_provider = SyncArbiter::start(1, NativeCapabilityHost::new);
    let im = crate::capability::native_host::Initialize {
        cap: provider.clone(),
        mw_chain: mw.clone(),
        seed: seed.to_string(),
        image_ref: image_ref.clone(),
    };
    let entity = new_provider.send(im).await??;
    let _capid = match entity {
        WasmCloudEntity::Capability { contract_id, .. } => contract_id,
        _ => return Err("Creating provider returned the wrong entity type!".into()),
    };

    Ok(new_provider)
}

async fn create_cache_provider(
    provider_ref: Option<String>,
    allow_latest: bool,
    allowed_insecure: &[String],
) -> (NativeCapability, Claims<wascap::jwt::CapabilityProvider>) {
    if let Some(s) = provider_ref {
        let par = crate::oci::fetch_provider_archive(&s, allow_latest, allowed_insecure)
            .await
            .unwrap();
        (
            NativeCapability::from_archive(&par, Some(CACHE_PROVIDER_LINK_NAME.to_string()))
                .unwrap(),
            par.claims().unwrap(),
        )
    } else {
        (
            create_default_cache_provider().unwrap(),
            crate::messagebus::latticecache_client::get_claims(),
        ) // if we can't instantiate the default provider, nothing will work anyway. panic is fine here.
    }
}

fn create_default_cache_provider() -> Result<NativeCapability> {
    let claims = crate::messagebus::latticecache_client::get_claims();
    let natscache = NatsReplicatedKVProvider::default();
    NativeCapability::from_instance(
        natscache,
        Some(CACHE_PROVIDER_LINK_NAME.to_string()),
        claims,
    )
}

pub(crate) fn detect_core_host_labels() -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert(
        CORELABEL_ARCH.to_string(),
        std::env::consts::ARCH.to_string(),
    );
    hm.insert(CORELABEL_OS.to_string(), std::env::consts::OS.to_string());
    hm.insert(
        CORELABEL_OSFAMILY.to_string(),
        std::env::consts::FAMILY.to_string(),
    );
    hm
}

// Take all environment variables with the `KVCACHE_` prefix and send them
// to the key-value store provider being used as the lattice cache for the
// "link(bind) actor" invocation.
fn get_kvcache_values_from_environment() -> HashMap<String, String> {
    let mut hm = HashMap::new();
    for (key, value) in std::env::vars() {
        if key.to_uppercase().starts_with("KVCACHE_") {
            let nkey = key.replace("KVCACHE_", "");
            hm.insert(nkey, value);
        }
    }
    hm
}
