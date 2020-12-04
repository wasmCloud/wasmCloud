use super::*;
use crate::actors::ActorHost;
use crate::auth::Authorizer;
use crate::capability::extras::ExtrasCapabilityProvider;
use crate::capability::native_host::NativeCapabilityHost;
use crate::dispatch::Invocation;
use crate::hlreg::HostLocalSystemService;
use crate::messagebus::{
    CanInvoke, FindBindings, GetClaims, MessageBus, Unsubscribe, OP_BIND_ACTOR,
};
use crate::middleware::Middleware;
use crate::{NativeCapability, Result, WasccEntity, SYSTEM_ACTOR};
use std::collections::HashMap;

use std::time::Instant;
use wascap::jwt::Claims;
use wascap::prelude::KeyPair;

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
    image_refs: HashMap<String, String>,
    started: Instant,
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
            image_refs: HashMap::new(),
            started: Instant::now(),
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

impl Handler<AuctionActor> for HostController {
    type Result = bool;

    fn handle(&mut self, msg: AuctionActor, _ctx: &mut Context<Self>) -> Self::Result {
        if self.image_refs.contains_key(&msg.actor_ref) || self.actors.contains_key(&msg.actor_ref)
        {
            return false; // don't respond to auctions where the actor in question is running already
        }

        satisfies_constraints(&self.host_labels, &msg.constraints)
    }
}

impl Handler<AuctionProvider> for HostController {
    type Result = bool;

    fn handle(&mut self, msg: AuctionProvider, _ctx: &mut Context<Self>) -> Self::Result {
        let pid = if let Some(pid) = self.image_refs.get(&msg.provider_ref) {
            pid
        } else {
            &msg.provider_ref
        };
        if self.providers.contains_key(&ProviderKey {
            id: pid.to_string(),
            link_name: msg.link_name.to_string(),
        }) {
            return false;
        }

        satisfies_constraints(&self.host_labels, &msg.constraints)
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
    type Result = bool;

    fn handle(&mut self, msg: QueryActorRunning, _ctx: &mut Context<Self>) -> Self::Result {
        self.image_refs.contains_key(&msg.actor_ref) || self.actors.contains_key(&msg.actor_ref)
    }
}

impl Handler<QueryUptime> for HostController {
    type Result = u64;

    fn handle(&mut self, _msg: QueryUptime, _ctx: &mut Context<Self>) -> Self::Result {
        self.started.elapsed().as_secs()
    }
}

impl Handler<QueryProviderRunning> for HostController {
    type Result = bool;

    fn handle(&mut self, msg: QueryProviderRunning, _ctx: &mut Context<Self>) -> Self::Result {
        self.image_refs.contains_key(&msg.provider_ref)
            || self
                .providers
                .contains_key(&ProviderKey::new(&msg.provider_ref, &msg.link_name))
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
            let actor = msg.linkdef.actor.to_string();
            let prov_entity = WasccEntity::Capability {
                id: msg.linkdef.provider_id.to_string(),
                contract_id: msg.linkdef.contract_id,
                binding: msg.linkdef.link_name,
            };
            let key = KeyPair::from_seed(&self.kp.as_ref().unwrap().seed().unwrap()).unwrap();
            let values = msg.linkdef.values.clone();
            Box::pin(
                async move {
                    let claims = mb.send(GetClaims).await;
                    if let Err(_) = claims {
                        error!("Could not get claims from message bus");
                        return;
                    }
                    let cr = claims.unwrap();
                    let claims = cr.claims.get(&actor).clone();
                    if claims.is_none() {
                        error!(
                            "No matching actor claims found in actor cache for establishing link"
                        );
                        return;
                    }
                    let claims = claims.unwrap();
                    // We use this utils function so that it's guaranteed to be the same
                    // link invocation as if they'd called `set_link` in the host
                    if let Err(_) = crate::messagebus::utils::generate_binding_invocation(
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
        // We should be able to make the actor stop itself by removing the last reference to it
        let pk = if let Some(pk) = self.image_refs.remove(&msg.actor_ref) {
            let _ = self.actors.remove(&pk);
            pk
        } else {
            let _ = self.actors.remove(&msg.actor_ref);
            msg.actor_ref.to_string()
        };

        // Ensure that this actor's interest is removed from the bus
        let b = MessageBus::from_hostlocal_registry(&self.kp.as_ref().unwrap().public_key());
        Box::pin(
            async move {
                let _ = b
                    .send(Unsubscribe {
                        interest: WasccEntity::Actor(pk),
                    })
                    .await;
            }
            .into_actor(self),
        )
    }
}

impl Handler<StopProvider> for HostController {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: StopProvider, _ctx: &mut Context<Self>) -> Self::Result {
        trace!("Stopping provider {} per request", msg.provider_ref);
        // The provider should stop itself once all references to it are gone
        let pk = if let Some(pk) = self.image_refs.remove(&msg.provider_ref) {
            let _provider = self.providers.remove(&ProviderKey::new(&pk, &msg.binding));
            pk
        } else {
            let _provider = self
                .providers
                .remove(&ProviderKey::new(&msg.provider_ref, &msg.binding));
            msg.provider_ref.to_string()
        };

        let b = MessageBus::from_hostlocal_registry(&self.kp.as_ref().unwrap().public_key());
        Box::pin(
            async move {
                let _ = b
                    .send(Unsubscribe {
                        interest: WasccEntity::Capability {
                            id: pk.to_string(),
                            contract_id: msg.contract_id.to_string(),
                            binding: msg.binding.to_string(),
                        },
                    })
                    .await;
            }
            .into_actor(self),
        )
    }
}

impl Handler<Initialize> for HostController {
    type Result = ();

    fn handle(&mut self, msg: Initialize, _ctx: &mut Context<Self>) {
        self.host_labels = msg.labels;
        self.authorizer = Some(msg.auth);
        let host_id = msg.kp.public_key();

        let claims = crate::capability::extras::get_claims();
        let pk = claims.subject.to_string();
        // Start wascc:extras
        let extras = SyncArbiter::start(1, move || NativeCapabilityHost::new());
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
        self.kp = Some(msg.kp);
        info!("Host controller initialized - {}", host_id);

        trace!("Host labels: {:?}", &self.host_labels);
    }
}

impl Handler<StartActor> for HostController {
    type Result = ResponseActFuture<Self, Result<()>>;

    fn handle(&mut self, msg: StartActor, _ctx: &mut Context<Self>) -> Self::Result {
        let sub = msg.actor.claims().subject.to_string();
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
            wasi: None,
            mw_chain: self.mw_chain.clone(),
            signing_seed: self.kp.as_ref().unwrap().seed().unwrap(),
            image_ref: msg.image_ref.clone(),
            host_id: self.kp.as_ref().unwrap().public_key(),
        };

        let new_actor = SyncArbiter::start(1, move || ActorHost::default());
        let na = new_actor.clone();

        Box::pin(
            async move { new_actor.send(init).await }
                .into_actor(self)
                .map(move |res, act, _ctx| match res {
                    Ok(_) => {
                        if let Some(imageref) = msg.image_ref {
                            act.image_refs.insert(imageref, msg.actor.public_key());
                        }
                        act.actors.insert(msg.actor.public_key(), na);
                        Ok(())
                    }
                    Err(_e) => {
                        error!("Failed to initialize actor");
                        Err("Failed to initialize actor".into())
                    }
                }),
        )
    }
}

impl Handler<QueryHostInventory> for HostController {
    type Result = HostInventory;

    fn handle(&mut self, _msg: QueryHostInventory, _ctx: &mut Context<Self>) -> Self::Result {
        HostInventory {
            actors: self
                .actors
                .iter()
                .map(|(k, v)| ActorSummary {
                    id: k.to_string(),
                    image_ref: find_imageref(k, &self.image_refs),
                })
                .collect(),
            host_id: self.kp.as_ref().unwrap().public_key(),
            providers: self
                .providers
                .iter()
                .map(|(k, v)| ProviderSummary {
                    image_ref: find_imageref(&k.id, &self.image_refs),
                    id: k.id.to_string(),
                    link_name: k.link_name.to_string(),
                })
                .collect(),
            labels: self.host_labels.clone(),
        }
    }
}

fn find_imageref(target: &str, image_refs: &HashMap<String, String>) -> Option<String> {
    image_refs
        .iter()
        .find(|(ir, pk)| &pk.to_string() == target)
        .map(|(ir, _pk)| ir.to_string())
}

impl Handler<StartProvider> for HostController {
    type Result = ResponseActFuture<Self, Result<()>>;

    fn handle(&mut self, msg: StartProvider, _ctx: &mut Context<Self>) -> Self::Result {
        let sub = msg.provider.claims.subject.to_string();
        let key = ProviderKey::new(&sub, &msg.provider.binding_name);
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
        let binding_name = provider.binding_name.to_string();
        let imageref = msg.image_ref.clone();
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
                    binding_name.to_string(),
                    auther,
                )
                .await
            }
            .into_actor(self)
            .map(move |res, act, _| {
                if let Ok(new_provider) = res {
                    if let Some(imageref) = ir2 {
                        act.image_refs.insert(imageref, pid.to_string());
                    }
                    act.providers.insert(key, new_provider);
                }
                Ok(())
            }),
        )
    }
}

async fn initialize_provider(
    provider: NativeCapability,
    mw: Vec<Box<dyn Middleware>>,
    host_id: String,
    seed: String,
    image_ref: Option<String>,
    provider_id: String,
    binding_name: String,
    authorizer: Box<dyn Authorizer>,
) -> Result<Addr<NativeCapabilityHost>> {
    let new_provider = SyncArbiter::start(1, || NativeCapabilityHost::new());
    let im = crate::capability::native_host::Initialize {
        cap: provider.clone(),
        mw_chain: mw.clone(),
        seed: seed.to_string(),
        image_ref: image_ref.clone(),
    };
    let entity = new_provider.send(im).await??;
    let capid = match entity {
        WasccEntity::Capability { contract_id, .. } => contract_id,
        _ => return Err("Creating provider returned the wrong entity type!".into()),
    };

    let b = MessageBus::from_hostlocal_registry(&host_id);

    Ok(new_provider)
    /*let bindings = b
        .send(FindBindings {
            provider_id: provider_id.to_string(),
            binding_name: binding_name.to_string(),
        })
        .await;
    if let Ok(bindings) = bindings {
        trace!("Re-applying link definitions to provider {}", &provider_id);
        let k = KeyPair::from_seed(&seed)?;
        let claims = b.send(GetClaims {}).await;
        if let Ok(c) = claims {
            reinvoke_bindings(
                &k,
                new_provider.clone().recipient(),
                &provider_id,
                &capid,
                &binding_name,
                bindings.bindings,
            )
            .await;
            Ok(new_provider)
        } else {
            Err("Failed to get claims cache from message bus".into())
        }
    } else {
        Err("Failed to obtain list of bindings for re-invoke from message bus".into())
    } */
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

// Examine the bindings cache for anything that applies to this specific provider and, if so, generate a binding
// invocation for it and send it to the provider
async fn reinvoke_bindings(
    key: &KeyPair,
    target: Recipient<Invocation>,
    provider_id: &str,
    contract_id: &str,
    link_name: &str,
    existing_bindings: Vec<(String, HashMap<String, String>)>,
) {
    let mb = MessageBus::from_hostlocal_registry(&key.public_key());
    for (actor, vals) in existing_bindings.iter() {
        trace!("Re-invoking bind_actor {}->{}", actor, provider_id);
        let config = crate::generated::core::CapabilityConfiguration {
            module: actor.to_string(),
            values: vals.clone(),
        };
        let inv = Invocation::new(
            key,
            WasccEntity::Actor(SYSTEM_ACTOR.to_string()),
            WasccEntity::Capability {
                id: provider_id.to_string(),
                contract_id: contract_id.to_string(),
                binding: link_name.to_string(),
            },
            OP_BIND_ACTOR,
            crate::generated::core::serialize(&config).unwrap(),
        );
        let auth = mb
            .send(CanInvoke {
                actor: actor.to_string(),
                contract_id: contract_id.to_string(),
                operation: OP_BIND_ACTOR.to_string(),
                provider_id: provider_id.to_string(),
                link_name: link_name.to_string(),
            })
            .await;
        if let Ok(a) = auth {
            if !a {
                error!("Attempt to re-establish link for unauthorized actor {} to {}. Not invoking link", actor, contract_id);
                continue;
            }
        } else {
            error!("Failed to get authorization decision from message bus, not invoking pre-existing binding");
            continue;
        }
        if let Err(_e) = target.clone().send(inv).await {
            error!(
                "Mailbox failure sending binding re-invoke for {} -> {}",
                actor, provider_id
            );
        }
    }
}
