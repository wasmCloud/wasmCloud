use crate::actors::{ActorHost, WasccActor};
use crate::auth::Authorizer;
use crate::capability::extras::ExtrasCapabilityProvider;
use crate::capability::native_host::NativeCapabilityHost;
use crate::control_plane::cpactor::ControlPlane;
use crate::dispatch::Invocation;
use crate::messagebus::{
    AdvertiseBinding, FindBindings, MessageBus, SetAuthorizer, SetKey, Unsubscribe, OP_BIND_ACTOR,
};
use crate::middleware::Middleware;
use crate::oci::fetch_oci_bytes;
use crate::{HostManifest, NativeCapability, Result, WasccEntity, SYSTEM_ACTOR};
use actix::prelude::*;
use futures::executor::block_on;
use provider_archive::ProviderArchive;
use std::collections::HashMap;
use std::sync::Arc;
use wascap::prelude::KeyPair;

pub(crate) const CORELABEL_ARCH: &str = "hostcore.arch";
pub(crate) const CORELABEL_OS: &str = "hostcore.os";
pub(crate) const CORELABEL_OSFAMILY: &str = "hostcore.osfamily";
pub(crate) const RESTRICTED_LABELS: [&str; 3] = [CORELABEL_OSFAMILY, CORELABEL_ARCH, CORELABEL_OS];

#[derive(Default)]
pub(crate) struct HostController {
    host_labels: HashMap<String, String>,
    mw_chain: Vec<Box<dyn Middleware>>,
    kp: Option<KeyPair>,
    actors: HashMap<String, Addr<ActorHost>>,
    providers: HashMap<String, Addr<NativeCapabilityHost>>,
    authorizer: Option<Box<dyn Authorizer>>,
    image_refs: HashMap<String, String>,
}

#[derive(Message)]
#[rtype(result = "()")]
pub(crate) struct SetLabels {
    pub labels: HashMap<String, String>,
}

#[derive(Message)]
#[rtype(result = "Result<()>")]
pub(crate) struct StartActor {
    pub actor: WasccActor,
    pub image_ref: Option<String>,
}

#[derive(Message)]
#[rtype(result = "Result<()>")]
pub(crate) struct StartProvider {
    pub provider: NativeCapability,
    pub image_ref: Option<String>,
}

#[derive(Message)]
#[rtype(result = "()")]
pub(crate) struct StopActor {
    pub actor_ref: String,
}

#[derive(Message)]
#[rtype(result = "()")]
pub(crate) struct StopProvider {
    pub provider_ref: String,
    pub binding: String,
    pub contract_id: String,
}

#[derive(Message)]
#[rtype(result = "String")]
pub(crate) struct GetHostID;

#[derive(Message)]
#[rtype(result = "Invocation")]
pub struct MintInvocationRequest {
    pub op: String,
    pub target: WasccEntity,
    pub msg: Vec<u8>,
    pub origin: WasccEntity,
}

impl Supervised for HostController {}

impl SystemService for HostController {
    fn service_started(&mut self, ctx: &mut Context<Self>) {
        let kp = KeyPair::new_server();
        info!("Host Controller started - {}", kp.public_key());
        let ks = kp.seed().unwrap();
        let k2 = ks.clone();
        self.kp = Some(kp);

        // TODO: make this value configurable
        ctx.set_mailbox_capacity(100);

        let b = MessageBus::from_registry();
        b.do_send(SetKey {
            key: KeyPair::from_seed(&k2).unwrap(),
        });

        let cp = ControlPlane::from_registry();
        cp.do_send(SetKey {
            key: KeyPair::from_seed(&k2).unwrap(),
        });

        let claims = crate::capability::extras::get_claims();
        let pk = claims.subject.to_string();
        // Start wascc:extras
        let extras = SyncArbiter::start(1, move || {
            // let k = KeyPair::from_seed(&ks).unwrap();
            NativeCapabilityHost::new()
        });
        let claims = crate::capability::extras::get_claims();
        let ex = ExtrasCapabilityProvider::default();
        let cap = NativeCapability::from_instance(ex, Some("default".to_string()), claims).unwrap();
        let init = crate::capability::native_host::Initialize {
            cap: cap,
            mw_chain: vec![],
            seed: k2.to_string(),
            image_ref: None,
        };
        extras.do_send(init);
        self.providers.insert(pk.clone(), extras); // can't let this provider go out of scope, or the actix actor will stop
    }
}

impl Actor for HostController {
    type Context = Context<Self>;
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
        let b = MessageBus::from_registry();
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
            let _provider = self.providers.remove(&pk);
            pk
        } else {
            let _provider = self.providers.remove(&msg.provider_ref);
            msg.provider_ref.to_string()
        };

        let b = MessageBus::from_registry();
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

impl Handler<SetLabels> for HostController {
    type Result = ();

    fn handle(&mut self, msg: SetLabels, _ctx: &mut Context<Self>) {
        self.host_labels = msg.labels;
        info!("Host labels: {:?}", &self.host_labels);
    }
}

impl Handler<MintInvocationRequest> for HostController {
    type Result = Invocation;

    fn handle(&mut self, msg: MintInvocationRequest, _ctx: &mut Context<Self>) -> Invocation {
        Invocation::new(
            self.kp.as_ref().unwrap(),
            msg.origin.clone(),
            msg.target.clone(),
            &msg.op,
            msg.msg.clone(),
        )
    }
}

impl Handler<StartActor> for HostController {
    type Result = ResponseActFuture<Self, Result<()>>;

    fn handle(&mut self, msg: StartActor, ctx: &mut Context<Self>) -> Self::Result {
        let sub = msg.actor.claims().subject.to_string();

        if self.actors.contains_key(&sub) {
            error!("Aborting attempt to start already running actor {}", sub);
            return Box::pin(
                async move { Err(format!("Cannot start already running actor {}", sub).into()) }
                    .into_actor(self),
            );
        }

        trace!(
            "Starting actor {} per request",
            msg.actor.token.claims.subject
        );
        // get "free standing" references to all these things so we don't
        // move self into the arbiter start closure. YAY borrow checker.
        let seed = self.kp.as_ref().unwrap().seed().unwrap();
        let mw = self.mw_chain.clone();
        let bytes = msg.actor.bytes.clone();
        let claims = &msg.actor.token.claims;
        let imgref = msg.image_ref.clone();
        if !self.authorizer.as_ref().unwrap().can_load(claims) {
            return Box::pin(
                async move { Err("Permission denied starting actor.".into()) }.into_actor(self),
            );
        }
        let init = crate::actors::Initialize {
            actor_bytes: bytes.clone(),
            wasi: None,
            mw_chain: mw.clone(),
            signing_seed: seed.clone(),
            image_ref: imgref.clone(),
        };

        let new_actor = SyncArbiter::start(1, move || ActorHost::default());
        let na = new_actor.clone();

        Box::pin(
            async move { new_actor.send(init).await }
                .into_actor(self)
                .map(move |res, act, ctx| match res {
                    Ok(_) => {
                        if let Some(imageref) = msg.image_ref {
                            act.image_refs.insert(imageref, msg.actor.public_key());
                        }
                        act.actors.insert(msg.actor.public_key(), na);
                        Ok(())
                    }
                    Err(e) => {
                        error!("Failed to start actor");
                        Err("Failed to start actor".into())
                    }
                }),
        )
    }
}

impl Handler<SetAuthorizer> for HostController {
    type Result = ();

    fn handle(&mut self, msg: SetAuthorizer, _ctx: &mut Context<Self>) {
        self.authorizer = Some(msg.auth);
    }
}

impl Handler<StartProvider> for HostController {
    type Result = ResponseActFuture<Self, Result<()>>;

    fn handle(&mut self, msg: StartProvider, ctx: &mut Context<Self>) -> Self::Result {
        let sub = msg.provider.claims.subject.to_string();
        if self.providers.contains_key(&sub) {
            error!("Aborting attempt to start already running provider {}", sub);
            return Box::pin(
                async move { Err(format!("Cannot start already running provider {}", sub).into()) }
                    .into_actor(self),
            );
        }

        trace!(
            "Starting provider {} per request",
            msg.provider.claims.subject
        );

        let seed = self.kp.as_ref().unwrap().seed().unwrap();
        let mw = self.mw_chain.clone();
        let provider = msg.provider;
        let provider_id = provider.claims.subject.to_string();
        let binding_name = provider.binding_name.to_string();
        let imageref = msg.image_ref.clone();
        let ir2 = imageref.clone();
        let pid = provider_id.to_string();

        let k = KeyPair::from_seed(&seed).unwrap();
        Box::pin(
            async move {
                initialize_provider(
                    provider.clone(),
                    mw.clone(),
                    seed.to_string(),
                    imageref.clone(),
                    provider_id.to_string(),
                    binding_name.to_string(),
                )
                .await
            }
            .into_actor(self)
            .map(move |res, act, _| {
                if let Ok(new_provider) = res {
                    if let Some(imageref) = ir2 {
                        act.image_refs.insert(imageref, pid.to_string());
                    }
                    act.providers.insert(pid.to_string(), new_provider);
                }
                Ok(())
            }),
        )
    }
}

async fn initialize_provider(
    provider: NativeCapability,
    mw: Vec<Box<dyn Middleware>>,
    seed: String,
    image_ref: Option<String>,
    provider_id: String,
    binding_name: String,
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

    let b = MessageBus::from_registry();
    let bindings = b
        .send(FindBindings {
            provider_id: provider_id.to_string(),
            binding_name: binding_name.to_string(),
        })
        .await;
    if let Ok(bindings) = bindings {
        trace!("Re-applying bindings to provider {}", &provider_id);
        let k = KeyPair::from_seed(&seed)?;
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
        Err("Failed to obtain list of bindings for re-invoke from message bus".into())
    }
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
    binding: &str,
    existing_bindings: Vec<(String, HashMap<String, String>)>,
) {
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
                binding: binding.to_string(),
            },
            OP_BIND_ACTOR,
            crate::generated::core::serialize(&config).unwrap(),
        );
        if let Err(_e) = target.clone().send(inv).await {
            error!(
                "Mailbox failure sending binding re-invoke for {} -> {}",
                actor, provider_id
            );
        }
    }
}
