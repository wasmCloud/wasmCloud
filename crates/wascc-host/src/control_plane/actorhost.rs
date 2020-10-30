use crate::control_plane::{ControlInterface, ControlPlaneProvider};
use crate::messagebus::{MessageBus, SetKey};
use crate::{ControlEvent, Result};
use actix::prelude::*;
use std::collections::HashMap;
use wascap::prelude::KeyPair;

#[derive(Default)]
pub struct ControlPlane {
    provider: Option<Box<dyn ControlPlaneProvider>>,
    key: Option<KeyPair>,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct SetProvider {
    pub provider: Box<dyn ControlPlaneProvider>,
    pub labels: HashMap<String, String>,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct PublishEvent {
    pub event: ControlEvent,
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

impl Handler<SetKey> for ControlPlane {
    type Result = ();

    fn handle(&mut self, msg: SetKey, _ctx: &mut Context<Self>) {
        self.key = Some(msg.key)
    }
}

impl Handler<PublishEvent> for ControlPlane {
    type Result = ();

    fn handle(&mut self, msg: PublishEvent, _ctx: &mut Context<Self>) {
        let evt = msg
            .event
            .replace_header(&self.key.as_ref().unwrap().public_key());
        println!("Publishing {:?}", evt);
        if let Some(ref p) = self.provider {
            if let Err(e) = p.emit_control_event(evt) {
                error!("Control plane failed to emit event: {}", e);
            }
        }
    }
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
        let evt = ControlEvent::HostStarted {
            header: Default::default(),
        };
        let evt = evt.replace_header(&self.key.as_ref().unwrap().public_key());
        if let Err(e) = self.provider.as_ref().unwrap().emit_control_event(evt) {
            error!("Control plane failed to emit host started event: {}", e);
        }
    }
}
