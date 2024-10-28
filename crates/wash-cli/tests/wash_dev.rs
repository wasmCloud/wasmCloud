#![cfg(target_family = "unix")]

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _, Result};
use futures::future::Either;
use nkeys::XKey;
use serde_json::json;
use tempfile::NamedTempFile;
use tokio::sync::RwLock;
use tokio::time::Duration;
use tokio::{io::AsyncWriteExt, process::Child};
use wadm_types::{Manifest, Properties};
use wasmcloud_control_interface::{
    Client as CtlClient, ClientBuilder as CtlClientBuilder, ComponentDescription, Host,
};

mod common;
use common::{
    find_open_port, init, start_nats, test_dir_with_subfolder, wait_for_no_hosts, wait_for_no_nats,
    wait_for_no_wadm, TestSetup,
};
use wasmcloud_secrets_types::SecretConfig;

/// The version of `nats-kv-secrets` to use with tests
const NATS_KV_SECRETS_VERSION: &str = "v0.1.1-rc.0";

/// Public key for http-password-checker component, used to enable secrets access
///
/// (this ID should match ghcr.io/wasmcloud/components/http-password-checker-rust:0.1.0)
const HTTP_PASSWORD_CHECKER_COMPONENT_PUBLIC_KEY: &str =
    "MB2AHZUIL6B5I32YAQD7IDHFTVKGBFEFI6XIAWVJYU7GXDHW53RZPRNP";

#[tokio::test]
#[serial_test::serial]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_dev_hello_component_serial() -> Result<()> {
    let setup = wash_dev_test_setup(WashDevTestSetupArgs {
        test_name: "dev_hello_component".into(),
        template_init: Some(("hello".into(), "hello-world-rust".into())),
        start_nats_kv_args: None,
    })
    .await?;

    let nats_port = setup
        .nats
        .as_ref()
        .context("missing nats setup for test")?
        .1;
    let test_setup = setup
        .test_setup
        .as_ref()
        .context("missing test setup after template init")?;
    let dev_cmd = Arc::new(RwLock::new(
        test_setup
            .base_command()
            .args([
                "dev",
                "--nats-connect-only",
                "--nats-port",
                nats_port.to_string().as_ref(),
                "--ctl-port",
                nats_port.to_string().as_ref(),
                "--rpc-port",
                nats_port.to_string().as_ref(),
            ])
            .kill_on_drop(true)
            .spawn()
            .context("failed running cargo dev")?,
    ));
    let watch_dev_cmd = dev_cmd.clone();

    let signed_file_path = Arc::new(setup.project_dir.join("build/http_hello_world_s.wasm"));
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
    assert!(signed_file_path.exists(), "signed component file was built");

    teardown_test(dev_cmd, setup).await?;

    wait_for_no_wadm()
        .await
        .context("wadm instance failed to exit cleanly (processes still left over)")?;

    Ok(())
}

/// Ensure that overriding manifest YAML works
#[tokio::test]
#[serial_test::serial]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_dev_override_manifest_yaml_serial() -> Result<()> {
    let setup = wash_dev_test_setup(WashDevTestSetupArgs {
        test_name: "wash_dev_integration_override_manifest_yaml".into(),
        template_init: Some(("hello".into(), "hello-world-rust".into())),
        start_nats_kv_args: None,
    })
    .await?;

    // Write out the fixture configuration to disk
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("./tests/fixtures/wadm/hello-world-rust-dev-override.yaml");
    tokio::fs::write(
        setup.project_dir.join("test.wadm.yaml"),
        tokio::fs::read(&fixture_path)
            .await
            .with_context(|| format!("failed to read fixture @ [{}]", fixture_path.display()))?,
    )
    .await
    .context("failed to write out fixture file")?;

    // Manipulate the wasmcloud.toml for the test project and override the manifest
    let wasmcloud_toml_path = setup.project_dir.join("wasmcloud.toml");
    let mut wasmcloud_toml = tokio::fs::File::options()
        .append(true)
        .open(&wasmcloud_toml_path)
        .await
        .with_context(|| {
            format!(
                "failed to open wasmcloud toml file @ [{}]",
                wasmcloud_toml_path.display()
            )
        })?;
    wasmcloud_toml
        .write_all(
            r#"
[dev]
manifests = [
  { component_name = "http-handler", path = "test.wadm.yaml" }
]
"#
            .as_bytes(),
        )
        .await
        .context("failed tow write dev configuration content to file")?;
    wasmcloud_toml.flush().await?;

    // Run wash dev
    let nats_port = setup
        .nats
        .as_ref()
        .context("missing nats setup for test")?
        .1;
    let test_setup = setup
        .test_setup
        .as_ref()
        .context("missing test setup after template init")?;
    let dev_cmd = Arc::new(RwLock::new(
        test_setup
            .base_command()
            .args([
                "dev",
                "--nats-port",
                nats_port.to_string().as_ref(),
                "--nats-connect-only",
                "--ctl-port",
                nats_port.to_string().as_ref(),
                "--rpc-port",
                nats_port.to_string().as_ref(),
            ])
            .kill_on_drop(true)
            .spawn()
            .context("failed running cargo dev")?,
    ));
    let watch_dev_cmd = dev_cmd.clone();

    // Get the host that was created
    let host = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(h) = setup
                .ctl_client
                .get_hosts()
                .await
                .map_err(|e| anyhow!("failed to get hosts: {e}"))
                .context("get components")?
                .into_iter()
                .map(|v| v.into_data())
                .next()
            {
                return Ok::<Option<Host>, anyhow::Error>(h);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .context("timed out waiting for host to start up")?
    .context("failed to get the host")?;
    let host_id = host
        .as_ref()
        .context("host was missing from request")?
        .id()
        .to_string();

    // Wait until the ferris-says component is present on the host
    let ctl_client = setup.ctl_client.clone();
    let _ = tokio::time::timeout(
        Duration::from_secs(60),
        tokio::spawn(async move {
            loop {
                // If the command failed (and exited early), bail
                if let Ok(Some(exit_status)) = watch_dev_cmd.write().await.try_wait() {
                    if !exit_status.success() {
                        bail!("dev command failed");
                    }
                }
                // If the file got built, we know dev succeeded
                let host_inventory = ctl_client
                    .get_host_inventory(&host_id)
                    .await
                    .map_err(|e| anyhow!(e))
                    .map(|v| v.into_data())
                    .context("failed to get host inventory");
                if host_inventory.is_ok_and(|inv| {
                    inv.is_some_and(|cs| {
                        cs.components()
                            .iter()
                            .any(|c| c.name() == Some("ferris-says"))
                    })
                }) {
                    break Ok(()) as anyhow::Result<()>;
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }),
    )
    .await
    .context("timed out while waiting for file path to get created")?;

    teardown_test(dev_cmd, setup).await?;

    wait_for_no_wadm()
        .await
        .context("wadm instance failed to exit cleanly (processes still left over)")?;

    Ok(())
}

/// Ensure that overriding by interface via project config YAML works
#[tokio::test]
#[serial_test::serial]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_dev_override_via_interface_serial() -> Result<()> {
    let setup = wash_dev_test_setup(WashDevTestSetupArgs {
        test_name: "wash_dev_integration_override_via_interface".into(),
        template_init: Some(("hello".into(), "hello-world-rust".into())),
        start_nats_kv_args: None,
    })
    .await?;

    // Create a dir for generated manifests
    let generated_manifests_dir = setup.project_dir.join("generated-manifests");
    tokio::fs::create_dir(&generated_manifests_dir).await?;

    // Manipulate the wasmcloud.toml for the test project and override the manifest
    let wasmcloud_toml_path = setup.project_dir.join("wasmcloud.toml");
    let mut wasmcloud_toml = tokio::fs::File::options()
        .append(true)
        .open(&wasmcloud_toml_path)
        .await
        .with_context(|| {
            format!(
                "failed to open wasmcloud toml file @ [{}]",
                wasmcloud_toml_path.display()
            )
        })?;
    wasmcloud_toml
        .write_all(
            r#"
[[dev.overrides.imports]]
interface =  "wasi:http/incoming-handler@0.2.0"
config = { name = "value" }
secrets = { name = "existing-secret", source = { policy = "nats-kv", key = "test" } }
image_ref = "ghcr.io/wasmcloud/http-server:0.23.0" # intentionally slightly older!
link_name = "default"
"#
            .as_bytes(),
        )
        .await
        .context("failed tow write dev configuration content to file")?;
    wasmcloud_toml.flush().await?;

    // Run wash dev
    let nats_port = setup
        .nats
        .as_ref()
        .context("missing nats setup for test")?
        .1;
    let test_setup = setup
        .test_setup
        .as_ref()
        .context("missing test setup after template init")?;
    let dev_cmd = Arc::new(RwLock::new(
        test_setup
            .base_command()
            .args([
                "dev",
                "--nats-port",
                nats_port.to_string().as_ref(),
                "--nats-connect-only",
                "--ctl-port",
                nats_port.to_string().as_ref(),
                "--rpc-port",
                nats_port.to_string().as_ref(),
                "--manifest-output-dir",
                &format!("{}", generated_manifests_dir.display()),
            ])
            .kill_on_drop(true)
            .spawn()
            .context("failed running cargo dev")?,
    ));
    let watch_dev_cmd = dev_cmd.clone();

    // Get the host that was created
    let ctl_client = setup.ctl_client.clone();
    let host = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(h) = ctl_client
                .get_hosts()
                .await
                .map_err(|e| anyhow!("failed to get hosts: {e}"))
                .context("getting hosts failed")?
                .into_iter()
                .map(|v| v.into_data())
                .next()
            {
                return Ok::<Option<Host>, anyhow::Error>(h);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .context("timed out waiting for host to start up")?
    .context("failed to get the host")?;
    let host_id = host
        .as_ref()
        .context("host was missing from request")?
        .id()
        .to_string();

    // Wait until the http-hello-world component is present on the host
    let ctl_client = setup.ctl_client.clone();
    let _component = wait_for_component(watch_dev_cmd, ctl_client, &host_id, "http-hello-world")
        .await
        .context("missing http-hello-world component")?;

    // Find the generated manifest (there should only be one)
    let generated_manifest = find_yaml_file(generated_manifests_dir)
        .await
        .context("failed to find generated manifest")?;

    // Find the HTTP provider component w/ the overridden image ref
    let _provider_component = generated_manifest
        .components()
        .find(|c| {
            matches!(
                c.properties,
                Properties::Capability { ref properties } if properties.image == Some("ghcr.io/wasmcloud/http-server:0.23.0".into()))
        })
        .context("missing http provider component in manifest w/ updated image_ref")?;

    teardown_test(dev_cmd, setup).await?;

    Ok(())
}

/// Ensure that secrets can be used
///
/// This test checks three methods of specifying secrets:
///
/// - custom manifest (pre-existing)
/// - interface overriden (pre-existing, dynamic generated +/- env values)
///
#[tokio::test]
#[serial_test::serial]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_dev_create_env_secrets_serial() -> Result<()> {
    // // Ensure the secret_nats_kv binary is present
    // let secrets_nats_kv_bin =
    //     wash_lib::start::secrets_nats_kv::ensure_binary(NATS_KV_SECRETS_VERSION, downloads_dir()?)
    //         .await
    //         .context("failed to download/ensure secret nats kv binary")?;

    let secrets_nats_kv_bin =
        "/home/mrman/code/work/cosmonic/forks/wasmCloud/target/debug/secrets-nats-kv".into();

    // Create transit & encryption XKeys for use by secrets machinery
    let (transit_xkey, encryption_xkey) = (XKey::new(), XKey::new());

    let setup = wash_dev_test_setup(WashDevTestSetupArgs {
        test_name: "wash_dev_integration_dev_create_env_secrets_serial".into(),
        template_init: Some(("hello".into(), "hello-world-rust".into())),
        start_nats_kv_args: Some(StartNatsKvSecretsArgs {
            secrets_nats_kv_bin: &secrets_nats_kv_bin,
            nats_address: None, // wash dev test setup will feed in NATS address
            transit_xkey: &transit_xkey,
            encryption_xkey: &encryption_xkey,
        }),
    })
    .await?;

    // Write out the fixture configuration to disk
    let fixture_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("./tests/fixtures/wadm/test-secrets.yaml");
    tokio::fs::write(
        setup.project_dir.join("test.wadm.yaml"),
        tokio::fs::read(&fixture_path)
            .await
            .with_context(|| format!("failed to read fixture @ [{}]", fixture_path.display()))?,
    )
    .await
    .context("failed to write out fixture file")?;

    // Manipulate the wasmcloud.toml for the test project and override the manifest
    let wasmcloud_toml_path = setup.project_dir.join("wasmcloud.toml");
    let mut wasmcloud_toml = tokio::fs::File::options()
        .append(true)
        .open(&wasmcloud_toml_path)
        .await
        .with_context(|| {
            format!(
                "failed to open wasmcloud toml file @ [{}]",
                wasmcloud_toml_path.display()
            )
        })?;
    wasmcloud_toml
        .write_all(
            r#"
[dev]
manifests = [
  { component_name = "http-handler", path = "test.wadm.yaml" }
]

[[dev.overrides.imports]]
interface =  "wasmcloud:secrets"
secrets = [
  { name = "overidden-by-interface", source = { policy = "nats-kv", key = "interface-override-password" } },
  { name = "overidden-by-env", values = { "user" = "ENV:ENV_PROVIDED_SECRET" } },
]
"#
            .as_bytes(),
        )
        .await
        .context("failed tow write dev configuration content to file")?;
    wasmcloud_toml.flush().await?;

    let bad_password = "badpassword";
    let better_password = "BetterbutStillBad";
    let good_password = "5sCdAsWUym2VEUqYsf9nCi6TnkrDCDxs!";

    // Setup test environment (NATS, etc)
    let nats_port = setup
        .nats
        .as_ref()
        .context("missing nats setup for test")?
        .1;
    let nats_address = format!("nats://127.0.0.1:{nats_port}");
    let test_setup = setup
        .test_setup
        .as_ref()
        .context("missing test setup after template init")?;
    let ctl_client = setup.ctl_client.clone();
    let nc = ctl_client.nats_client();

    // Create the YAML secret that won't be overridden
    create_secret(CreateSecretArgs {
        secrets_nats_kv_bin: &secrets_nats_kv_bin,
        nats_address: &nats_address,
        transit_xkey: &transit_xkey,
        key: "test-original".into(),
        version: "0.0.1".into(),
        value: Either::Left(bad_password.into()),
    })
    .await
    .context("failed to create yaml-referenced secret")?;
    allow_secret_access(
        &secrets_nats_kv_bin,
        &nats_address,
        HTTP_PASSWORD_CHECKER_COMPONENT_PUBLIC_KEY,
        "test-original",
    )
    .await?;

    // Create the YAML secret that will be overriden by interface
    create_secret(CreateSecretArgs {
        secrets_nats_kv_bin: &secrets_nats_kv_bin,
        nats_address: &nats_address,
        transit_xkey: &transit_xkey,
        key: "test-interface".into(),
        version: "0.0.1".into(),
        value: Either::Left(bad_password.into()),
    })
    .await
    .context("failed to create yaml-referenced secret")?;
    allow_secret_access(
        &secrets_nats_kv_bin,
        &nats_address,
        HTTP_PASSWORD_CHECKER_COMPONENT_PUBLIC_KEY,
        "test-interface",
    )
    .await?;

    // Create the YAML secret that will be overriden by interface
    create_secret(CreateSecretArgs {
        secrets_nats_kv_bin: &secrets_nats_kv_bin,
        nats_address: &nats_address,
        transit_xkey: &transit_xkey,
        key: "test-interface".into(),
        version: "0.0.1".into(),
        value: Either::Left(better_password.into()),
    })
    .await
    .context("failed to create yaml-referenced secret")?;
    allow_secret_access(
        &secrets_nats_kv_bin,
        &nats_address,
        HTTP_PASSWORD_CHECKER_COMPONENT_PUBLIC_KEY,
        "test-interface",
    )
    .await?;

    let dev_cmd = Arc::new(RwLock::new(
        test_setup
            .base_command()
            .args([
                "dev",
                "--nats-connect-only",
                "--secrets-nats-kv-connect-only",
                "--nats-port",
                nats_port.to_string().as_ref(),
                "--ctl-port",
                nats_port.to_string().as_ref(),
                "--rpc-port",
                nats_port.to_string().as_ref(),
            ])
            .env(
                "SECRETS_NATS_KV_TRANSIT_XKEY_SEED",
                transit_xkey
                    .seed()
                    .context("failed to get transit key seed")?,
            )
            .env(
                "SECRETS_NATS_KV_ENCRYPTION_XKEY_SEED",
                encryption_xkey
                    .seed()
                    .context("failed to get encryption key seed")?,
            )
            .env("SECRETS_NATS_KV_NATS_ADDRESS", &nats_address)
            .env("ENV_PROVIDED_SECRET", good_password)
            .kill_on_drop(true)
            .spawn()
            .context("failed running cargo dev")?,
    ));

    // Get the host that was created
    let host = get_first_available_host(ctl_client.clone()).await?;
    let host_id = host.id().to_string();
    let http_client = reqwest::Client::new();

    // Wait until the check endpoint is up and can handle trivial workloads
    tokio::time::timeout(tokio::time::Duration::from_secs(30), async {
        loop {
            if http_client
                .post("http://localhost:8080/api/v1/check")
                .body(serde_json::to_vec(&json!({ "value": "test" })).unwrap())
                .send()
                .await
                .is_ok_and(|r| r.status().is_success())
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .context("http server was never available")?;

    // Retrieve the original secret value
    assert_eq!(
        http_client
            .post("http://localhost:8080/api/v1/check")
            .body(serde_json::to_vec(&json!({ "secret": { "key": "yaml" } }))?)
            .send()
            .await?
            .json::<PasswordCheckResponse>()
            .await
            .context("failed to convert response to JSON")?,
        PasswordCheckResponse {
            strength: PasswordStrength::Weak,
            length: bad_password.len(),
            contains: vec!["lowercase".into(),],
        },
        "default password in yaml is as expected"
    );

    assert_eq!(
        http_client
            .post("http://localhost:8080/api/v1/check")
            .body(serde_json::to_vec(
                &json!({ "secret": { "key": "overidden-by-interface" } })
            )?)
            .send()
            .await?
            .json::<PasswordCheckResponse>()
            .await
            .context("failed to convert response to JSON")?,
        PasswordCheckResponse {
            strength: PasswordStrength::Weak,
            length: better_password.len(),
            contains: vec!["lowercase".into(), "uppercase".into()],
        },
        "interface-overriden password was changed properly",
    );

    // NOTE: the env secret should actually be created *on the fly* by `wash dev`
    assert_eq!(
        http_client
            .post("http://localhost:8080/api/v1/check")
            .body(serde_json::to_vec(
                &json!({ "secret": { "key": "overidden-by-env" } })
            )?)
            .send()
            .await?
            .json::<PasswordCheckResponse>()
            .await
            .context("failed to convert response to JSON")?,
        PasswordCheckResponse {
            strength: PasswordStrength::Strong,
            length: good_password.len(),
            contains: vec![
                "lowercase".into(),
                "symbol".into(),
                "number".into(),
                "uppercase".into(),
            ],
        },
        "env-overriden password was changed properly"
    );

    teardown_test(dev_cmd, setup).await?;

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PasswordStrength {
    VeryWeak,
    Weak,
    Medium,
    Strong,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordCheckResponse {
    /// Size of the input
    strength: PasswordStrength,
    /// Size of the input
    length: usize,
    /// Size of the input
    contains: Vec<String>,
}

/// Test setup specific to wash dev
#[derive(Default)]
struct WashDevTestSetupArgs<'a> {
    /// Name of the test (for on-disk output)
    test_name: String,

    /// Initialize a template with a given name, and the given template name
    /// (e.g. `Some(("hello", "hello-world-rust"))`)
    template_init: Option<(String, String)>,

    /// Whether to start a secrets-nats-kv instance
    start_nats_kv_args: Option<StartNatsKvSecretsArgs<'a>>,
}

struct WashDevTestSetup {
    /// Project directory
    project_dir: PathBuf,

    /// NATS process & port to use for the test (if one was required
    nats: Option<(tokio::process::Child, u16)>,

    /// secrets-nats-kv instance
    secrets_nats_kv: Option<tokio::process::Child>,

    /// Client to use to interact with the lattice
    ctl_client: wasmcloud_control_interface::Client,

    /// Reusable [`common::TestSetup`], if a template initialization was specified
    test_setup: Option<TestSetup>,
}

/// Basic test setup for `wash dev` tests
async fn wash_dev_test_setup(
    WashDevTestSetupArgs {
        test_name,
        template_init,
        start_nats_kv_args,
    }: WashDevTestSetupArgs<'_>,
) -> Result<WashDevTestSetup> {
    wait_for_no_hosts()
        .await
        .context("unexpected wasmcloud instance(s) running")?;

    let dir = test_dir_with_subfolder(&test_name);

    // Set up a test repository if one was provided
    let (test_setup, project_dir) = match template_init {
        Some((name, template)) => {
            let test_setup = init(&name, &template)
                .await
                .with_context(|| format!("failed to setup project from template [{template}]"))?;
            let project_dir = test_setup.project_dir.clone();
            (Some(test_setup), project_dir)
        }
        None => (None, dir.clone()),
    };

    wait_for_no_hosts()
        .await
        .context("one or more unexpected wasmcloud instances running")?;

    // Start NATS
    let nats_port = find_open_port().await?;
    let nats = start_nats(nats_port, &dir).await?;

    // Create a ctl client to check the cluster
    let ctl_client = CtlClientBuilder::new(
        async_nats::connect(format!("127.0.0.1:{nats_port}"))
            .await
            .context("failed to create nats client")?,
    )
    .lattice("default")
    .build();

    // Start secrets-nats-kv if necessary
    let mut secrets_nats_kv = None;
    if let Some(mut args) = start_nats_kv_args {
        args.nats_address = Some(format!("nats://127.0.0.1:{nats_port}"));
        secrets_nats_kv = Some(start_nats_kv(args).await?);
    }

    Ok(WashDevTestSetup {
        nats: Some((nats, nats_port)),
        ctl_client,
        project_dir,
        test_setup,
        secrets_nats_kv,
    })
}

/// Teardown just the wash dev process
async fn teardown_wash_dev(dev_cmd: Arc<RwLock<tokio::process::Child>>) -> Result<()> {
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
    Ok(())
}

/// Teardown all test related environment for a `wash dev` test
async fn teardown_test(
    dev_cmd: Arc<RwLock<tokio::process::Child>>,
    WashDevTestSetup { nats, .. }: WashDevTestSetup,
) -> Result<()> {
    teardown_wash_dev(dev_cmd).await?;

    // Kill the nats instance
    if let Some((mut nats, _)) = nats {
        nats.kill().await.map_err(|e| anyhow!(e))?;
    }

    wait_for_no_nats()
        .await
        .context("nats instance failed to exit cleanly (processes still left over)")?;

    Ok(())
}

/// Helper function that makes it easy to wait for a single component
async fn wait_for_component(
    watch_dev_cmd: Arc<RwLock<Child>>,
    ctl_client: CtlClient,
    host_id: &str,
    component_name: &str,
) -> Result<ComponentDescription> {
    let host_id = host_id.to_string();
    let component_name = component_name.to_string();
    tokio::time::timeout(
        Duration::from_secs(60),
        tokio::spawn(async move {
            loop {
                // If the command failed (and exited early), bail
                if let Ok(Some(exit_status)) = watch_dev_cmd.write().await.try_wait() {
                    if !exit_status.success() {
                        bail!("dev command failed");
                    }
                }
                // If the file got built, we know dev succeeded
                let host_inventory = ctl_client
                    .get_host_inventory(&host_id)
                    .await
                    .map_err(|e| anyhow!(e))
                    .map(|v| v.into_data())
                    .context("failed to get host inventory");

                if let Ok(Some(inv)) = host_inventory {
                    if let Some(c) = inv
                        .components()
                        .iter()
                        .find(|c| c.name() == Some(&component_name))
                    {
                        break Ok(c.clone());
                    }
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }),
    )
    .await
    .context("timed out while waiting for component to be present in host inventory")?
    .context("failed to find component")?
}

/// Helper to find the first available manifest file in a given directory
async fn find_yaml_file(dir: impl AsRef<Path>) -> Result<Manifest> {
    let mut dir_entries = tokio::fs::read_dir(dir.as_ref()).await?;
    loop {
        let entry = dir_entries
            .next_entry()
            .await
            .context("failed to get dir entry")?
            .context("no more dir entries")?;
        if entry.path().extension().is_some_and(|v| v == "yaml") {
            return serde_yaml::from_slice::<Manifest>(&tokio::fs::read(entry.path()).await?)
                .context("failed to parse manifest YAML");
        }
    }
}

/// Helper to get the first available host
async fn get_first_available_host(ctl_client: CtlClient) -> Result<Host> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(Some(h)) = ctl_client
                .get_hosts()
                .await
                .map_err(|e| anyhow!("failed to get hosts: {e}"))
                .context("getting hosts failed")?
                .into_iter()
                .map(|v| v.into_data())
                .next()
            {
                return Ok::<Host, anyhow::Error>(h);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .context("timed out waiting for host to start up")?
    .context("failed to get the host")
}

/// Create a wasmcloud secret that tells wasmcloud how to get the existing secret
async fn put_wasmcloud_secret(
    ctl_client: &CtlClient,
    secret_backend: String,
    secret_name: String,
    key: String,
    field: Option<String>,
    version: Option<String>,
    policy_property_map: impl Into<HashMap<String, serde_json::Value>>,
) -> Result<()> {
    let config_name = format!("SECRET_{secret_name}");
    let secret_config = SecretConfig::new(
        secret_name,
        secret_backend,
        key,
        field,
        version,
        policy_property_map.into(),
    );
    let secret_values: HashMap<String, String> = secret_config.try_into()?;
    assert!(ctl_client
        .put_config(&config_name, secret_values)
        .await
        .map_err(|e| anyhow!(e).context("failed to put secret config"))?
        .succeeded());
    Ok(())
}

/// Arguments required for saving a secret in a running secrets-nats-kv
/// store, using the binary
struct CreateSecretArgs<'a> {
    secrets_nats_kv_bin: &'a PathBuf,
    nats_address: &'a str,
    transit_xkey: &'a nkeys::XKey,
    key: String,
    version: String,
    value: Either<String, Vec<u8>>,
}

/// Helper struct for starting a secrets-nats-kv instance
struct StartNatsKvSecretsArgs<'a> {
    secrets_nats_kv_bin: &'a PathBuf,
    nats_address: Option<String>,
    transit_xkey: &'a nkeys::XKey,
    encryption_xkey: &'a nkeys::XKey,
}

/// Start the secrets-nats-kv binary
async fn start_nats_kv(
    StartNatsKvSecretsArgs {
        secrets_nats_kv_bin,
        nats_address,
        transit_xkey,
        encryption_xkey,
    }: StartNatsKvSecretsArgs<'_>,
) -> Result<Child> {
    let nats_address = nats_address.context("missing NATS address")?;
    let mut cmd = tokio::process::Command::new(secrets_nats_kv_bin);
    cmd.arg("run")
        .args(["--subject-base", "wasmcloud.secrets"]) // known to be used by wash dev
        .args(["--nats-address", &nats_address])
        .env(
            "TRANSIT_XKEY_SEED",
            transit_xkey
                .seed()
                .context("failed to get transit key seed")?,
        )
        .env(
            "ENCRYPTION_XKEY_SEED",
            encryption_xkey
                .seed()
                .context("failed to get encryption key seed")?,
        )
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| anyhow!(e))
}

/// Create a secret in both the backing NATS KV secret store and in CONFIGDATA it can be retrieved
async fn create_secret(
    CreateSecretArgs {
        secrets_nats_kv_bin,
        nats_address,
        transit_xkey,
        version,
        key,
        value,
    }: CreateSecretArgs<'_>,
) -> Result<Option<NamedTempFile>> {
    let mut generated_byte_file = None;

    // TODO: re-add the put_secret code instead of shelling out
    //
    // secrets_nats_kv::client::put_secret(
    //     &self.nats_client,
    //     &self.subject_base,
    //     &self.transit_xkey,
    //     PutSecretRequest { key, version, match value { ... }},
    // )
    // .await?;

    let mut cmd = tokio::process::Command::new(secrets_nats_kv_bin);
    cmd.arg("put")
        .arg(&key)
        .args(["--subject-base", "wasmcloud.secrets"]) // known to be used by wash dev
        .args(["--secret-version", &version])
        .args(["--nats-address", nats_address])
        .env(
            "TRANSIT_XKEY_SEED",
            transit_xkey
                .seed()
                .context("failed to get transit key seed")?,
        );
    match value {
        Either::Left(s) => {
            cmd.env("SECRET_STRING_VALUE", &s);
        }
        Either::Right(b) => {
            let secret_file =
                NamedTempFile::new().context("failed to create temporary log file")?;
            tokio::fs::write(secret_file.path(), &b)
                .await
                .context("failed to write secret bytes to file")?;
            cmd.env("SECRET_BINARY_FILE", secret_file.path());
            generated_byte_file = Some(secret_file);
        }
    }

    let output = cmd
        .output()
        .await
        .context("failed to run secret-nats-kv put")?;
    assert!(
        output.status.success(),
        "failed to run put with secret nats KV bin"
    );

    Ok(generated_byte_file)
}

/// Create a secret in both the backing NATS KV secret store and in CONFIGDATA it can be retrieved
async fn allow_secret_access(
    secrets_nats_kv_bin: &PathBuf,
    nats_address: &str,
    public_key: &str,
    secret_name: &str,
) -> Result<()> {
    let output = tokio::process::Command::new(secrets_nats_kv_bin)
        .arg("add-mapping")
        .arg(&public_key)
        .args(["--subject-base", "wasmcloud.secrets"]) // known to be used by wash dev
        .args(["--secret", secret_name])
        .args(["--nats-address", nats_address])
        .output()
        .await
        .context("failed to run secret-nats-kv put")?;
    eprintln!("OUTPUT: {output:#?}");
    assert!(
        output.status.success(),
        "failed to run put with secret nats KV bin"
    );

    Ok(())
}
