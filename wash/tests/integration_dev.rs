use std::{collections::HashMap, sync::Arc};

use anyhow::{anyhow, bail};
#[cfg(target_family = "unix")]
use anyhow::{Context, Result};
use serial_test::serial;
use tokio::{process::Command, sync::RwLock, time::Duration};

mod common;

use crate::common::{init, start_nats, test_dir_with_subfolder};

#[tokio::test]
#[serial]
async fn integration_dev_hello_actor_serial() -> Result<()> {
    let test_setup = init(
        /* actor_name= */ "hello", /* template_name= */ "hello",
    )
    .await?;
    let project_dir = test_setup.project_dir;

    let dir = test_dir_with_subfolder("dev_hello_actor");
    let mut nats = start_nats(5895, &dir).await?;

    let dev_cmd = Arc::new(RwLock::new(
        Command::new(env!("CARGO_BIN_EXE_wash"))
            .args([
                "dev",
                "--nats-port",
                "5895",
                "--nats-connect-only",
                "--ctl-port",
                "5895",
                "--use-host-subprocess",
            ])
            .kill_on_drop(true)
            .envs(HashMap::from([("WASH_EXPERIMENTAL", "true")]))
            .spawn()
            .context("failed running cargo dev")?,
    ));
    let watch_dev_cmd = dev_cmd.clone();

    let signed_file_path = Arc::new(project_dir.join("build/hello_s.wasm"));
    let expected_path = signed_file_path.clone();

    // Wait until the signed file is there (this means dev succeeded)
    let _ = tokio::time::timeout(
        Duration::from_secs(1200),
        tokio::spawn(async move {
            loop {
                // If the command failed (and exited early), bail
                if let Ok(Some(exit_status)) = watch_dev_cmd.write().await.try_wait() {
                    if !exit_status.success() {
                        bail!("dev command failed");
                    }
                }
                // If the file got built, we know dev succeeded
                if expected_path.exists() {
                    break Ok(());
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }),
    )
    .await
    .context("timed out while waiting for file path to get created")?;
    assert!(signed_file_path.exists(), "signed actor file was built",);

    let process_pid = dev_cmd
        .write()
        .await
        .id()
        .context("failed to get child process pid")?;

    // Send ctrl + c signal to stop the process
    // send SIGINT to the child
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(process_pid as i32),
        nix::sys::signal::Signal::SIGINT,
    )
    .expect("cannot send ctrl-c");

    // Wait until the process stops
    let _ = tokio::time::timeout(Duration::from_secs(15), dev_cmd.write().await.wait())
        .await
        .context("dev command did not exit")?;

    nats.kill().await.map_err(|e| anyhow!(e))?;
    Ok(())
}
