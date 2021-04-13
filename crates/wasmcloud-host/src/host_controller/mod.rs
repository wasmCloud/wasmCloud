use crate::actors::{ActorHost, WasmCloudActor};
use crate::auth::Authorizer;

use crate::{NativeCapability, Result};
use actix::prelude::*;
use std::collections::HashMap;
use wasmcloud_control_interface::LinkDefinition;

use wascap::prelude::KeyPair;

mod hc_actor;

pub(crate) const CORELABEL_ARCH: &str = "hostcore.arch";
pub(crate) const CORELABEL_OS: &str = "hostcore.os";
pub(crate) const CORELABEL_OSFAMILY: &str = "hostcore.osfamily";
pub(crate) const RESTRICTED_LABELS: [&str; 3] = [CORELABEL_OSFAMILY, CORELABEL_ARCH, CORELABEL_OS];

use actix::dev::MessageResponse;
pub(crate) use hc_actor::detect_core_host_labels;
pub(crate) use hc_actor::HostController;

#[derive(Message)]
#[rtype(result = "()")]
pub(crate) struct CheckLink {
    pub linkdef: LinkDefinition,
}

#[derive(Message)]
#[rtype(result = "()")]
pub(crate) struct Initialize {
    pub labels: HashMap<String, String>,
    pub auth: Box<dyn Authorizer>,
    pub kp: KeyPair,
    pub allow_live_updates: bool,
    pub allow_latest: bool,
    pub allowed_insecure: Vec<String>,
    pub lattice_cache_provider: Option<String>,
    pub strict_update_check: bool,
}

#[derive(Message)]
#[rtype(result = "()")]
pub(crate) struct SetLabels {
    pub labels: HashMap<String, String>,
}

#[derive(Message)]
#[rtype(result = "Result<()>")]
pub(crate) struct StartActor {
    pub actor: WasmCloudActor,
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
    pub link_name: String,
    pub contract_id: String,
}

#[derive(Message)]
#[rtype(result = "bool")]
pub(crate) struct QueryActorRunning {
    pub actor_ref: String,
}

#[derive(Message)]
#[rtype(result = "bool")]
pub(crate) struct QueryProviderRunning {
    pub provider_ref: String,
    pub link_name: String,
}

#[derive(Message)]
#[rtype(result = "Option<Addr<ActorHost>>")]
pub(crate) struct GetRunningActor {
    pub actor_id: String,
}

#[derive(Message)]
#[rtype(result = "String")]
pub(crate) struct GetHostID;

#[derive(Message)]
#[rtype(result = "u64")]
pub(crate) struct QueryUptime;

#[derive(Message)]
#[rtype(result = "HostInventory")]
pub(crate) struct QueryHostInventory;

#[derive(Message)]
#[rtype(result = "bool")]
pub(crate) struct AuctionProvider {
    pub constraints: HashMap<String, String>,
    pub provider_ref: String,
    pub link_name: String,
}

#[derive(Message)]
#[rtype(result = "bool")]
pub(crate) struct AuctionActor {
    pub constraints: HashMap<String, String>,
    pub actor_ref: String,
}

#[derive(Default, Debug, Clone, PartialEq)]
pub(crate) struct HostInventory {
    pub host_id: String,
    pub labels: HashMap<String, String>,
    pub actors: Vec<ActorSummary>,
    pub providers: Vec<ProviderSummary>,
}

#[derive(Default, Debug, Clone, PartialEq)]
pub(crate) struct ActorSummary {
    pub id: String,
    pub image_ref: Option<String>,
    pub name: Option<String>,
}

#[derive(Default, Debug, Clone, PartialEq)]
pub(crate) struct ProviderSummary {
    pub id: String,
    pub image_ref: Option<String>,
    pub link_name: String,
    pub name: Option<String>,
}

impl<A, M> MessageResponse<A, M> for HostInventory
where
    A: Actor,
    M: Message<Result = HostInventory>,
{
    fn handle(self, _: &mut A::Context, tx: Option<actix::dev::OneshotSender<Self>>) {
        if let Some(tx) = tx {
            if let Err(e) = tx.send(self) {
                error!("send error (HostInventory host:{})", &e.host_id);
            }
        }
    }
}
