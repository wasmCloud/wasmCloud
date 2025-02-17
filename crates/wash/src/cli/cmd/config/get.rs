use serde_json::json;
use tracing::error;
use crate::lib::cli::{CliConnectionOpts, CommandOutput, OutputKind};
use crate::lib::config::WashConnectionOptions;
use wasmcloud_secrets_types::SECRET_PREFIX;

use crate::appearance::spinner::Spinner;
use crate::errors::suggest_run_host_error;
use crate::secrets::is_secret;

/// Invoke `wash config get`
pub async fn invoke(
    opts: CliConnectionOpts,
    name: &str,
    output_kind: OutputKind,
) -> anyhow::Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    let config_type = if is_secret(name) {
        "secret"
    } else {
        "configuration"
    };
    sp.update_spinner_message(format!("Getting {config_type}..."));

    let wco: WashConnectionOptions = opts.try_into()?;
    let ctl_client = wco.into_ctl_client(None).await?;

    let config_response = ctl_client
        .get_config(name)
        .await
        .map_err(suggest_run_host_error)?;

    sp.finish_and_clear();

    if !config_response.succeeded() {
        error!("Error getting {config_type}: {}", config_response.message());
    };

    if let Some(config) = config_response.into_data() {
        Ok(CommandOutput::new(
            format!("{config:?}"),
            config.into_iter().map(|(k, v)| (k, json!(v))).collect(),
        ))
    } else {
        Err(anyhow::anyhow!(
            "No {config_type} found for name: {}",
            name.replace(format!("{SECRET_PREFIX}_").as_str(), "")
        ))
    }
}
