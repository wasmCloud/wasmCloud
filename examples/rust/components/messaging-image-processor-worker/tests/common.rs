use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context as _, Result};
use merkle_hash::{Encodable as _, MerkleTree};
use reqwest::Client;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::core::ContainerAsync;
use testcontainers_modules::testcontainers::runners::AsyncRunner as _;
use testcontainers_modules::testcontainers::ImageExt as _;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout, Command};

// TODO: replace this with wasmcloud-test-util once we have the newer code released
const DEFAULT_WASH_BIN: &str = "wash";

const DEFAULT_POSTGRES_IMAGE_TAG: &str = "16.4-alpine3.20";
const DEFAULT_POSTGRES_USER: &str = "postgres";
const DEFAULT_POSTGRES_PASSWORD: &str = "postgres";
const DEFAULT_POSTGRES_DATABASE: &str = "postgres";

const DEFAULT_APPLICATION_BASE_URL: &str = "http://localhost:8000";

/// Name of the subscription that will be linked to the component  (see fixtures/test.wadm.yaml)
pub const TEST_SUBSCRIPTION_NAME: &str = "wasmcloud.test";

/// Environment used for a given test
#[allow(unused)]
pub struct TestEnv {
    /// Directory from which to execute wash (contains wasmcloud.toml)
    project_dir: PathBuf,

    /// Path to wash bin
    wash_bin_path: PathBuf,

    /// Path to one or more deployed WADM manifests
    deployed_wadm_manifests: Vec<PathBuf>,

    /// Postgres container
    pg_container: ContainerAsync<Postgres>,

    /// Path to WASM binary
    wasm_path: PathBuf,

    /// Path to WASM binary
    host_process: Child,

    /// Base URL for the application (served by HTTP server provider)
    pub base_url: String,

    /// NATS client that can be used to trigger action
    pub nats_client: async_nats::Client,

    /// HTTP client that can be used to make requests
    pub http_client: Client,
}

/// Setup required environment for an isolated test, including:
///
/// - wasmCloud instance (launched with the component running)
///
/// NOTE: this setup is *NOT* robust (unlike test utilities in wash-cli) -- it expects only one wasmcloud instance
/// to be running at a time.
pub async fn setup_test_env() -> Result<TestEnv> {
    let pg_tag = std::env::var("TEST_POSTGRES_IMAGE_TAG")
        .unwrap_or_else(|_| DEFAULT_POSTGRES_IMAGE_TAG.into());
    // TODO: ensure to pull the default images *before* starting the test suite in CI
    let pg_container = Postgres::default()
        .with_tag(pg_tag)
        .start()
        .await
        .context("failed to start postgres container")?;

    let wash_bin_path =
        PathBuf::from(std::env::var("TEST_WASH_BIN").unwrap_or_else(|_| DEFAULT_WASH_BIN.into()));
    let project_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src_dir = project_dir.join("src");
    let test_scratch_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));

    // `wash build` the component, but only if the source code has changed
    // NOTE: this will ignore changes to cargo.toml which may be significant
    let src_merkle_hash = MerkleTree::builder(format!("{}", src_dir.display()))
        .build()
        .context("failed to build hash of src dir")?
        .root
        .item
        .hash
        .to_hex_string();
    let merkle_wasm_path = test_scratch_dir.join(format!("{src_merkle_hash}.wasm"));

    // If the merkle hashed wasm doesn't exist, then build it
    if !fs::try_exists(&merkle_wasm_path).await? {
        let output = Command::new(&wash_bin_path)
            .current_dir(&project_dir)
            .arg("build")
            .output()
            .await
            .context("failed to run wash build")?;
        assert!(output.status.success(), "wash build succeeded");

        // Retrieve the WASM built by wash
        let built_wasm_path = project_dir.join("build/messaging_image_processor_worker_s.wasm");
        assert!(
            fs::try_exists(&built_wasm_path).await.is_ok_and(|v| v),
            "generated wasm exists"
        );

        // Copy the newly built wasm path to the merkle
        fs::copy(&built_wasm_path, &merkle_wasm_path)
            .await
            .context("failed to copy built wasm to merkle hash name")?;
    }

    // Start the host with wash
    let mut host_process = Command::new(&wash_bin_path)
        .current_dir(&project_dir)
        .arg("up")
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to run wash build")?;

    // Wait until stdout reflects a started host
    let stdout = host_process
        .stdout
        .take()
        .context("failed to get stdout from host child process")?;
    let stdout_reader = BufReader::new(stdout);
    let mut stdout_lines = stdout_reader.lines();
    let stdout_lines = tokio::time::timeout(Duration::from_secs(5), async move {
        loop {
            if let Ok(Some(l)) = stdout_lines
                .next_line()
                .await
                .context("failed to get next line of stdout")
            {
                if l.contains("wasmCloud host started") {
                    return Ok::<Lines<BufReader<ChildStdout>>, anyhow::Error>(stdout_lines);
                }
            }
        }
    })
    .await
    .context("failed to find host stated line (is host already running/left over?)")?
    .context("failed to get stdout object back")?;
    host_process
        .stdout
        .replace(stdout_lines.into_inner().into_inner());

    let test_wadm_path = project_dir.join("tests/fixtures/test.wadm.yaml");
    assert!(
        fs::try_exists(&test_wadm_path).await.is_ok_and(|v| v),
        "test.wadm.yaml exists"
    );

    // Load config for postgres
    let output = Command::new(&wash_bin_path)
        .current_dir(&project_dir)
        .arg("config")
        .arg("put")
        .arg("test-default-postgres") // see: fixtures/test/test.wadm.yaml
        .arg(format!("POSTGRES_HOST={}", pg_container.get_host().await?))
        .arg(format!(
            "POSTGRES_PORT={}",
            pg_container.get_host_port_ipv4(5432).await?,
        ))
        .arg(format!("POSTGRES_USERNAME={DEFAULT_POSTGRES_USER}"))
        .arg(format!("POSTGRES_PASSWORD={DEFAULT_POSTGRES_PASSWORD}"))
        .arg(format!("POSTGRES_DATABASE={DEFAULT_POSTGRES_DATABASE}"))
        .arg("POSTGRES_TLS_REQUIRED=false")
        .output()
        .await
        .context("failed to save postgres config")?;
    assert!(output.status.success(), "wash config put succeeded");

    // Load config for messaging
    let output = Command::new(&wash_bin_path)
        .current_dir(&project_dir)
        .arg("config")
        .arg("put")
        .arg("test-default-messaging") // see: fixtures/test/test.wadm.yaml
        .arg(format!("subscriptions={TEST_SUBSCRIPTION_NAME}"))
        .output()
        .await
        .context("failed to save messaging config")?;
    assert!(output.status.success(), "wash config put succeeded");

    // Run wash app deploy on test.wadm.yaml
    let output = Command::new(&wash_bin_path)
        .current_dir(&project_dir)
        .arg("app")
        .arg("deploy")
        .arg(format!("{}", test_wadm_path.display()))
        .output()
        .await
        .context("failed to run wash app deploy")?;
    assert!(output.status.success(), "wash app deploy succeeded");

    // Wait until the application is accessible, this can take a whlie for multiple reasons:
    // - sqldb provider download and start
    // - http server provider setup
    tokio::time::timeout(Duration::from_secs(20), async move {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if reqwest::get(format!("{DEFAULT_APPLICATION_BASE_URL}/ready"))
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                return;
            }
        }
    })
    .await
    .context("failed to access running application")?;

    let nats_client = async_nats::connect("127.0.0.1:4222")
        .await
        .expect("should be able to connect to local NATS");

    let http_client = reqwest::Client::builder()
        .build()
        .context("failed to build http client")?;

    // // Wait until the component is loaded
    // eprintln!("ABOUT TO START LOOKING");
    // loop {
    //     tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    //     let output_json: serde_json::Value = serde_json::from_slice(
    //         &Command::new(&wash_bin_path)
    //             .current_dir(&project_dir)
    //             .arg("get")
    //             .arg("inventory")
    //             .arg("--output")
    //             .arg("json")
    //             .output()
    //             .await
    //             .context("failed to run wash get inventory")?
    //             .stdout,
    //     )
    //     .context("failed to parse get inventory output")?;
    //     eprintln!("inventories: {:#?}", output_json);
    //     if output_json["inventories"]
    //         .as_array()
    //         .context("failed to open array")?
    //         .iter()
    //         .find(|inv| {
    //             inv["components"].as_array().is_some_and(|cs| {
    //                 cs.iter()
    //                     .find(|c| c["name"] == "messaging-image-processor")
    //                     .is_some()
    //             })
    //         })
    //         .is_some()
    //     {
    //         break;
    //     }
    // }

    Ok(TestEnv {
        wash_bin_path,
        deployed_wadm_manifests: vec![test_wadm_path],
        pg_container,
        project_dir,
        wasm_path: merkle_wasm_path,
        host_process,
        base_url: DEFAULT_APPLICATION_BASE_URL.into(),
        nats_client,
        http_client,
    })
}

/// Tear down a test environment that was set up for an isolated test
impl Drop for TestEnv {
    fn drop(&mut self) {
        tokio::task::block_in_place(move || {
            tokio::runtime::Handle::current().block_on(async move {
                for manifest_path in self.deployed_wadm_manifests.iter() {
                    Command::new(&self.wash_bin_path)
                        .current_dir(&self.project_dir)
                        .arg("app")
                        .arg("delete")
                        .arg(format!("{}", manifest_path.display()))
                        .output()
                        .await
                        .expect("failed to delete wadm manifest");
                }

                Command::new(&self.wash_bin_path)
                    .current_dir(&self.project_dir)
                    .arg("down")
                    .arg("--all")
                    .output()
                    .await
                    .expect("failed to run wash down");
            });
        });
    }
}
