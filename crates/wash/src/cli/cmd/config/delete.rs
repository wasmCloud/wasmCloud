use std::collections::HashMap;

use serde_json::json;
use crate::lib::cli::{CliConnectionOpts, CommandOutput, OutputKind};
use crate::lib::config::WashConnectionOptions;
use wasmcloud_secrets_types::SECRET_PREFIX;

use crate::appearance::spinner::Spinner;
use crate::errors::suggest_run_host_error;
use crate::secrets::is_secret;

/// Invoke `wash config delete` (sub)command
pub async fn invoke(
    opts: CliConnectionOpts,
    name: &str,
    output_kind: OutputKind,
) -> anyhow::Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    let is_secret = is_secret(name);
    let config_type = if is_secret { "secret" } else { "configuration" };
    sp.update_spinner_message("Deleting {config_type}...".to_string());
    let wco: WashConnectionOptions = opts.try_into()?;
    let ctl_client = wco.into_ctl_client(None).await?;

    let config_response = ctl_client
        .delete_config(name)
        .await
        .map_err(suggest_run_host_error)?;

    sp.finish_and_clear();

    let message = if config_response.succeeded() {
        let mut out_name = name;
        if is_secret {
            out_name = out_name
                .strip_prefix(format!("{SECRET_PREFIX}_").as_str())
                .unwrap_or(name);
        };
        format!("{config_type} '{out_name}' deleted successfully.")
    } else {
        config_response
            .message()
            .replace(&format!("{SECRET_PREFIX}_"), "")
    };
    let json_out = HashMap::from_iter([
        ("success".to_string(), json!(config_response.succeeded())),
        ("message".to_string(), json!(message)),
    ]);
    let output = CommandOutput::new(message, json_out);

    Ok(output)
}
