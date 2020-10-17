use crate::Result;
use actix::prelude::*;

#[derive(Default)]
pub struct ControlPlane;

impl Supervised for ControlPlane {}

impl SystemService for ControlPlane {
    fn service_started(&mut self, ctx: &mut Context<Self>) {
        info!("Control Plane started");
    }
}

impl Actor for ControlPlane {
    type Context = Context<Self>;
}

impl Handler<GetProviderForBinding> for ControlPlane {
    type Result = Option<String>;

    fn handle(&mut self, _msg: GetProviderForBinding, ctx: &mut Self::Context) -> Self::Result {
        Some("TBD".to_string())
    }
}

#[derive(Message)]
#[rtype(result = "Option<String>")]
pub struct GetProviderForBinding {
    pub contract_id: String,
    pub actor: String,
}
