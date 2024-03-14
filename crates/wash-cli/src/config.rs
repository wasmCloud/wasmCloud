use std::{collections::HashMap, error::Error};

use clap::Subcommand;
use serde_json::json;
use tracing::error;
use wash_lib::{
    cli::{input_vec_to_hashmap, CliConnectionOpts, CommandOutput, OutputKind},
    config::WashConnectionOptions,
};

use crate::appearance::spinner::Spinner;

#[derive(Debug, Clone, Subcommand)]
#[allow(clippy::enum_variant_names)]
pub enum ConfigCliCommand {
    /// Put a named configuration
    #[clap(name = "put", alias = "create", about = "Put named configuration")]
    PutCommand {
        #[clap(flatten)]
        opts: CliConnectionOpts,
        /// The name of the configuration to put
        #[clap(name = "name")]
        name: String,
        /// The configuration values to put, in the form of `key=value`. Can be specified multiple times, but must be specified at least once.
        #[clap(name = "config_value", required = true)]
        config_values: Vec<String>,
    },
    /// Get a named configuration
    #[clap(name = "get")]
    GetCommand {
        #[clap(flatten)]
        opts: CliConnectionOpts,
        /// The name of the configuration to get
        #[clap(name = "name")]
        name: String,
    },
    /// Delete a named configuration
    #[clap(name = "del", alias = "delete")]
    DelCommand {
        #[clap(flatten)]
        opts: CliConnectionOpts,
        /// The name of the configuration to delete
        #[clap(name = "name")]
        name: String,
    },
}

pub async fn handle_command(
    command: ConfigCliCommand,
    output_kind: OutputKind,
) -> anyhow::Result<CommandOutput> {
    match command {
        ConfigCliCommand::PutCommand {
            opts,
            name,
            config_values,
        } => {
            put_config(
                opts,
                &name,
                input_vec_to_hashmap(config_values)?,
                output_kind,
            )
            .await
        }
        ConfigCliCommand::GetCommand { opts, name } => get_config(opts, &name, output_kind).await,
        ConfigCliCommand::DelCommand { opts, name } => {
            delete_config(opts, &name, output_kind).await
        }
    }
}

async fn put_config(
    opts: CliConnectionOpts,
    name: &str,
    values: HashMap<String, String>,
    output_kind: OutputKind,
) -> anyhow::Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    sp.update_spinner_message("Putting configuration ...".to_string());
    let wco: WashConnectionOptions = opts.try_into()?;
    let ctl_client = wco.into_ctl_client(None).await?;
    // Handle no responders by suggesting a host needs to be running
    let config_response = ctl_client
        .put_config(name, values)
        .await
        .map_err(suggest_run_host_error)?;

    sp.finish_and_clear();

    let message = if config_response.message.is_empty() && config_response.success {
        format!("Configuration {name} put successfully.")
    } else {
        config_response.message
    };
    let json_out = HashMap::from_iter([
        ("success".to_string(), json!(config_response.success)),
        ("message".to_string(), json!(message)),
    ]);
    let output = CommandOutput::new(message, json_out);

    Ok(output)
}

async fn get_config(
    opts: CliConnectionOpts,
    name: &str,
    output_kind: OutputKind,
) -> anyhow::Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    sp.update_spinner_message("Getting configuration ...".to_string());
    let wco: WashConnectionOptions = opts.try_into()?;
    let ctl_client = wco.into_ctl_client(None).await?;

    let config_response = ctl_client
        .get_config(name)
        .await
        .map_err(suggest_run_host_error)?;

    sp.finish_and_clear();

    if !config_response.message.is_empty() && !config_response.success {
        error!("Error getting configuration: {}", config_response.message);
    };

    if let Some(config) = config_response.response {
        Ok(CommandOutput::new(
            format!("{:?}", config),
            config.into_iter().map(|(k, v)| (k, json!(v))).collect(),
        ))
    } else {
        Err(anyhow::anyhow!("No configuration found for name: {}", name))
    }
}

async fn delete_config(
    opts: CliConnectionOpts,
    name: &str,
    output_kind: OutputKind,
) -> anyhow::Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    sp.update_spinner_message("Deleting configuration ...".to_string());
    let wco: WashConnectionOptions = opts.try_into()?;
    let ctl_client = wco.into_ctl_client(None).await?;

    let config_response = ctl_client
        .delete_config(name)
        .await
        .map_err(suggest_run_host_error)?;

    sp.finish_and_clear();

    let message = if config_response.message.is_empty() && config_response.success {
        format!("Configuration {name} deleted successfully.")
    } else {
        config_response.message
    };
    let json_out = HashMap::from_iter([
        ("success".to_string(), json!(config_response.success)),
        ("message".to_string(), json!(message)),
    ]);
    let output = CommandOutput::new(message, json_out);

    Ok(output)
}

/// Simple helper function to suggest running a host if no responders are found
fn suggest_run_host_error(e: Box<dyn Error + std::marker::Send + Sync>) -> anyhow::Error {
    let err_str = e.to_string();
    if err_str.contains("no responders") {
        anyhow::anyhow!("No responders found for config put request. Is a host running?")
    } else {
        anyhow::anyhow!(e)
    }
}
