use clap::Subcommand;
use std::collections::HashMap;
use wash_lib::cli::{CliConnectionOpts, CommandOutput, OutputKind};
use wasmcloud_secrets_types::{SecretConfig, SECRET_PREFIX, SECRET_TYPE};

use crate::config::{delete_config, get_config, put_config};

#[derive(Debug, Clone, Subcommand)]
pub enum SecretsCliCommand {
    #[clap(name = "put", alias = "create", about = "Put secret reference")]
    PutCommand {
        #[clap(flatten)]
        opts: CliConnectionOpts,
        #[clap(name = "name")]
        name: String,
        // TODO: we should have a type for all that, since we use it in wadm
        backend: String,
        key: String,
        version: Option<String>,
    },

    /// Get a secret reference
    #[clap(name = "get")]
    GetCommand {
        #[clap(flatten)]
        opts: CliConnectionOpts,
        #[clap(name = "name")]
        name: String,
    },

    /// Delete a secret reference
    #[clap(name = "del", alias = "delete")]
    DelCommand {
        #[clap(flatten)]
        opts: CliConnectionOpts,
        #[clap(name = "name")]
        name: String,
    },
}

pub async fn handle_command(
    command: SecretsCliCommand,
    output_kind: OutputKind,
) -> anyhow::Result<CommandOutput> {
    match command {
        SecretsCliCommand::PutCommand {
            opts,
            name,
            backend,
            key,
            version,
        } => {
            let cfg = SecretConfig {
                backend,
                key,
                version,
                secret_type_identifier: SECRET_TYPE.to_string(),
            };
            let values: HashMap<String, String> = cfg.try_into()?;

            put_config(opts, &format_secret_name(&name), values, output_kind).await
        }
        SecretsCliCommand::GetCommand { opts, name } => {
            get_config(opts, &format_secret_name(&name), output_kind).await
        }
        SecretsCliCommand::DelCommand { opts, name } => {
            delete_config(opts, &format_secret_name(&name), output_kind).await
        }
    }
}

fn format_secret_name(name: &str) -> String {
    if !name.starts_with(SECRET_PREFIX) {
        format!("{SECRET_PREFIX}_{}", name)
    } else {
        name.to_string()
    }
}
