use crate::actors::{ActorHost, WasccActor};
use crate::auth::Authorizer;
use crate::capability::extras::ExtrasCapabilityProvider;
use crate::capability::native_host::NativeCapabilityHost;
use crate::control_interface::ctlactor::ControlInterface;
use crate::dispatch::Invocation;
use crate::hlreg::HostLocalSystemService;
use crate::messagebus::rpc_client::LinkDefinition;
use crate::messagebus::{AdvertiseBinding, FindBindings, MessageBus, Unsubscribe, OP_BIND_ACTOR};
use crate::middleware::Middleware;
use crate::oci::fetch_oci_bytes;
use crate::{HostManifest, NativeCapability, Result, WasccEntity, SYSTEM_ACTOR};
use actix::prelude::*;
use futures::executor::block_on;
use provider_archive::ProviderArchive;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use wascap::prelude::KeyPair;

mod hc_actor;

pub(crate) const CORELABEL_ARCH: &str = "hostcore.arch";
pub(crate) const CORELABEL_OS: &str = "hostcore.os";
pub(crate) const CORELABEL_OSFAMILY: &str = "hostcore.osfamily";
pub(crate) const RESTRICTED_LABELS: [&str; 3] = [CORELABEL_OSFAMILY, CORELABEL_ARCH, CORELABEL_OS];

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
#[rtype(result = "bool")]
pub(crate) struct QueryActorRunning {
    pub actor_ref: String,
}

#[derive(Message)]
#[rtype(result = "bool")]
pub(crate) struct QueryProviderRunning {
    pub provider_ref: String,
}

#[derive(Message)]
#[rtype(result = "String")]
pub(crate) struct GetHostID;

#[derive(Message)]
#[rtype(result = "u64")]
pub(crate) struct QueryUptime;
