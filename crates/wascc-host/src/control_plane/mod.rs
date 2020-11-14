use crate::control_plane::cpactor::ControlPlane;
use crate::control_plane::events::{ControlEvent, PublishedEvent};
use crate::messagebus::MessageBus;
use crate::Result;
use actix::Addr;
use std::collections::HashMap;

pub(crate) mod cpactor;
pub mod events;

/// A control plane provider is responsible for managing whatever endpoint (or endpoints) is
/// necessary to provide an entrypoint into the functionality exposed via the ControlInterface
/// struct, which is supplied to the control plane provider during the init function.
pub trait ControlPlaneProvider: Sync + Send {
    /// Used to initialize the control plane provider. Use this function to establish whatever connections
    /// and resources are needed
    fn init(&mut self, controller: ControlInterface) -> Result<()>;
    /// Invoked when the host is about to stop to give the provider time to clean up
    fn close(&mut self) -> Result<()>;
    /// Instructs the control plane provider to emit a control plane event. This event
    /// contains a header and the raw event, which should be enough information to allow
    /// the provider to construct a `CloudEvents` event, for example
    fn emit_control_event(&self, event: PublishedEvent) -> Result<()>;
}

/// The control interface is given out to an instance that implements the ControlPlaneProvider
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

    pub fn get_known_links(&self) -> Result<()> {
        Ok(())
    }
}
