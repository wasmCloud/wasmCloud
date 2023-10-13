use std::{ffi::OsStr, process::Stdio, time::Duration};

use anyhow::{Context, Result};

use tokio::fs::read_to_string;

use tokio::process::Command;

mod common;
use common::{
    init, wait_for_nats_to_start, wait_for_no_hosts, wait_for_no_nats, wait_for_single_host,
    TestSetup, TestWashInstance,
};
use wadm::model::{Manifest, VERSION_ANNOTATION_KEY};

const APP_ACTOR_NAME: &str = "hello";
const APP_TEMPLATE_NAME: &str = "hello";

struct TestWorkspace {
    project: TestSetup,
    wash: TestWashInstance,
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

        wait_for_no_nats()
            .await
            .context("one or more unexpected nats-server instances running")?;

        wait_for_no_hosts()
            .await
            .context("one or more unexpected wasmcloud instances running")?;

        let test_wash_instance = TestWashInstance::create().await?;

        wait_for_nats_to_start()
            .await
            .context("nats process not running")?;

        wait_for_single_host(
            test_wash_instance.nats_port,
            Duration::from_secs(15),
            Duration::from_secs(1),
        )
        .await?;

        run_cmd(
            "`wash app undeploy` to start from a clean slate".to_string(),
            env!("CARGO_BIN_EXE_wash"),
            [
                "app",
                "undeploy",
                "hello",
                "--ctl-port",
                test_wash_instance.nats_port.to_string().as_str(),
            ],
            Stdio::piped(),
            Stdio::piped(),
            true,
        )
        .await?;

        run_cmd(
            "`wash app del` to start from a clean slate".to_string(),
            env!("CARGO_BIN_EXE_wash"),
            [
                "app",
                "del",
                "hello",
                "--delete-all",
                "--ctl-port",
                test_wash_instance.nats_port.to_string().as_str(),
            ],
            Stdio::piped(),
            Stdio::piped(),
            true,
        )
        .await?;

        Ok(Self {
            project: test_setup,
            wash: test_wash_instance,
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
            test_workspace.wash.nats_port.to_string().as_str(),
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
    //         test_workspace.wash.nats_port.to_string().as_str(),
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
    //         test_workspace.wash.nats_port.to_string().as_str(),
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
    //         test_workspace.wash.nats_port.to_string().as_str(),
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
    //         test_workspace.wash.nats_port.to_string().as_str(),
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
