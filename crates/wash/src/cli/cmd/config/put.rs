use std::collections::HashMap;

use serde_json::json;
use crate::lib::cli::{CliConnectionOpts, CommandOutput, OutputKind};
use crate::lib::config::WashConnectionOptions;
use wasmcloud_secrets_types::SECRET_PREFIX;

use crate::appearance::spinner::Spinner;
use crate::errors::suggest_run_host_error;

/// Invoke a `wash config put` command
pub async fn invoke(
    opts: CliConnectionOpts,
    name: &str,
    values: HashMap<String, String>,
    output_kind: OutputKind,
) -> anyhow::Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    let is_secret = name.starts_with(SECRET_PREFIX);
    let msg = if is_secret {
        "Putting secret configuration ..."
    } else {
        "Putting configuration ..."
    };
    sp.update_spinner_message(msg.to_string());

    let wco: WashConnectionOptions = opts.try_into()?;
    let ctl_client = wco.into_ctl_client(None).await?;
    // Handle no responders by suggesting a host needs to be running
    let config_response = ctl_client
        .put_config(name, values)
        .await
        .map_err(suggest_run_host_error)?;

    sp.finish_and_clear();

    let message = if config_response.succeeded() {
        let mut out_name = name;
        let mut config_type = "Configuration";
        if is_secret {
            out_name = out_name
                .strip_prefix(format!("{SECRET_PREFIX}_").as_str())
                .unwrap_or(name);
            config_type = "Secret";
        };
        format!("{config_type} '{out_name}' put successfully.")
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
