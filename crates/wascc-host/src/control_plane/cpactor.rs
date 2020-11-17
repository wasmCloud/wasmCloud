use crate::control_plane::{ControlInterface, ControlPlaneProvider};
use crate::hlreg::HostLocalSystemService;
use crate::messagebus::MessageBus;
use crate::{ControlEvent, Result};
use actix::prelude::*;
use std::collections::HashMap;
use wascap::prelude::KeyPair;

#[derive(Default)]
pub struct ControlPlane {
    provider: Option<Box<dyn ControlPlaneProvider>>,
    key: Option<KeyPair>,
    options: ControlOptions,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct Initialize {
    pub provider: Option<Box<dyn ControlPlaneProvider>>,
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
        let evt = msg
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
        }
    }
}

impl Handler<Initialize> for ControlPlane {
    type Result = ();

    fn handle(&mut self, msg: Initialize, ctx: &mut Context<Self>) {
        self.key = Some(msg.key);
        let controller = ControlInterface {
            labels: msg.control_options.host_labels.clone(),
            bus: MessageBus::from_hostlocal_registry(&self.key.as_ref().unwrap().public_key()),
            control_plane: ctx.address(),
        };

        let mut passed = false;
        self.provider = msg.provider;
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
        }

        self.options = msg.control_options;
    }
}
