use crate::control_plane::{ControlInterface, ControlPlaneProvider};
use crate::messagebus::MessageBus;
use crate::Result;
use actix::prelude::*;
use std::collections::HashMap;

#[derive(Default)]
pub struct ControlPlane {
    provider: Option<Box<dyn ControlPlaneProvider>>,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct SetProvider {
    pub provider: Box<dyn ControlPlaneProvider>,
    pub labels: HashMap<String, String>,
}

impl Supervised for ControlPlane {}

impl SystemService for ControlPlane {
    fn service_started(&mut self, ctx: &mut Context<Self>) {
        info!("Control Plane started");
    }
}

impl Actor for ControlPlane {
    type Context = Context<Self>;
}

impl Handler<SetProvider> for ControlPlane {
    type Result = ();

    fn handle(&mut self, msg: SetProvider, ctx: &mut Context<Self>) {
        let controller = ControlInterface {
            labels: msg.labels.clone(),
            bus: MessageBus::from_registry(),
            control_plane: ctx.address(),
        };
        self.provider = Some(msg.provider);
        self.provider.as_mut().unwrap().init(controller);
    }
}
