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
        "2.10.22-linux"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stderr("Server is ready")]
    }

    // Based on https://github.com/nats-io/nats-docker/blob/v2.10.22/2.10.x/scratch/Dockerfile#L8
    fn entrypoint(&self) -> Option<&str> {
        Some("/nats-server")
    }

    // Based on https://github.com/nats-io/nats-docker/blob/v2.10.22/2.10.x/scratch/Dockerfile#L9
    fn cmd(&self) -> impl IntoIterator<Item = impl Into<std::borrow::Cow<'_, str>>> {
        vec!["--config", "nats-server.conf", "--jetstream"]
    }
}
