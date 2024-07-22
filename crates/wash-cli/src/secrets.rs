use clap::Subcommand;
use std::collections::HashMap;
use tracing::trace;
use wash_lib::cli::{input_vec_to_hashmap, CliConnectionOpts, CommandOutput, OutputKind};
use wasmcloud_secrets_types::{SecretConfig, SECRET_PREFIX};

use crate::config::{delete_config, get_config, is_secret, put_config};

#[derive(Debug, Clone, Subcommand)]
pub enum SecretsCliCommand {
    #[clap(name = "put", alias = "create", about = "Put secret reference")]
    PutCommand {
        #[clap(flatten)]
        opts: CliConnectionOpts,
        /// The name of the secret reference to create.
        #[clap(name = "name")]
        name: String,
        /// The backend to fetch the secret from at runtime.
        #[clap(name = "backend")]
        backend: String,
        /// The key to use for retrieving the secret from the backend.
        #[clap(name = "key")]
        key: String,
        /// The version of the secret to retrieve. If not supplied, the latest version will be used.
        #[clap(short = 'v', long = "version")]
        version: Option<String>,
        /// Freeform policy properties to pass to the secrets backend, in the form of `key=value`. Can be specified multiple times.
        #[clap(long = "property")]
        policy_properties: Vec<String>,
    },

    /// Get a secret reference by name
    #[clap(name = "get")]
    GetCommand {
        #[clap(flatten)]
        opts: CliConnectionOpts,
        #[clap(name = "name")]
        name: String,
    },

    /// Delete a secret reference by name
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
            policy_properties,
        } => {
            let policy_property_map = input_vec_to_hashmap(policy_properties)?;
            let secret_config = SecretConfig::new(backend, key, version, policy_property_map);
            trace!(?secret_config, "Putting secret config");
            let values: HashMap<String, String> = secret_config.try_into()?;

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

/// Ensures the secret name is prefixed by `SECRET_`
fn format_secret_name(name: &str) -> String {
    if is_secret(name) {
        name.to_string()
    } else {
        format!("{SECRET_PREFIX}_{}", name)
    }
}
