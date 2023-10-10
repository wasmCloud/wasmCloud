use std::path::PathBuf;
use std::{ffi::OsStr, process::Stdio, time::Duration};

use anyhow::{Context, Result};
use tokio::fs::{read_to_string, remove_dir_all};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

mod common;
use common::{init_workspace, start_nats, wait_for_no_hosts, wait_for_single_host};
use wadm::model::{Manifest, VERSION_ANNOTATION_KEY};

const NATS_PORT: u16 = 5893;

struct TestManifest {
    inner: Manifest,
}

impl TestManifest {
    fn new(inner: Manifest) -> Self {
        Self { inner }
    }

    fn version(&self) -> &str {
        self.inner.version()
    }
    fn update_version(&mut self, version: &str) {
        self.inner
            .metadata
            .annotations
            .insert(VERSION_ANNOTATION_KEY.to_string(), version.to_string());
    }

    async fn write(&self, path: &PathBuf) -> Result<()> {
        let content = serde_yaml::to_string(&self.inner)
            .context("could not serialize manifest into yaml string")?;
        tokio::fs::write(path, content)
            .await
            .context("could not write manifest to file")
    }
}

#[tokio::test]
async fn integration_can_deploy_app() -> Result<()> {
    let test_workspace = init_workspace(vec![/* actor_names= */ "hello"]).await?;
    let test_dir = test_workspace.project_dirs.get(0).unwrap();
    std::env::set_current_dir(test_dir)?;
    let mut test_manifest = TestManifest::new(
        serde_yaml::from_str::<Manifest>(
            read_to_string(test_dir.join("wadm.yaml"))
                .await
                .context("could not read wadm.yaml")?
                .as_str(),
        )
        .context("could not parse wadm.yaml content into Manifest object")?,
    );

    let log_path = test_dir.join("washapp.log");
    let stdout = tokio::fs::File::create(&log_path)
        .await
        .context("could not create log file for app deploy tests")?;

    // First, we `wash up` to get things running...
    wait_for_no_hosts()
        .await
        .context("unexpected wasmcloud instance(s) running")?;
    let host_seed = nkeys::KeyPair::new_server();
    let mut nats = start_nats(NATS_PORT, test_dir).await?;

    run_cmd_and_check_status(
        "`wash up`".to_string(),
        env!("CARGO_BIN_EXE_wash"),
        [
            "up",
            "--nats-port",
            NATS_PORT.to_string().as_str(),
            "-o",
            "json",
            "--detached",
            "--host-seed",
            &host_seed.seed().expect("Should have a seed for the host"),
        ],
        Stdio::piped(),
        stdout.into_std().await.into(),
        None,
    )
    .await?;

    let output = read_to_string(&log_path)
        .await
        .context("could not read (wash up) output from log file")?;

    let (wash_up_kill_cmd, _) = match serde_json::from_str::<serde_json::Value>(&output) {
        Ok(v) => (v["kill_cmd"].to_owned(), v["wasmcloud_log"].to_owned()),
        Err(_e) => panic!("Unable to parse kill cmd from wash up output"),
    };

    wait_for_single_host(NATS_PORT, Duration::from_secs(10), Duration::from_secs(1)).await?;

    //NOTE(ahmedtadde): this is normally not needed. From my experience running this test locally, the test may fail on REruns due to the manifest store already having versions used on previous runs. This is a workaround to ensure that the store is clean before running the commands.
    run_cmd_and_check_status(
        "`wash app del` to clean the manifest store before running commands".to_string(),
        env!("CARGO_BIN_EXE_wash"),
        [
            "app",
            "del",
            "hello",
            "--delete-all",
            "--ctl-port",
            NATS_PORT.to_string().as_str(),
        ],
        Stdio::piped(),
        Stdio::piped(),
        None,
    )
    .await?;

    // Next, we test `wash app deploy` (w/ remote manifest file https://raw.githubusercontent.com/wasmCloud/examples/main/actor/hello/wadm.yaml)
    assert_eq!(test_manifest.version(), "v0.0.1");
    run_cmd_and_check_status(
        "`wash app deploy` w/ remote manifest file".to_string(),
        env!("CARGO_BIN_EXE_wash"),
        [
            "app",
            "deploy",
            "https://raw.githubusercontent.com/wasmCloud/examples/main/actor/hello/wadm.yaml",
            "--ctl-port",
            NATS_PORT.to_string().as_str(),
        ],
        Stdio::piped(),
        Stdio::piped(),
        None,
    )
    .await?;

    // Then, we test `wash app deploy` (w/ local manifest file)
    test_manifest.update_version("v0.0.2");
    test_manifest.write(&test_dir.join("wadm.yaml")).await?;
    run_cmd_and_check_status(
        "`wash app deploy` w/ local manifest file".to_string(),
        env!("CARGO_BIN_EXE_wash"),
        [
            "app",
            "deploy",
            "wadm.yaml",
            "--ctl-port",
            NATS_PORT.to_string().as_str(),
        ],
        Stdio::piped(),
        Stdio::piped(),
        None,
    )
    .await?;

    // And, we test `wash app deploy` (w/ local manifest file piped into stdin)
    test_manifest.update_version("v0.0.3");
    test_manifest.write(&test_dir.join("wadm.yaml")).await?;
    run_cmd_and_check_status(
        "`wash app deploy` w/ local manifest file piped into stdin".to_string(),
        env!("CARGO_BIN_EXE_wash"),
        [
            "app",
            "deploy",
            "--ctl-port",
            NATS_PORT.to_string().as_str(),
        ],
        Stdio::piped(),
        Stdio::piped(),
        Some(
            Command::new("cat")
                .args(["wadm.yaml"])
                .kill_on_drop(true)
                .output()
                .await
                .context("failed to cat wadm.yaml for `wash app deploy`")?
                .stdout
                .as_slice(),
        ),
    )
    .await?;

    // Lastly, let's cleanup...
    run_cmd_and_check_status(
        "`wash app del` to cleanup".to_string(),
        env!("CARGO_BIN_EXE_wash"),
        [
            "app",
            "del",
            "hello",
            "--delete-all",
            "--ctl-port",
            NATS_PORT.to_string().as_str(),
        ],
        Stdio::piped(),
        Stdio::piped(),
        None,
    )
    .await?;

    let wash_up_kill_cmd = wash_up_kill_cmd.to_string();
    let (_, wash_up_kill_cmd) = wash_up_kill_cmd.trim_matches('"').split_once(' ').unwrap();

    run_cmd_and_check_status(
        "`wash down` to clean up".to_string(),
        env!("CARGO_BIN_EXE_wash"),
        vec![
            wash_up_kill_cmd,
            "--ctl-port",
            NATS_PORT.to_string().as_str(),
            "--host-id",
            &host_seed.public_key(),
        ],
        Stdio::piped(),
        Stdio::piped(),
        None,
    )
    .await?;

    // Wait until the host process has finished and exited
    wait_for_no_hosts()
        .await
        .context("wasmcloud instance(s) failed to exit cleanly (processes still left over)")?;

    nats.kill()
        .await
        .context("failed to kill nats server process (after `wash down` to clean up)")?;

    remove_dir_all(test_dir)
        .await
        .context("failed to remove test directory (project)")?;

    remove_dir_all(test_workspace.test_dir)
        .await
        .context("failed to remove test directory (workspace")?;

    Ok(())
}

async fn run_cmd_and_check_status<I, S, T>(
    cmd_name: String,
    cmd: S,
    args: I,
    stdin: T,
    stdout: T,
    stdin_input: Option<&[u8]>,
) -> Result<()>
where
    I: IntoIterator<Item = S> + std::fmt::Debug,
    S: AsRef<OsStr>,
    T: Into<Stdio>,
{
    println!(
        "running command(name={}; args={:?}) and will assert success status...",
        cmd_name, args
    );

    let mut cmd = Command::new(cmd)
        .args(args)
        .kill_on_drop(true)
        .stdin(stdin)
        .stdout(stdout)
        .spawn()
        .context("could not spawn process for command")?;

    if let Some(stdin_input) = stdin_input {
        cmd.stdin
            .as_mut()
            .unwrap()
            .write_all(stdin_input)
            .await
            .context("could not write to stdin")?;
    }

    let status = cmd
        .wait()
        .await
        .context("command failed to complete; unable to retrieve exit status")?;

    assert!(
        status.success(),
        "command exited with failure status: {:?}",
        status
    );

    println!("command(name={}) ran successfully...", cmd_name);
    Ok(())
}
