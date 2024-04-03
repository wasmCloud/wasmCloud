mod common;

use common::{
    find_open_port, init, start_nats, test_dir_with_subfolder, wait_for_no_hosts, wait_for_no_nats,
};

use std::sync::Arc;

use anyhow::{anyhow, bail};
#[cfg(target_family = "unix")]
use anyhow::{Context, Result};
use tokio::{process::Command, sync::RwLock, time::Duration};

#[tokio::test]
#[serial_test::serial]
#[cfg(target_family = "unix")]
async fn integration_dev_hello_actor_serial() -> Result<()> {
    wait_for_no_hosts()
        .await
        .context("unexpected wasmcloud instance(s) running")?;
    let test_setup = init(
        /* actor_name= */ "hello",
        /* template_name= */ "hello-world-rust",
    )
    .await?;
    let project_dir = test_setup.project_dir;

    let dir = test_dir_with_subfolder("dev_hello_actor");

    wait_for_no_hosts()
        .await
        .context("one or more unexpected wasmcloud instances running")?;

    let nats_port = find_open_port().await?;
    let mut nats = start_nats(nats_port, &dir).await?;

    let dev_cmd = Arc::new(RwLock::new(
        Command::new(env!("CARGO_BIN_EXE_wash"))
            .args([
                "dev",
                "--nats-port",
                nats_port.to_string().as_ref(),
                "--nats-connect-only",
                "--ctl-port",
                nats_port.to_string().as_ref(),
                "--use-host-subprocess",
                "--disable-wadm",
            ])
            .kill_on_drop(true)
            .spawn()
            .context("failed running cargo dev")?,
    ));
    let watch_dev_cmd = dev_cmd.clone();

    let signed_file_path = Arc::new(project_dir.join("build/http_hello_world_s.wasm"));
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

    wait_for_no_hosts()
        .await
        .context("wasmcloud instance failed to exit cleanly (processes still left over)")?;

    // Kill the nats instance
    nats.kill().await.map_err(|e| anyhow!(e))?;

    wait_for_no_nats()
        .await
        .context("nats instance failed to exit cleanly (processes still left over)")?;

    Ok(())
}
