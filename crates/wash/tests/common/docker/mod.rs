use anyhow::{Context, Result};
use testcontainers::core::{CmdWaitFor, ExecCommand};
use testcontainers::{ContainerAsync, GenericImage};

pub mod kind;

/// Execute a container command
#[allow(unused)]
pub async fn exec_container_command<const N: usize>(
    container: &ContainerAsync<GenericImage>,
    args: [&str; N],
) -> Result<String> {
    let command: Vec<_> = args.iter().map(|arg| (*arg).to_string()).collect();
    let mut result = container
        .exec(ExecCommand::new(command).with_cmd_ready_condition(CmdWaitFor::exit_code(0)))
        .await
        .with_context(|| {
            format!(
                "failed to execute command in container ID [{}]",
                container.id()
            )
        })?;
    let stdout = result
        .stdout_to_vec()
        .await
        .context("failed to read container command stdout")?;
    let stderr = result
        .stderr_to_vec()
        .await
        .context("failed to read container command stderr")?;
    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    let stderr = String::from_utf8_lossy(&stderr).into_owned();
    if !stderr.trim().is_empty() {
        eprintln!("{stderr}");
    }
    Ok(stdout)
}
