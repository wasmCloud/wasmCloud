use crate::control_plane::actorhost::ControlPlane;
use crate::control_plane::events::ControlEvent;
use crate::messagebus::MessageBus;
use crate::Result;
use actix::Addr;
use std::collections::HashMap;

pub(crate) mod actorhost;
pub mod events;

/// A control plane provider is responsible for managing whatever endpoint (or endpoints) is
/// necessary to provide an entrypoint into the functionality exposed via the ControlInterface
/// struct, which is supplied to the control plane provider during the init function.
pub trait ControlPlaneProvider: Sync + Send {
    fn init(&mut self, controller: ControlInterface) -> Result<()>;
    fn close(&mut self) -> Result<()>;
    fn emit_control_event(&self, event: ControlEvent) -> Result<()>;
}

/// The control interface is given out to a struct that implements the ControlPlaneProvider
/// interface. This "handle" is used to give the control plane provider access to control
/// functionality without knowledge of the host internals.
#[derive(Clone)]
pub struct ControlInterface {
    bus: Addr<MessageBus>,
    control_plane: Addr<ControlPlane>,
    labels: HashMap<String, String>,
}

impl ControlInterface {
    pub fn start_actor(&self) -> Result<()> {
        Ok(())
    }

    pub fn host_labels(&self) -> HashMap<String, String> {
        self.labels.clone()
    }

    pub fn stop_actor(&self) -> Result<()> {
        Ok(())
    }

    pub fn start_provider(&self) -> Result<()> {
        Ok(())
    }

    pub fn stop_provider(&self) -> Result<()> {
        Ok(())
    }

    pub fn get_running_actors(&self) -> Result<()> {
        Ok(())
    }

    pub fn get_running_providers(&self) -> Result<()> {
        Ok(())
    }

    pub fn get_known_bindings(&self) -> Result<()> {
        Ok(())
    }
}
