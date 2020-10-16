use crate::Result;
use actix::prelude::*;
use std::collections::HashMap;
use wascap::prelude::KeyPair;
use crate::actor_host::ActorHost;

#[derive(Default)]
pub(crate) struct HostController {
    kp: Option<KeyPair>,
    host_labels: HashMap<String, String>,
}

#[derive(Message)]
#[rtype(result = "()")]
pub(crate) struct SetLabels {
    pub labels: HashMap<String, String>,
}

impl Supervised for HostController {}

impl SystemService for HostController {
    fn service_started(&mut self, ctx: &mut Context<Self>) {
        let kp = KeyPair::new_server();
        info!("Host Controller started - {}", kp.public_key());
        self.kp = Some(kp);
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
        let _a = SyncArbiter::start(1, || {
            ActorHost{}
        });
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
