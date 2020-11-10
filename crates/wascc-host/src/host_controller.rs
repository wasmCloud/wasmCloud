use crate::actors::{ActorHost, WasccActor};
use crate::auth::Authorizer;
use crate::capability::extras::ExtrasCapabilityProvider;
use crate::capability::native_host::{NativeCapabilityHost, NativeCapabilityHostBuilder};
use crate::control_plane::actorhost::ControlPlane;
use crate::dispatch::Invocation;
use crate::messagebus::{
    AdvertiseBinding, FindBindings, MessageBus, SetAuthorizer, SetKey, Unsubscribe, OP_BIND_ACTOR,
};
use crate::middleware::Middleware;
use crate::oci::fetch_oci_bytes;
use crate::{HostManifest, NativeCapability, Result, WasccEntity, SYSTEM_ACTOR};
use actix::prelude::*;
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
            let k = KeyPair::from_seed(&ks).unwrap();
            let extras = ExtrasCapabilityProvider::default();
            let claims = crate::capability::extras::get_claims();
            let cap = NativeCapability::from_instance(extras, Some("default".to_string()), claims)
                .unwrap();
            NativeCapabilityHostBuilder::try_new(cap, vec![], None)
                .unwrap()
                .build(KeyPair::from_seed(&k2).unwrap())
        });
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
    type Result = Result<()>;

    fn handle(&mut self, msg: StartActor, ctx: &mut Context<Self>) -> Result<()> {
        let sub = msg.actor.claims().subject.to_string();

        if self.actors.contains_key(&sub) {
            error!("Aborting attempt to start already running actor {}", sub);
            return Err(format!("Cannot start already running actor {}", sub).into());
        }

        trace!(
            "Starting actor {} per request",
            msg.actor.token.claims.subject
        );
        // get "free standing" references to all these things so we don't
        // move self into the arbiter start closure. YAY borrow checker.
        let seed = self.kp.as_ref().unwrap().seed()?;
        let mw = self.mw_chain.clone();
        let bytes = msg.actor.bytes.clone();
        let claims = &msg.actor.token.claims;
        let imgref = msg.image_ref.clone();
        if !self.authorizer.as_ref().unwrap().can_load(claims) {
            return Err("Permission denied starting actor.".into());
        }

        let new_actor = SyncArbiter::start(1, move || {
            ActorHost::new(
                bytes.clone(),
                None,
                mw.clone(),
                seed.clone(),
                imgref.clone(),
            )
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
        let s = seed.clone();
        let mw = self.mw_chain.clone();

        let provider = msg.provider;

        let capid = provider.claims.metadata.as_ref().unwrap().capid.to_string();
        let binding_name = provider.binding_name.to_string();
        let provider_id = provider.claims.subject.to_string();
        let image_ref = msg.image_ref.clone();
        let ir2 = msg.image_ref.clone();

        let ncb =
            NativeCapabilityHostBuilder::try_new(provider.clone(), mw.clone(), image_ref.clone());

        if ncb.is_err() {
            error!("Failed to create a native capability provider host");
            return Box::pin(
                async move { Err("Failed to create native capability provider host".into()) }
                    .into_actor(self),
            );
        }
        let ncb = ncb.unwrap();

        let new_provider = SyncArbiter::start(1, move || {
            ncb.clone().build(KeyPair::from_seed(&seed).unwrap())
        });

        let target = new_provider.clone().recipient();
        if let Some(imageref) = ir2 {
            self.image_refs.insert(imageref, provider_id.to_string());
        }
        self.providers.insert(provider_id.to_string(), new_provider);

        let k = KeyPair::from_seed(&s).unwrap();
        Box::pin(
            async move {
                let b = MessageBus::from_registry();
                let bindings = b
                    .send(FindBindings {
                        provider_id: provider_id.to_string(),
                        binding_name: binding_name.to_string(),
                    })
                    .await;
                if let Ok(bindings) = bindings {
                    trace!("Re-applying bindings to provider {}", &sub);
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
