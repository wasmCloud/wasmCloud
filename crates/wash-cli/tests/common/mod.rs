use std::fs::read_to_string;
use std::net::TcpListener;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::{
    env,
    fs::{create_dir_all, remove_dir_all},
    path::PathBuf,
};

use anyhow::{bail, Context, Result};
use rand::{distributions::Alphanumeric, Rng};
use sysinfo::{ProcessExt, SystemExt};
use tempfile::TempDir;
use tokio::{
    fs::File,
    io::AsyncWriteExt,
    process::{Child, Command},
    time::Duration,
};

use wash_lib::cli::output::GetHostsCommandOutput;
use wash_lib::start::{ensure_nats_server, start_nats_server, NatsConfig, WASMCLOUD_HOST_BIN};
use wasmcloud_control_interface::Host;

#[allow(unused)]
pub const LOCAL_REGISTRY: &str = "localhost:5001";

#[allow(unused)]
pub const ECHO_OCI_REF: &str = "wasmcloud.azurecr.io/echo:0.3.8";

#[allow(unused)]
pub const PROVIDER_HTTPSERVER_OCI_REF: &str = "wasmcloud.azurecr.io/httpserver:0.19.1";

/// Helper function to create the `wash` binary process
#[allow(unused)]
pub fn wash() -> std::process::Command {
    std::process::Command::new(env!("CARGO_BIN_EXE_wash"))
}

#[allow(unused)]
pub fn output_to_string(output: std::process::Output) -> Result<String> {
    String::from_utf8(output.stdout).with_context(|| "Failed to convert output bytes to String")
}

#[allow(unused)]
pub fn get_json_output(output: std::process::Output) -> Result<serde_json::Value> {
    let output_str = output_to_string(output)?;

    let json: serde_json::Value = serde_json::from_str(&output_str)
        .with_context(|| "Failed to parse json from output string")?;

    Ok(json)
}

#[allow(unused)]
/// Creates a subfolder in the test directory for use with a specific test
/// It's preferred that the same test that calls this function also
/// uses std::fs::remove_dir_all to remove the subdirectory
pub fn test_dir_with_subfolder(subfolder: &str) -> PathBuf {
    let root_dir = &env::var("CARGO_MANIFEST_DIR").expect("$CARGO_MANIFEST_DIR");
    let with_subfolder = PathBuf::from(format!("{root_dir}/tests/fixtures/{subfolder}"));
    remove_dir_all(with_subfolder.clone());
    create_dir_all(with_subfolder.clone());
    with_subfolder
}

#[allow(unused)]
/// Returns a PathBuf by appending the subfolder and file arguments
/// to the test fixtures directory. This does _not_ create the file,
/// so the test is responsible for initialization and modification of this file
pub fn test_dir_file(subfolder: &str, file: &str) -> PathBuf {
    let root_dir = &env::var("CARGO_MANIFEST_DIR").expect("$CARGO_MANIFEST_DIR");
    PathBuf::from(format!("{root_dir}/tests/fixtures/{subfolder}/{file}"))
}

#[allow(unused)]
pub async fn start_nats(port: u16, nats_install_dir: &PathBuf) -> Result<Child> {
    let nats_binary = ensure_nats_server("v2.8.4", nats_install_dir).await?;
    let config = NatsConfig::new_standalone("127.0.0.1", port, None);
    start_nats_server(nats_binary, std::process::Stdio::null(), config).await
}

/// Returns an open port on the interface, searching within the range endpoints, inclusive
pub async fn find_open_port() -> Result<u16> {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .context("failed to bind random port")?
        .local_addr()
        .map(|addr| addr.port())
        .context("failed to get local address from opened TCP socket")
}

#[allow(unused)]
pub struct TestWashInstance {
    pub host_id: String,
    pub test_dir: PathBuf,
    pub nats_port: u16,
    pub kill_cmd: String,
    nats: Child,
}

impl Drop for TestWashInstance {
    fn drop(&mut self) {
        let TestWashInstance {
            test_dir, kill_cmd, ..
        } = self;

        // Attempt to stop the host (this may fail)
        let kill_cmd = kill_cmd.to_string();
        let (_wash, down) = kill_cmd.trim_matches('"').split_once(' ').unwrap();
        wash()
            .args(vec![
                down,
                "--host-id",
                &self.host_id,
                "--ctl-port",
                &self.nats_port.to_string(),
            ])
            .output()
            .expect("Could not spawn wash down process");

        // Attempt to stop NATS
        self.nats
            .start_kill()
            .expect("failed to start_kill() on nats instance");

        remove_dir_all(test_dir).expect("failed to remove temporary directory during cleanup");
    }
}

impl TestWashInstance {
    #[allow(unused)]
    pub async fn create() -> Result<TestWashInstance> {
        let test_id: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(6)
            .map(char::from)
            .collect();
        let test_name = format!("test-{test_id}");
        let test_dir = test_dir_with_subfolder(test_name.as_str());
        let log_path = test_dir.join("washup.log");
        let stdout = tokio::fs::File::create(&log_path)
            .await
            .context("failed to create log file for wash up test {test_name}")?;

        let nats_port = find_open_port().await?;
        let nats = start_nats(nats_port, &test_dir).await?;

        let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_wash"));
        cmd.kill_on_drop(true);

        let host_seed = nkeys::KeyPair::new_server();
        let host_id = host_seed.public_key();

        let mut up_cmd = cmd
            .args([
                "up",
                "--nats-port",
                nats_port.to_string().as_str(),
                "--nats-connect-only",
                "-o",
                "json",
                "--detached",
                "--host-seed",
                &host_seed.seed().expect("Should have a seed for the host"),
            ])
            .stdout(stdout.into_std().await)
            .kill_on_drop(true)
            .spawn()
            .context("Could not spawn wash up process")?;

        let status = up_cmd
            .wait()
            .await
            .context("up command failed to complete")?;

        assert!(status.success());

        let out = read_to_string(&log_path).context("could not read output of wash up")?;

        let (kill_cmd, wasmcloud_log) = match serde_json::from_str::<serde_json::Value>(&out) {
            Ok(v) => (v["kill_cmd"].to_owned(), v["wasmcloud_log"].to_owned()),
            Err(_e) => panic!("Unable to parse kill cmd from wash up output"),
        };

        // Wait until the host starts by checking the logs
        let logs_path = String::from(wasmcloud_log.to_string().trim_matches('"'));
        tokio::time::timeout(Duration::from_secs(15), async move {
            loop {
                match tokio::fs::read_to_string(&logs_path).await {
                    Ok(file_contents) => {
                        if file_contents.contains("started") {
                            // After wasmcloud says it's ready, it still requires some seconds to start up.
                            tokio::time::sleep(Duration::from_secs(3)).await;
                            break;
                        }
                    }
                    _ => {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        })
        .await?;

        Ok(TestWashInstance {
            test_dir,
            kill_cmd: kill_cmd.to_string(),
            nats,
            nats_port,
            host_id,
        })
    }
}

pub struct TestSetup {
    /// The path to the directory for the test.
    /// Added here so that the directory is not deleted until the end of the test.
    #[allow(dead_code)]
    pub test_dir: TempDir,
    /// The path to the created actor's directory.
    #[allow(dead_code)]
    pub project_dir: PathBuf,
}

#[allow(dead_code)]
pub struct WorkspaceTestSetup {
    /// The path to the directory for the test.
    /// Added here so that the directory is not deleted until the end of the test.
    #[allow(dead_code)]
    pub test_dir: TempDir,
    /// The path to the created actor's directory.
    #[allow(dead_code)]
    pub project_dirs: Vec<PathBuf>,
}

/// Inits an actor build test by setting up a test directory and creating an actor from a template.
/// Returns the paths of the test directory and actor directory.
#[allow(dead_code)]
pub async fn init(actor_name: &str, template_name: &str) -> Result<TestSetup> {
    let test_dir = TempDir::new()?;
    std::env::set_current_dir(&test_dir)?;
    let project_dir = init_actor_from_template(actor_name, template_name).await?;
    std::env::set_current_dir(&project_dir)?;
    Ok(TestSetup {
        test_dir,
        project_dir,
    })
}

/// Initializes a new actor from a wasmCloud template, and sets the environment to use the created actor's directory.
#[allow(dead_code)]
pub async fn init_actor_from_template(actor_name: &str, template_name: &str) -> Result<PathBuf> {
    let status = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "new",
            "actor",
            actor_name,
            "--git",
            "wasmcloud/project-templates",
            "--subfolder",
            &format!("actor/{template_name}"),
            "--silent",
            "--no-git-init",
        ])
        .kill_on_drop(true)
        .status()
        .await
        .context("Failed to generate project")?;

    assert!(status.success());

    let project_dir = std::env::current_dir()?.join(actor_name);
    Ok(project_dir)
}

/// Wait until a process has a given count on the current machine
#[allow(dead_code)]
pub async fn wait_until_process_has_count(
    filter: &str,
    predicate: impl Fn(usize) -> bool,
    timeout: Duration,
    check_interval: Duration,
) -> Result<()> {
    // Check to see if process was removed
    let mut info = sysinfo::System::new_with_specifics(
        sysinfo::RefreshKind::new().with_processes(sysinfo::ProcessRefreshKind::new()),
    );

    tokio::time::timeout(timeout, async move {
        loop {
            info.refresh_processes();
            let count = info
                .processes()
                .values()
                .map(|p| p.exe().to_string_lossy())
                .filter(|name| name.contains(filter))
                .count();
            if predicate(count) {
                break;
            };
            tokio::time::sleep(check_interval).await;
        }
    })
    .await
    .context(format!(
        "failed to find satisfactory amount of processes named [{filter}]"
    ))?;

    Ok(())
}

#[allow(dead_code)]
pub async fn wait_for_single_host(
    ctl_port: u16,
    timeout: Duration,
    check_interval: Duration,
) -> Result<Host> {
    tokio::time::timeout(timeout, async move {
        loop {
            let output = Command::new(env!("CARGO_BIN_EXE_wash"))
                .args([
                    "get",
                    "hosts",
                    "--ctl-port",
                    ctl_port.to_string().as_str(),
                    "--output",
                    "json",
                ])
                .output()
                .await
                .context("get host command failed")?;

            // Continue until `wash get hosts` succeeds w/ non-empty content, until timeout
            // this may happen when a NATS instance is unavailable for a certain amount of time
            if !output.status.success() || output.stdout.is_empty() {
                continue;
            }

            let mut cmd_output: GetHostsCommandOutput = serde_json::from_slice(&output.stdout)
                .with_context(|| {
                    format!(
                        "failed to parse get hosts command JSON output: {}",
                        String::from_utf8_lossy(&output.stdout)
                    )
                })?;

            match &cmd_output.hosts[..] {
                [] => {}
                [_h] => break Ok(cmd_output.hosts.remove(0)),
                _ => bail!("unexpected received more than one host"),
            }

            tokio::time::sleep(check_interval).await;
        }
    })
    .await
    .context("failed to wait for single host to exist")?
}

/// Inits an actor build test by setting up a test directory and creating an actor from a template.
/// Returns the paths of the test directory and actor directory.
#[allow(dead_code)]
pub async fn init_workspace(actor_names: Vec<&str>) -> Result<WorkspaceTestSetup> {
    let test_dir = TempDir::new()?;
    std::env::set_current_dir(&test_dir)?;

    let project_dirs: Vec<_> =
        futures::future::try_join_all(actor_names.iter().map(|actor_name| async {
            let project_dir = init_actor_from_template(actor_name, "hello").await?;
            Result::<PathBuf>::Ok(project_dir)
        }))
        .await?;

    let members = actor_names
        .iter()
        .map(|actor_name| format!("\"{actor_name}\""))
        .collect::<Vec<_>>()
        .join(",");
    let cargo_toml = format!(
        "
    [workspace]
    members = [{members}]
    "
    );

    let mut cargo_path = PathBuf::from(test_dir.path());
    cargo_path.push("Cargo.toml");
    let mut file = File::create(cargo_path).await?;
    file.write_all(cargo_toml.as_bytes()).await?;
    Ok(WorkspaceTestSetup {
        test_dir,
        project_dirs,
    })
}

/// Wait for no hosts to be running by checking for process names,
/// expecting that the wasmcloud process invocation contains 'wasmcloud_host'
#[allow(dead_code)]
pub async fn wait_for_no_hosts() -> Result<()> {
    wait_until_process_has_count(
        WASMCLOUD_HOST_BIN,
        |v| v == 0,
        Duration::from_secs(15),
        Duration::from_millis(250),
    )
    .await
    .context("number of hosts running is still non-zero")
}

/// Wait for NATS to start running by checking for process names.
/// expecting that exactly one 'nats-server' process is running
#[allow(dead_code)]
pub async fn wait_for_nats_to_start() -> Result<()> {
    wait_until_process_has_count(
        "nats-server",
        |v| v == 1,
        Duration::from_secs(10),
        Duration::from_secs(1),
    )
    .await
    .context("at least one nats-server process has not started")
}

/// Wait for no nats to be running by checking for process names
#[allow(dead_code)]
pub async fn wait_for_no_nats() -> Result<()> {
    wait_until_process_has_count(
        "nats-server",
        |v| v == 0,
        Duration::from_secs(10),
        Duration::from_millis(250),
    )
    .await
    .context("number of nats-server processes should be zero")
}
