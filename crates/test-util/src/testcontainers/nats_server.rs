use std::collections::BTreeMap;

use serde::Serialize;
use testcontainers::{
    core::{ExecCommand, WaitFor},
    Image,
};

const NATS_CONFIG_PATH: &str = "/etc/nats/config.json";

#[derive(Default, Debug, Clone)]
pub struct NatsServer {
    _priv: (),
    config: Option<NatsConfig>,
}

impl NatsServer {
    pub fn with_config(mut self, config: NatsConfig) -> Self {
        self.config = Some(config);
        self
    }

    fn generate_config(&self) -> ExecCommand {
        if let Some(config) = &self.config {
            let json = serde_json::to_string(&config)
                .expect("Parsing should only fail if structs were defined incorrectly.");

            ExecCommand::new(vec![
                "/bin/sh",
                "-c",
                &format!("echo '{json}' > {}", NATS_CONFIG_PATH),
            ])
        } else {
            ExecCommand::new(vec!["/bin/true"])
        }
    }
}

impl Image for NatsServer {
    fn name(&self) -> &str {
        "library/nats"
    }

    fn tag(&self) -> &str {
        "2.10.26-alpine"
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
