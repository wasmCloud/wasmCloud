use crate::actors::{ActorHost, WasccActor};
use crate::auth::Authorizer;
use crate::capability::extras::ExtrasCapabilityProvider;
use crate::capability::native_host::NativeCapabilityHost;
use crate::control_plane::actorhost::ControlPlane;
use crate::dispatch::Invocation;
use crate::messagebus::{
    FindBindings, MessageBus, SetAuthorizer, SetKey, Unsubscribe, OP_BIND_ACTOR,
};
use crate::middleware::Middleware;
use crate::{NativeCapability, Result, WasccEntity, SYSTEM_ACTOR};
use actix::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use wascap::prelude::KeyPair;

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
            let k = KeyPair::from_seed(&ks).unwrap();
            let extras = ExtrasCapabilityProvider::default();
            let claims = crate::capability::extras::get_claims();
            let cap = NativeCapability::from_instance(extras, Some("default".to_string()), claims)
                .unwrap();
            NativeCapabilityHost::try_new(cap, vec![], k, None).unwrap()
        });
        self.providers.insert(pk.clone(), extras); // can't let this provider go out of scope, or the actor will stop
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

    // We should be able to make the actor stop itself by removing the last reference to it
    fn handle(&mut self, msg: StopActor, _ctx: &mut Context<Self>) -> Self::Result {
        println!("Handling remove");
        let pk = if let Some(pk) = self.image_refs.remove(&msg.actor_ref) {
            let _ = self.actors.remove(&pk);
            pk
        } else {
            let _ = self.actors.remove(&msg.actor_ref);
            msg.actor_ref.to_string()
        };
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

    // The provider should stop itself once all references to it are gone
    fn handle(&mut self, msg: StopProvider, _ctx: &mut Context<Self>) -> Self::Result {
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
        println!("minting invocation request");
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
    type Result = Result<()>;

    fn handle(&mut self, msg: StartActor, ctx: &mut Context<Self>) -> Result<()> {
        let seed = self.kp.as_ref().unwrap().seed()?;
        let mw = self.mw_chain.clone();
        let bytes = msg.actor.bytes.clone();
        let claims = &msg.actor.token.claims;
        let imgref = msg.image_ref.clone();
        if !self.authorizer.as_ref().unwrap().can_load(claims) {
            return Err("Permission denied starting actor.".into());
        }

        let new_actor = SyncArbiter::start(1, move || {
            ActorHost::new(bytes.clone(), None, mw.clone(), seed.clone(), imgref.clone())
        });

        if let Some(imageref) = msg.image_ref {
            self.image_refs.insert(imageref, msg.actor.public_key());
        }
        self.actors.insert(msg.actor.public_key(), new_actor);

        Ok(())
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

    fn handle(&mut self, msg: StartProvider, _ctx: &mut Context<Self>) -> Self::Result {
        let seed = self.kp.as_ref().unwrap().seed().unwrap();
        let s = seed.clone();
        let mw = self.mw_chain.clone();

        let provider = msg.provider;
        let sub = provider.claims.subject.to_string();
        let capid = provider.claims.metadata.as_ref().unwrap().capid.to_string();
        let binding_name = provider.binding_name.to_string();
        let provider_id = provider.claims.subject.to_string();
        let image_ref = msg.image_ref.clone();
        let ir2 = msg.image_ref.clone();

        let new_provider = SyncArbiter::start(1, move || {
            //TODO: get rid of this unwrap - it can take out the entire host controller
            NativeCapabilityHost::try_new(
                provider.clone(),
                mw.clone(),
                KeyPair::from_seed(&seed).unwrap(),
                image_ref.clone(),
            )
            .unwrap()
        });
        let target = new_provider.clone().recipient();
        if let Some(imageref) = ir2 {
            self.image_refs.insert(imageref, provider_id.to_string());
        }
        self.providers.insert(provider_id.to_string(), new_provider);

        let k = KeyPair::from_seed(&s).unwrap();
        println!("Attempting to re-invoke bindings");
        Box::pin(
            async move {
                let b = MessageBus::from_registry();
                let bindings = b
                    .send(FindBindings {
                        provider_id: provider_id.to_string(),
                        binding_name: binding_name.to_string(),
                    })
                    .await;
                println!("BINDINGS: {:?}", bindings);
                if let Ok(bindings) = bindings {
                    reinvoke_bindings(&k, target, &sub, &capid, &binding_name, bindings.bindings)
                        .await;
                    Ok(())
                } else {
                    Err("Failed to obtain list of bindings for re-invoke from message bus".into())
                }
            }
            .into_actor(self),
        )
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
        println!("Re-invoking binding {}->{}", actor, provider_id);
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

pub(crate) const CORELABEL_ARCH: &str = "hostcore.arch";
pub(crate) const CORELABEL_OS: &str = "hostcore.os";
pub(crate) const CORELABEL_OSFAMILY: &str = "hostcore.osfamily";
