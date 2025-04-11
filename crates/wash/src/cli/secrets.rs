use std::collections::HashMap;

use crate::lib::cli::{input_vec_to_hashmap, CliConnectionOpts, CommandOutput, OutputKind};
use anyhow::bail;
use clap::Subcommand;
use tracing::trace;
use wasmcloud_secrets_types::{SecretConfig, SECRET_PREFIX};

use crate::cmd;

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
        /// The field to use for retrieving the secret from the backend.
        #[clap(long = "field")]
        field: Option<String>,
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
            field,
            version,
            policy_properties,
        } => {
            let policy_property_map = input_vec_to_hashmap(policy_properties)?;
            let secret_name = name.clone();
            let secret_config = SecretConfig::new(
                secret_name,
                backend,
                key,
                field,
                version,
                policy_property_map
                    .into_iter()
                    .map(|(k, v)| (k, v.into()))
                    .collect(),
            );
            trace!(?secret_config, "Putting secret config");
            let values: HashMap<String, String> = secret_config.try_into()?;

            cmd::config::put::invoke(opts, &secret_configdata_key(&name), values, output_kind).await
        }
        SecretsCliCommand::GetCommand { opts, name } => {
            cmd::config::get::invoke(opts, &secret_configdata_key(&name), output_kind).await
        }
        SecretsCliCommand::DelCommand { opts, name } => {
            cmd::config::delete::invoke(opts, &secret_configdata_key(&name), output_kind).await
        }
    }
}

/// Ensure that a given config KV name is *not* a secret
pub(crate) fn ensure_not_secret(name: &str) -> anyhow::Result<()> {
    if name.starts_with(SECRET_PREFIX) {
        bail!("Configuration names cannot start with '{SECRET_PREFIX}'. Did you mean to use the 'secrets' command?");
    }
    Ok(())
}

/// Check if a given configuration KV name name represents a secret
pub(crate) fn is_secret(name: &str) -> bool {
    name.starts_with(SECRET_PREFIX)
}

/// Ensures the secret name is prefixed by `SECRET_`
pub(crate) fn secret_configdata_key(name: &str) -> String {
    if is_secret(name) {
        name.to_string()
    } else {
        format!("{SECRET_PREFIX}_{name}")
    }
}
