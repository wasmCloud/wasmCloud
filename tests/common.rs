use std::fs::read_to_string;
use std::{
    env,
    fs::{create_dir_all, remove_dir_all},
    path::PathBuf,
};

use anyhow::{bail, Context, Result};
use rand::{distributions::Alphanumeric, Rng};
use tokio::net::TcpStream;

use sysinfo::SystemExt;
use tokio::process::Child;
use wash_lib::start::{ensure_nats_server, start_nats_server, NatsConfig};

#[allow(unused)]
pub(crate) const LOCAL_REGISTRY: &str = "localhost:5001";

#[allow(unused)]
pub(crate) const ECHO_OCI_REF: &str = "wasmcloud.azurecr.io/echo:0.3.4";

#[allow(unused)]
pub(crate) const PROVIDER_HTTPSERVER_OCI_REF: &str = "wasmcloud.azurecr.io/httpserver:0.17.0";

/// Helper function to create the `wash` binary process
#[allow(unused)]
pub(crate) fn wash() -> std::process::Command {
    test_bin::get_test_bin("wash")
}

#[allow(unused)]
pub(crate) fn output_to_string(output: std::process::Output) -> Result<String> {
    String::from_utf8(output.stdout).with_context(|| "Failed to convert output bytes to String")
}

#[allow(unused)]
pub(crate) fn get_json_output(output: std::process::Output) -> Result<serde_json::Value> {
    let output_str = output_to_string(output)?;

    let json: serde_json::Value = serde_json::from_str(&output_str)
        .with_context(|| "Failed to parse json from output string")?;

    Ok(json)
}

#[allow(unused)]
/// Creates a subfolder in the test directory for use with a specific test
/// It's preferred that the same test that calls this function also
/// uses std::fs::remove_dir_all to remove the subdirectory
pub(crate) fn test_dir_with_subfolder(subfolder: &str) -> PathBuf {
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
pub(crate) fn test_dir_file(subfolder: &str, file: &str) -> PathBuf {
    let root_dir = &env::var("CARGO_MANIFEST_DIR").expect("$CARGO_MANIFEST_DIR");
    PathBuf::from(format!("{root_dir}/tests/fixtures/{subfolder}/{file}"))
}

#[allow(unused)]
async fn start_nats(port: u16, nats_install_dir: &PathBuf) -> Result<Child> {
    let nats_binary = ensure_nats_server("v2.8.4", nats_install_dir).await?;
    let config = NatsConfig::new_standalone("127.0.0.1", port, None);
    start_nats_server(nats_binary, std::process::Stdio::null(), config).await
}

const RANDOM_PORT_RANGE_START: u16 = 5000;
const RANDOM_PORT_RANGE_END: u16 = 6000;
const LOCALHOST: &str = "127.0.0.1";

/// Returns an open port on the interface, searching within the range endpoints, inclusive
async fn find_open_port() -> Result<u16> {
    for i in RANDOM_PORT_RANGE_START..=RANDOM_PORT_RANGE_END {
        if let Ok(conn) = TcpStream::connect((LOCALHOST, i)).await {
            drop(conn);
        } else {
            return Ok(i);
        }
    }
    bail!("Failed to find open port for host")
}

#[allow(unused)]
pub(crate) struct TestWashInstance {
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

        self.nats
            .start_kill()
            .expect("failed to start_kill() on nats instance");

        // Check to see if process was removed
        let mut info = sysinfo::System::new_with_specifics(
            sysinfo::RefreshKind::new().with_processes(sysinfo::ProcessRefreshKind::new()),
        );

        info.refresh_processes();

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
        let mut tries: i32 = 30;
        let mut start_message_logs: String = String::new();
        loop {
            start_message_logs = read_to_string(wasmcloud_log.to_string().trim_matches('"'))
                .context("could not read log file output")?;
            if (start_message_logs.contains("Started wasmCloud OTP Host Runtime")) {
                break;
            }
            tries -= 1;
            assert!(tries >= 0);
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        Ok(TestWashInstance {
            test_dir,
            kill_cmd: kill_cmd.to_string(),
            nats,
            nats_port,
            host_id,
        })
    }
}
