use crate::actors::{ActorHost, WasccActor};
use crate::auth::Authorizer;
use crate::capability::extras::ExtrasCapabilityProvider;
use crate::capability::native_host::NativeCapabilityHost;
use crate::dispatch::Invocation;
use crate::messagebus::{MessageBus, SetAuthorizer, SetKey};
use crate::middleware::Middleware;
use crate::{NativeCapability, Result, WasccEntity};
use actix::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use wascap::prelude::KeyPair;

#[derive(Default)]
pub(crate) struct HostController {
    host_labels: HashMap<String, String>,
    mw_chain: Vec<Box<dyn Middleware>>,
    kp: Option<KeyPair>,
    actors: Vec<Addr<ActorHost>>,
    providers: Vec<Addr<NativeCapabilityHost>>,
    authorizer: Option<Box<dyn Authorizer>>,
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
}

#[derive(Message)]
#[rtype(result = "Result<()>")]
pub(crate) struct StartProvider {
    pub provider: NativeCapability,
}

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

        // Start wascc:extras
        let _extras = SyncArbiter::start(1, move || {
            let k = KeyPair::from_seed(&ks).unwrap();
            let extras = ExtrasCapabilityProvider::default();
            let claims = crate::capability::extras::get_claims();
            let cap = NativeCapability::from_instance(extras, Some("default".to_string()), claims)
                .unwrap();
            NativeCapabilityHost::new(Arc::new(cap), vec![], k)
        });

        let b = MessageBus::from_registry();
        b.do_send(SetKey {
            key: KeyPair::from_seed(&k2).unwrap(),
        });
    }
}

impl Actor for HostController {
    type Context = Context<Self>;
}

impl Handler<SetLabels> for HostController {
    type Result = ();

    fn handle(&mut self, msg: SetLabels, _ctx: &mut Context<Self>) {
        self.host_labels = msg.labels;
        info!("Host labels: {:?}", &self.host_labels);

        // TODO: this is experimental - remove this
        // Just testing to see if I can start a sync arbiter from inside a
        // non-sync actor
        /*let _a = SyncArbiter::start(1, || {
            ActorHost{}
        }); */
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
        let bytes = msg.actor.bytes;
        let claims = wascap::wasm::extract_claims(&bytes)?;
        if let Some(c) = claims {
            if !self.authorizer.as_ref().unwrap().can_load(&c.claims) {
                return Err("Permission denied starting actor.".into());
            }
        } else {
            return Err("No claims found in actor. Aborting startup.".into());
        }

        let new_actor = SyncArbiter::start(1, move || {
            println!("instantiating (seed {})", &seed);
            let ah = ActorHost::new(bytes.clone(), None, mw.clone(), seed.clone());
            println!("Instantiated actor host");
            ah
        });
        self.actors.push(new_actor);
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
    type Result = Result<()>;

    fn handle(&mut self, msg: StartProvider, _ctx: &mut Context<Self>) -> Result<()> {
        let seed = self.kp.as_ref().unwrap().seed()?;
        let mw = self.mw_chain.clone();
        let prov = Arc::new(msg.provider);

        let new_provider = SyncArbiter::start(1, move || {
            NativeCapabilityHost::new(prov.clone(), mw.clone(), KeyPair::from_seed(&seed).unwrap())
        });
        self.providers.push(new_provider);
        Ok(())
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

pub(crate) const CORELABEL_ARCH: &str = "hostcore.arch";
pub(crate) const CORELABEL_OS: &str = "hostcore.os";
pub(crate) const CORELABEL_OSFAMILY: &str = "hostcore.osfamily";
