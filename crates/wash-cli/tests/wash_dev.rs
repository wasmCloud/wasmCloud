#![cfg(target_family = "unix")]

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _, Result};
use nkeys::KeyPair;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use tokio::time::Duration;
use wadm_types::{Manifest, Properties};
use wasmcloud_control_interface::{ClientBuilder as CtlClientBuilder, Host};

mod common;
use common::{
    find_open_port, init, start_nats, test_dir_with_subfolder, wait_for_no_hosts, wait_for_no_nats,
    wait_for_no_wadm, wait_for_num_hosts,
};

#[tokio::test]
#[serial_test::serial]
async fn integration_dev_hello_component_serial() -> Result<()> {
    wait_for_no_hosts()
        .await
        .context("unexpected wasmcloud instance(s) running")?;
    let test_setup = init(
        /* component_name= */ "hello",
        /* template_name= */ "hello-world-rust",
    )
    .await?;
    let project_dir = test_setup.project_dir.clone();

    let dir = test_dir_with_subfolder("dev_hello_component");

    wait_for_no_hosts()
        .await
        .context("one or more unexpected wasmcloud instances running")?;

    let nats_port = find_open_port().await?;
    let mut nats = start_nats(nats_port, &dir).await?;

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
    assert!(signed_file_path.exists(), "signed component file was built");

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

    wait_for_no_wadm()
        .await
        .context("wadm instance failed to exit cleanly (processes still left over)")?;

    Ok(())
}

/// Ensure that overriding manifest YAML works
#[tokio::test]
#[serial_test::serial]
async fn integration_override_manifest_yaml_serial() -> Result<()> {
    wait_for_no_hosts()
        .await
        .context("unexpected wasmcloud instance(s) running")?;

    let test_setup = init("hello", "hello-world-rust").await?;
    let project_dir = test_setup.project_dir.clone();
    let dir = test_dir_with_subfolder("dev_hello_component");

    wait_for_no_hosts()
        .await
        .context("one or more unexpected wasmcloud instances running")?;

    // Start NATS
    let nats_port = find_open_port().await?;
    let mut nats = start_nats(nats_port, &dir).await?;

    // Create a ctl client to check the cluster
    let ctl_client = CtlClientBuilder::new(
        async_nats::connect(format!("127.0.0.1:{nats_port}"))
            .await
            .context("failed to create nats client")?,
    )
    .lattice("default")
    .build();

    // Write out the fixture configuration to disk
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("./tests/fixtures/wadm/hello-world-rust-dev-override.yaml");
    tokio::fs::write(
        project_dir.join("test.wadm.yaml"),
        tokio::fs::read(&fixture_path)
            .await
            .with_context(|| format!("failed to read fixture @ [{}]", fixture_path.display()))?,
    )
    .await
    .context("failed to write out fixture file")?;

    // Manipulate the wasmcloud.toml for the test project and override the manifest
    let wasmcloud_toml_path = project_dir.join("wasmcloud.toml");
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
            if let Some(h) = ctl_client
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

    wait_for_no_wadm()
        .await
        .context("wadm instance failed to exit cleanly (processes still left over)")?;

    Ok(())
}

/// Ensure that overriding by interface via project config YAML works
#[tokio::test]
#[serial_test::serial]
async fn integration_override_via_interface_serial() -> Result<()> {
    wait_for_no_hosts()
        .await
        .context("unexpected wasmcloud instance(s) running")?;

    let test_setup = init("hello", "hello-world-rust").await?;
    let project_dir = test_setup.project_dir.clone();
    let dir = test_dir_with_subfolder("dev_hello_component");

    // Create a dir for generated manifests
    let generated_manifests_dir = project_dir.join("generated-manifests");
    tokio::fs::create_dir(&generated_manifests_dir).await?;

    wait_for_no_hosts()
        .await
        .context("one or more unexpected wasmcloud instances running")?;

    // Start NATS
    let nats_port = find_open_port().await?;
    let mut nats = start_nats(nats_port, &dir).await?;

    // Create a ctl client to check the cluster
    let ctl_client = CtlClientBuilder::new(
        async_nats::connect(format!("127.0.0.1:{nats_port}"))
            .await
            .context("failed to create nats client")?,
    )
    .lattice("default")
    .build();

    // Write out the fixture configuration to disk
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("./tests/fixtures/wadm/hello-world-rust-dev-override.yaml");
    tokio::fs::write(
        project_dir.join("test.wadm.yaml"),
        tokio::fs::read(&fixture_path)
            .await
            .with_context(|| format!("failed to read fixture @ [{}]", fixture_path.display()))?,
    )
    .await
    .context("failed to write out fixture file")?;

    // Manipulate the wasmcloud.toml for the test project and override the manifest
    let wasmcloud_toml_path = project_dir.join("wasmcloud.toml");
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
        .context("failed to write dev configuration content to file")?;
    wasmcloud_toml.flush().await?;

    // Run wash dev
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
                            .any(|c| c.name() == Some("http-hello-world"))
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

    // Find the generated manifest (there should only be one
    let generated_manifest = {
        let mut dir_entries = tokio::fs::read_dir(generated_manifests_dir).await?;
        loop {
            let entry = dir_entries
                .next_entry()
                .await
                .context("failed to get dir entry")?
                .context("no more dir entries")?;
            if entry.path().extension().is_some_and(|v| v == "yaml") {
                break serde_yaml::from_slice::<Manifest>(&tokio::fs::read(entry.path()).await?)
                    .context("failed to parse manifest YAML")?;
            }
        }
    };

    // Find the HTTP provider component w/ the overridden image ref
    let _provider_component = generated_manifest
        .components()
        .find(|c| {
            matches!(
                c.properties,
                Properties::Capability { ref properties } if properties.image.as_ref().is_some_and(|i| i == "ghcr.io/wasmcloud/http-server:0.23.0"))
        })
        .context("missing http provider component in manifest w/ updated image_ref")?;

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

    wait_for_no_wadm()
        .await
        .context("wadm instance failed to exit cleanly (processes still left over)")?;

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
/// This test ensures that dev works when there is already a running host by
/// connecting to it and then starting a dev loop.
async fn integration_dev_running_host_tests() -> Result<()> {
    wait_for_no_hosts()
        .await
        .context("unexpected wasmcloud instance(s) running")?;
    let test_setup = init(
        /* component_name= */ "hello",
        /* template_name= */ "hello-world-rust",
    )
    .await?;
    let project_dir = test_setup.project_dir.clone();

    let dir = test_dir_with_subfolder("dev_hello_component");

    wait_for_no_hosts()
        .await
        .context("one or more unexpected wasmcloud instances running")?;

    let nats_port = find_open_port().await?;
    let mut nats = start_nats(nats_port, &dir).await?;

    // Start a wasmCloud host
    let up_cmd = Arc::new(RwLock::new(
        test_setup
            .base_command()
            .args([
                "up",
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

    // Start a dev loop, which should just work and use the existing host
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
    assert!(signed_file_path.exists(), "signed component file was built");

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

    // Kill the originally launched host
    let process_pid = up_cmd
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

    wait_for_no_hosts()
        .await
        .context("wasmcloud instance failed to exit cleanly (processes still left over)")?;

    // Kill the nats instance
    nats.kill().await.map_err(|e| anyhow!(e))?;

    wait_for_no_nats()
        .await
        .context("nats instance failed to exit cleanly (processes still left over)")?;

    wait_for_no_wadm()
        .await
        .context("wadm instance failed to exit cleanly (processes still left over)")?;

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
/// This test ensures that dev does not start and exits cleanly when multiple hosts are
/// available and the host ID is not specified. Then, ensures dev does start when
/// the host ID is specified.
async fn integration_dev_running_multiple_hosts_tests() -> Result<()> {
    wait_for_no_hosts()
        .await
        .context("unexpected wasmcloud instance(s) running")?;
    let test_setup = init(
        /* component_name= */ "hello",
        /* template_name= */ "hello-world-rust",
    )
    .await?;
    let project_dir = test_setup.project_dir.clone();

    let dir = test_dir_with_subfolder("dev_hello_component");

    wait_for_no_hosts()
        .await
        .context("one or more unexpected wasmcloud instances running")?;

    let nats_port = find_open_port().await?;
    let mut nats = start_nats(nats_port, &dir).await?;

    // Start a wasmCloud host
    let host_id = KeyPair::new_server();
    let up_cmd = Arc::new(RwLock::new(
        test_setup
            .base_command()
            .args([
                "up",
                "--nats-connect-only",
                "--nats-port",
                nats_port.to_string().as_ref(),
                "--ctl-port",
                nats_port.to_string().as_ref(),
                "--rpc-port",
                nats_port.to_string().as_ref(),
                "--host-seed",
                host_id.seed().context("failed to get host seed")?.as_str(),
            ])
            .kill_on_drop(true)
            .spawn()
            .context("failed running cargo dev")?,
    ));
    // Start a second wasmCloud host
    let up_cmd2 = Arc::new(RwLock::new(
        test_setup
            .base_command()
            .args([
                "up",
                "--nats-connect-only",
                "--nats-port",
                nats_port.to_string().as_ref(),
                "--ctl-port",
                nats_port.to_string().as_ref(),
                "--rpc-port",
                nats_port.to_string().as_ref(),
                "--multi-local",
            ])
            .kill_on_drop(true)
            .spawn()
            .context("failed running cargo dev")?,
    ));

    // Ensure two hosts are running
    wait_for_num_hosts(2)
        .await
        .context("did not get 2 hosts running")?;

    // Start a dev loop, which will not work with more than one host
    let bad_dev_cmd_multiple_hosts =
        // We're going to wait for the output, which should happen right after
        // querying for the host list.
        tokio::time::timeout(
            Duration::from_secs(10),
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
                .output(),
        )
        .await
        .context("dev loop did not exit in expected time")?
        .context("dev loop failed to exit cleanly")?;

    assert!(!bad_dev_cmd_multiple_hosts.status.success());
    assert!(bad_dev_cmd_multiple_hosts.stdout.is_empty());
    assert!(String::from_utf8_lossy(&bad_dev_cmd_multiple_hosts.stderr)
        .contains("found multiple running hosts"));

    // Start a dev loop, which will fail to find the desired host
    let bad_dev_cmd_multiple_hosts =
        // We're going to wait for the output, which should happen right after
        // querying for the host list.
        tokio::time::timeout(
            Duration::from_secs(10),
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
                    "--host-id",
                    // Real host ID, just not one of the ones we started
                    "NAAX34C3KIELJQJRZBAJSRJ6S3Q5NGAGMAAFITB64F4L5L4LQC6XVAZK"
                ])
                .kill_on_drop(true)
                .output(),
        )
        .await
        .context("dev loop did not exit in expected time")?
        .context("dev loop failed to exit cleanly")?;

    assert!(!bad_dev_cmd_multiple_hosts.status.success());
    assert!(bad_dev_cmd_multiple_hosts.stdout.is_empty());
    assert!(String::from_utf8_lossy(&bad_dev_cmd_multiple_hosts.stderr)
        .contains("not found in running hosts"));

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
                "--host-id",
                host_id.public_key().as_str(),
            ])
            .kill_on_drop(true)
            .spawn()
            .context("dev loop did not start successfully with multiple hosts")?,
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
    assert!(signed_file_path.exists(), "signed component file was built");

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

    // Kill the originally launched host
    let process_pid = up_cmd
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

    // Kill the second host
    let process_pid = up_cmd2
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

    wait_for_no_hosts()
        .await
        .context("wasmcloud instance failed to exit cleanly (processes still left over)")?;

    // Kill the nats instance
    nats.kill().await.map_err(|e| anyhow!(e))?;

    wait_for_no_nats()
        .await
        .context("nats instance failed to exit cleanly (processes still left over)")?;

    wait_for_no_wadm()
        .await
        .context("wadm instance failed to exit cleanly (processes still left over)")?;

    Ok(())
}
