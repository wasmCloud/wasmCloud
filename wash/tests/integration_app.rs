use std::path::PathBuf;
use std::{ffi::OsStr, process::Stdio, time::Duration};

use anyhow::{bail, Context, Result};
use nkeys::KeyPair;

use tokio::fs::read_to_string;

use tokio::process::Command;

mod common;
use common::{
    init, start_nats, test_dir_with_subfolder, wait_for_nats_to_start, wait_for_no_hosts,
    wait_for_no_nats, wait_for_single_host, wash, TestSetup,
};
use wadm::model::{Manifest, VERSION_ANNOTATION_KEY};

const NATS_PORT: u16 = 5893;
const APP_ACTOR_NAME: &str = "hello";
const APP_TEMPLATE_NAME: &str = "hello";

struct TestWorkspace {
    project: TestSetup,
    test_dir: PathBuf,
    nats: tokio::process::Child,
    nats_port: u16,
    host_seed: KeyPair,
    wash_instance_kill_cmd: String,
    manifest: Manifest,
}

impl TestWorkspace {
    async fn set_manifest_version(&mut self, version: &str) -> Result<()> {
        self.manifest
            .metadata
            .annotations
            .insert(VERSION_ANNOTATION_KEY.to_string(), version.to_string());
        tokio::fs::write(
            self.project.project_dir.join("wadm.yaml"),
            serde_yaml::to_string(&self.manifest)
                .context("could not serialize manifest into yaml string")?,
        )
        .await
        .context("could not write manifest to file")?;
        Ok(())
    }

    async fn try_new() -> Result<Self> {
        let test_setup = init(
            /* actor_name= */ APP_ACTOR_NAME,
            /* template_name= */ APP_TEMPLATE_NAME,
        )
        .await?;
        let project_dir = test_setup.project_dir.to_owned();
        let test_dir = test_dir_with_subfolder("wash_app_deploy");

        run_cmd(
            "`wash down` to ensure clean slate before running tests".to_string(),
            env!("CARGO_BIN_EXE_wash"),
            ["down"],
            Stdio::piped(),
            Stdio::piped(),
            true,
        )
        .await?;

        wait_for_no_nats()
            .await
            .context("one or more unexpected nats-server instances running")?;
        let nats = start_nats(NATS_PORT, &test_dir).await?;
        wait_for_nats_to_start()
            .await
            .context("nats process not running")?;

        wait_for_no_hosts()
            .await
            .context("one or more unexpected wasmcloud instances running")?;
        let host_seed = nkeys::KeyPair::new_server();

        run_cmd(
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
            tokio::fs::File::create(test_dir.join("wash_up.ouput"))
                .await
                .context("could not create log file for wash up command")?
                .into_std()
                .await
                .into(),
            true,
        )
        .await?;

        let wash_instance_kill_cmd = match serde_json::from_str::<serde_json::Value>(
            &read_to_string(test_dir.join("wash_up.ouput"))
                .await
                .context("could not read output of wash up command")?,
        ) {
            Ok(v) => v["kill_cmd"].to_owned().to_string(),
            Err(e) => bail!("Unable to parse kill cmd from wash up output: {}", e),
        };

        wait_for_single_host(NATS_PORT, Duration::from_secs(15), Duration::from_secs(1)).await?;

        Ok(Self {
            project: test_setup,
            test_dir,
            nats,
            nats_port: NATS_PORT,
            host_seed,
            wash_instance_kill_cmd,
            manifest: serde_yaml::from_str::<Manifest>(
                read_to_string(project_dir.join("wadm.yaml"))
                    .await
                    .context("could not read wadm.yaml")?
                    .as_str(),
            )
            .context("could not parse wadm.yaml content into Manifest object")?,
        })
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        println!("[integration_app::TestWorkspace::drop] runnning test workspace clean up...");
        let TestWorkspace {
            project,
            wash_instance_kill_cmd,
            host_seed,
            nats,
            nats_port,
            test_dir,
            ..
        } = self;

        let (_, down) = wash_instance_kill_cmd
            .trim_matches('"')
            .split_once(' ')
            .unwrap();
        wash()
            .args(vec![
                down,
                "--host-id",
                &host_seed.public_key(),
                "--ctl-port",
                &nats_port.to_string(),
            ])
            .output()
            .expect("[integration_app::TestWorkspace::drop] wash instance kill command failed");

        nats.start_kill()
            .expect("[integration_app::TestWorkspace::drop] nats kill command failed");

        std::fs::remove_dir_all(test_dir)
            .expect("[integration_app::TestWorkspace::drop] failed to remove temporary `test_dir` directory during cleanup");

        std::fs::remove_dir_all(&project.project_dir)
            .expect("[integration_app::TestWorkspace::drop] failed to remove temporary `project_dir` directory during cleanup");
    }
}

#[tokio::test]
async fn integration_can_deploy_app() -> Result<()> {
    let mut test_workspace = TestWorkspace::try_new().await?;
    test_workspace.set_manifest_version("v0.1.0").await?;

    // Note(ahmedtadde): everything works until we get here... error log
    //     running 1 test
    // 🔧   Cloning template from repo wasmcloud/project-templates subfolder actor/hello...
    // 🔧   Using template subfolder actor/hello...
    // 🔧   Generating template...
    // ✨   Done! New project created /private/var/folders/47/80g4yscn7t58njrqm47wjg500000gn/T/.tmpEMoMNS/hello

    // Project generated and is located at: /private/var/folders/47/80g4yscn7t58njrqm47wjg500000gn/T/.tmpEMoMNS/hello
    // ==================================
    // executing command(name=`wash down` to ensure clean slate before running tests)
    // ...command executed successfully
    // ==================================
    // ==================================
    // executing command(name=`wash up`)
    // ...command executed successfully
    // ==================================
    // ==================================
    // executing command(name=`wash app deploy` w/ local manifest file)

    // Could not put manifest to deploy Internal storage error
    run_cmd(
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
        true,
    )
    .await?;

    // run_cmd(
    //     "`wash app deploy` w/ remote manifest file".to_string(),
    //     env!("CARGO_BIN_EXE_wash"),
    //     [
    //         "app",
    //         "deploy",
    //         "https://raw.githubusercontent.com/wasmCloud/examples/main/actor/hello/wadm.yaml",
    //         "--ctl-port",
    //         NATS_PORT.to_string().as_str(),
    //     ],
    //     Stdio::piped(),
    //     Stdio::piped(),
    //     true,
    // )
    // .await?;
    // run_cmd(
    //     "`wash app undeploy` to cleanup after `wash app deploy` w/ remote manifest file"
    //         .to_string(),
    //     env!("CARGO_BIN_EXE_wash"),
    //     [
    //         "app",
    //         "undeploy",
    //         "hello",
    //         "--ctl-port",
    //         NATS_PORT.to_string().as_str(),
    //     ],
    //     Stdio::piped(),
    //     Stdio::piped(),
    //     true,
    // )
    // .await?;

    // run_cmd(
    //     "`wash app del` to cleanup after `wash app deploy` w/ remote manifest file".to_string(),
    //     env!("CARGO_BIN_EXE_wash"),
    //     [
    //         "app",
    //         "del",
    //         "hello",
    //         "--delete-all",
    //         "--ctl-port",
    //         NATS_PORT.to_string().as_str(),
    //     ],
    //     Stdio::piped(),
    //     Stdio::piped(),
    //     true,
    // )
    // .await?;

    // test_workspace.set_manifest_version("v0.2.0").await?;
    // run_cmd(
    //     "`wash app deploy` w/ local manifest file piped into stdin".to_string(),
    //     env!("CARGO_BIN_EXE_wash"),
    //     [
    //         "app",
    //         "deploy",
    //         "--ctl-port",
    //         NATS_PORT.to_string().as_str(),
    //     ],
    //     tokio::fs::File::create("wadm.yaml")
    //         .await
    //         .context("could not create file for stdin input")?
    //         .into_std()
    //         .await
    //         .into(),
    //     Stdio::piped(),
    //     true,
    // )
    // .await?;

    Ok(())
}

async fn run_cmd<I, S, T>(
    cmd_name: String,
    cmd: S,
    args: I,
    stdin: T,
    stdout: T,
    expect_success: bool,
) -> Result<()>
where
    I: IntoIterator<Item = S> + std::fmt::Debug,
    S: AsRef<OsStr>,
    T: Into<Stdio>,
{
    println!("==================================");
    println!("executing command(name={})", cmd_name);

    let mut cmd = Command::new(cmd)
        .args(args)
        .kill_on_drop(true)
        .stdin(stdin)
        .stdout(stdout)
        .spawn()
        .context("could not spawn process for command")?;

    let status = cmd
        .wait()
        .await
        .context("command failed to execute and complete command")?;

    assert_eq!(
        status.success(),
        expect_success,
        "unexpected command status: expected status.success={:?} instead of status.success={:?} w/ status.code={:?}",
        expect_success,
        status.success(),
        status.code()
    );

    println!("...command executed successfully");
    println!("==================================");
    Ok(())
}
