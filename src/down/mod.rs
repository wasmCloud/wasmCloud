use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use serde_json::json;
use tokio::process::Command;
use wash_lib::cli::{CommandOutput, OutputKind};
use wash_lib::start::*;

use crate::appearance::spinner::Spinner;
use crate::cfg::cfg_dir;
use crate::up::DOWNLOADS_DIR;

#[derive(Parser, Debug, Clone)]
pub(crate) struct DownCommand {}

pub(crate) async fn handle_command(
    command: DownCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    handle_down(command, output_kind).await
}

pub(crate) async fn handle_down(
    _cmd: DownCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let install_dir = cfg_dir()?.join(DOWNLOADS_DIR);
    let sp = Spinner::new(&output_kind)?;

    let mut out_json = HashMap::new();
    let mut out_text = String::from("");
    let host_bin = install_dir.join(WASMCLOUD_HOST_BIN);
    if host_bin.is_file() {
        sp.update_spinner_message(" Stopping host ...".to_string());
        if let Ok(output) = stop_wasmcloud(host_bin).await {
            if output.stderr.is_empty() && output.stdout.is_empty() {
                // if there was a host running, 'stop' has no output.
                // Give it time to stop before stopping nats
                tokio::time::sleep(Duration::from_secs(6)).await;
                out_json.insert("host_stopped".to_string(), json!(true));
                out_text.push_str("✅ wasmCloud host stopped successfully\n");
            } else {
                out_json.insert("host_stopped".to_string(), json!(true));
                out_text.push_str(
                    "🤔 Host did not appear to be running, assuming it's already stopped\n",
                );
            }
        }
    }

    let nats_bin = install_dir.join(NATS_SERVER_BINARY);
    if nats_bin.is_file() {
        sp.update_spinner_message(" Stopping NATS server ...".to_string());
        if let Err(e) = stop_nats(install_dir).await {
            out_json.insert("nats_stopped".to_string(), json!(false));
            out_text.push_str(&format!(
                "❌ NATS server did not stop successfully: {:?}\n",
                e
            ));
        } else {
            out_json.insert("nats_stopped".to_string(), json!(true));
            out_text.push_str("✅ NATS server stopped successfully\n");
        }
    }

    out_json.insert("success".to_string(), json!(true));
    out_text.push_str("🛁 wash down completed successfully");

    sp.finish_and_clear();
    Ok(CommandOutput::new(out_text, out_json))
}

/// Helper function to send wasmCloud the `stop` command and wait for it to clean up
pub(crate) async fn stop_wasmcloud<P>(bin_path: P) -> Result<Output>
where
    P: AsRef<Path>,
{
    Command::new(bin_path.as_ref())
        .stdout(Stdio::piped())
        .arg("stop")
        .output()
        .await
        .map_err(anyhow::Error::from)
}

/// Helper function to send the nats-server the stop command
pub(crate) async fn stop_nats<P>(install_dir: P) -> Result<Output>
where
    P: AsRef<Path>,
{
    let bin_path = install_dir.as_ref().join(NATS_SERVER_BINARY);
    let pid_file = nats_pid_path(install_dir);
    let signal = if pid_file.is_file() {
        format!("stop={}", &pid_file.display())
    } else {
        "stop".into()
    };
    let output = Command::new(bin_path)
        .arg("--signal")
        .arg(signal)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(anyhow::Error::from);

    // remove PID file
    if pid_file.is_file() {
        let _ = tokio::fs::remove_file(&pid_file).await;
    }
    output
}

/// Helper function to get the path to the NATS server pid file
pub(crate) fn nats_pid_path<P>(install_dir: P) -> PathBuf
where
    P: AsRef<Path>,
{
    install_dir.as_ref().join(NATS_SERVER_PID)
}
