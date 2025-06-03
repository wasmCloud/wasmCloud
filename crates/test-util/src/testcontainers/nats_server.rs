use std::collections::BTreeMap;

use serde::Serialize;
use testcontainers::core::{ExecCommand, WaitFor};
use testcontainers::Image;

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
        "2.11.3-alpine"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stderr("Server is ready")]
    }

    // Based on https://github.com/nats-io/nats-docker/blob/v2.10.22/2.10.x/scratch/Dockerfile#L8
    fn entrypoint(&self) -> Option<&str> {
        // This is handled as part of cmd below to allow for better/more customization
        None
    }

    // Based on https://github.com/testcontainers/testcontainers-rs-modules-community/blob/main/src/dex/mod.rs#L162-L170
    fn cmd(&self) -> impl IntoIterator<Item = impl Into<std::borrow::Cow<'_, str>>> {
        if self.config.is_some() {
            let command = format!(
                r#"while [[ ! -f {NATS_CONFIG_PATH} ]]; do sleep 1; echo "Waiting for configuration file..."; done;
                nats-server --config {NATS_CONFIG_PATH}"#,
            );
            vec![String::from("/bin/sh"), String::from("-c"), command]
        } else {
            vec![String::from("nats-server"), String::from("--jetstream")]
        }
    }

    fn exec_before_ready(
        &self,
        _cs: testcontainers::core::ContainerState,
    ) -> testcontainers::core::error::Result<Vec<ExecCommand>> {
        if self.config.is_some() {
            Ok(vec![self.generate_config()])
        } else {
            Ok(vec![])
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct NatsConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_port: Option<u16>,
    pub jetstream: String,
    pub lame_duck_duration: String,
    pub lame_duck_grace_period: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    pub pid_file: String,
    pub port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolver: Option<NatsResolver>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolver_preload: Option<BTreeMap<String, String>>,
    pub server_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_account: Option<String>,
}

impl Default for NatsConfig {
    fn default() -> Self {
        Self {
            http_port: None,
            jetstream: "enabled".to_string(),
            lame_duck_duration: "30s".to_string(),
            lame_duck_grace_period: "10s".to_string(),
            pid_file: "/var/run/nats.pid".to_string(),
            port: 4222,
            server_name: "$SERVER_NAME".to_string(),
            system_account: None,
            operator: None,
            resolver: None,
            resolver_preload: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum NatsResolver {
    Memory {
        // Memory resolver has no fields
    },
}
