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
