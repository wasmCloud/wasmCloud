use wascc_host::{ControlInterface, ControlPlaneProvider, PublishedEvent, Result};

pub struct NatsControlPlaneProvider {
    control: Option<ControlInterface>,
}

impl NatsControlPlaneProvider {
    pub fn new() -> NatsControlPlaneProvider {
        NatsControlPlaneProvider { control: None }
    }
}

impl ControlPlaneProvider for NatsControlPlaneProvider {
    fn init(&mut self, controller: ControlInterface) -> Result<()> {
        self.control = Some(controller);
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        unimplemented!()
    }

    fn emit_control_event(&self, event: PublishedEvent) -> Result<()> {
        unimplemented!()
    }
}
