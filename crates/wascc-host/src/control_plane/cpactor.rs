use crate::hlreg::HostLocalSystemService;
use crate::messagebus::MessageBus;
use crate::{ControlEvent, Result};
use actix::prelude::*;
use std::collections::HashMap;
use wascap::prelude::KeyPair;

#[derive(Default)]
pub struct ControlPlane {
    client: Option<nats::asynk::Connection>,
    key: Option<KeyPair>,
    options: ControlOptions,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct Initialize {
    pub client: Option<nats::asynk::Connection>,
    pub control_options: ControlOptions,
    pub key: KeyPair,
}

#[derive(Clone, Debug, Default)]
pub struct ControlOptions {
    pub oci_allow_latest: bool,
    pub host_labels: HashMap<String, String>,
    pub max_actors: u16,    // Currently unused
    pub max_providers: u16, // Currently unused
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

impl HostLocalSystemService for ControlPlane {}

impl Actor for ControlPlane {
    type Context = Context<Self>;
}

impl Handler<PublishEvent> for ControlPlane {
    type Result = ();

    fn handle(&mut self, msg: PublishEvent, _ctx: &mut Context<Self>) {
        // TODO: implement in the next PR
        /*let evt = msg
            .event
            .into_published(&self.key.as_ref().unwrap().public_key());
        trace!(
            "Control plane instructing provider to publish event {:?}",
            evt
        );
        if let Some(ref p) = self.provider {
            if let Err(e) = p.emit_control_event(evt) {
                error!("Control plane failed to emit event: {}", e);
            }
        } */
    }
}

impl Handler<Initialize> for ControlPlane {
    type Result = ();

    fn handle(&mut self, msg: Initialize, _ctx: &mut Context<Self>) {
        self.key = Some(msg.key);
        self.client = msg.client;
        self.options = msg.control_options;

        // TODO: implement in next PR
        /*
        let mut passed = false;
        self.client = msg.client;

        if let Some(ref mut p) = self.provider.as_mut() {
            if let Err(e) = p.init(controller) {
                error!("Failed to initialize control plane: {}, falling back to null control plane default.", e);
                passed = false;
            }
            let evt = ControlEvent::HostStarted;
            let evt = evt.into_published(&self.key.as_ref().unwrap().public_key());
            if let Err(e) = p.emit_control_event(evt) {
                error!(
                    "Control plane provider failed to emit host started event: {}",
                    e
                );
                passed = false;
            }
            true;
        }
        if !passed {
            self.provider = None;
        }*/
    }
}
