use testcontainers::{core::WaitFor, Image};

#[derive(Default, Debug, Clone)]
pub struct NatsServer {
    _priv: (),
}

impl Image for NatsServer {
    fn name(&self) -> &str {
        "library/nats"
    }

    fn tag(&self) -> &str {
        "2.10.18-linux"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stderr("Server is ready")]
    }
}
