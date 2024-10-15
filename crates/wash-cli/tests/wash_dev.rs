#![cfg(target_family = "unix")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _, Result};
use nkeys::XKey;
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

#[tokio::test]
#[serial_test::serial]
async fn integration_dev_hello_component_serial() -> Result<()> {
    let setup = wash_dev_test_setup(WashDevTestSetupArgs {
        test_name: "dev_hello_component".into(),
        template_init: Some(("hello".into(), "hello-world-rust".into())),
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

    teardown_wash_dev_test(dev_cmd, setup).await?;

    wait_for_no_wadm()
        .await
        .context("wadm instance failed to exit cleanly (processes still left over)")?;

    Ok(())
}

/// Ensure that overriding manifest YAML works
#[tokio::test]
#[serial_test::serial]
async fn integration_dev_override_manifest_yaml_serial() -> Result<()> {
    let setup = wash_dev_test_setup(WashDevTestSetupArgs {
        test_name: "wash_dev_integration_override_manifest_yaml".into(),
        template_init: Some(("hello".into(), "hello-world-rust".into())),
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

    teardown_wash_dev_test(dev_cmd, setup).await?;

    wait_for_no_wadm()
        .await
        .context("wadm instance failed to exit cleanly (processes still left over)")?;

    Ok(())
}

/// Ensure that overriding by interface via project config YAML works
#[tokio::test]
#[serial_test::serial]
async fn integration_dev_override_via_interface_serial() -> Result<()> {
    let setup = wash_dev_test_setup(WashDevTestSetupArgs {
        test_name: "wash_dev_integration_override_via_interface".into(),
        template_init: Some(("hello".into(), "hello-world-rust".into())),
    })
    .await?;

    // Create a dir for generated manifests
    let generated_manifests_dir = setup.project_dir.join("generated-manifests");
    tokio::fs::create_dir(&generated_manifests_dir).await?;

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
    let generated_manifest = first_yaml_file(generated_manifests_dir)
        .await
        .context("failed to find generated manifest")?;

    // Find the HTTP provider component w/ the overridden image ref
    let _provider_component = generated_manifest
        .components()
        .find(|c| {
            matches!(
                c.properties,
                Properties::Capability { ref properties } if properties.image == "ghcr.io/wasmcloud/http-server:0.23.0")
        })
        .context("missing http provider component in manifest w/ updated image_ref")?;

    teardown_wash_dev_test(dev_cmd, setup).await?;

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
async fn integration_dev_create_env_secrets_serial() -> Result<()> {
    let setup = wash_dev_test_setup(WashDevTestSetupArgs {
        test_name: "wash_dev_integration_override_via_interface".into(),
        template_init: Some(("hello".into(), "hello-world-rust".into())),
    })
    .await?;

    // Write out the fixture configuration to disk
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("./tests/fixtures/wadm/secret-strength-checker-rust-dev-override.yaml");
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
[[dev.overrides.imports]]
interface =  "wasmcloud:secrets"
secrets = [
  { name = "iface-override-existing", source = { policy = "nats-kv", key = "test" },
  { name = "iface-override-dynamic", values = { "user" = "ENV:ENV_PROVIDED_SECRET" },
]
"#
            .as_bytes(),
        )
        .await
        .context("failed tow write dev configuration content to file")?;
    wasmcloud_toml.flush().await?;

    // Create transit & encryption XKeys for use by secrets machinery
    let (transit_xkey, encryption_xkey) = (XKey::new(), XKey::new());

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
            .env("ENV_PROVIDED_SECRET", "notrandom")
            .kill_on_drop(true)
            .spawn()
            .context("failed running cargo dev")?,
    ));

    // Get the host that was created
    let ctl_client = setup.ctl_client.clone();
    let host = get_first_available_host(ctl_client).await?;
    let host_id = host.id().to_string();

    // TODO: Create the pre-existing key in the custom manifest
    // TODO: Create the pre-existing key in the interface override

    // TODO: figure out how to query for the secrets as a component would access them?
    // build custom component?

    // TODO: Retrieve obscured secret for pre-existing in custom manifest
    // TODO: Retrieve obscured secret for pre-existing in iface override
    // TODO: Retrieve obscured secret for dynamic ENV-genreated in iface override

    teardown_wash_dev_test(dev_cmd, setup).await?;

    Ok(())
}

/// Test setup specific to wash dev
#[derive(Default)]
struct WashDevTestSetupArgs {
    /// Name of the test (for on-disk output)
    test_name: String,

    /// Initialize a template with a given name, and the given template name
    /// (e.g. `Some(("hello", "hello-world-rust"))`)
    template_init: Option<(String, String)>,
}

struct WashDevTestSetup {
    /// Project directory
    project_dir: PathBuf,

    /// NATS process & port to use for the test (if one was required
    nats: Option<(tokio::process::Child, u16)>,

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
    }: WashDevTestSetupArgs,
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

    Ok(WashDevTestSetup {
        nats: Some((nats, nats_port)),
        ctl_client,
        project_dir,
        test_setup,
    })
}

/// Macro that makes it easy to tear down a test that utilizes a run of `wash dev`
async fn teardown_wash_dev_test(
    dev_cmd: Arc<RwLock<tokio::process::Child>>,
    WashDevTestSetup { nats, .. }: WashDevTestSetup,
) -> Result<()> {
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
