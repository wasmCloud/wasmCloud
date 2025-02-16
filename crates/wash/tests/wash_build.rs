mod common;

use common::{init, init_path, init_provider, init_workspace, load_fixture};

use anyhow::{Context, Result};
use tokio::{fs::File, process::Command};
use wash::lib::build::PACKAGE_LOCK_FILE_NAME;

#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_build_rust_component_unsigned() -> Result<()> {
    let test_setup = init(
        /* component_name= */ "hello-unsigned",
        /* template_name= */ "hello-world-rust",
    )
    .await?;
    let project_dir = test_setup.project_dir.clone();

    let status = test_setup
        .base_command()
        .args(["build", "--build-only"])
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("build/http_hello_world.wasm");
    assert!(
        tokio::fs::try_exists(unsigned_file).await.unwrap(),
        "unsigned file not found!"
    );
    let signed_file = project_dir.join("build/http_hello_world_s.wasm");
    assert!(
        !tokio::fs::try_exists(signed_file).await.unwrap(),
        "signed file should not exist when using --build-only!"
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_build_rust_component_signed() -> Result<()> {
    // We test a dep from git above, so this tests with a local path so we can test local changes
    let test_setup = init_path(
        /* component_name= */ "hello",
        /* template_name= */ "crates/wash/tests/fixtures/dog-fetcher",
    )
    .await?;
    let project_dir = test_setup.project_dir.clone();

    let status = test_setup
        .base_command()
        .args(["build"])
        .kill_on_drop(true)
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("build/dog_fetcher.wasm");
    assert!(
        tokio::fs::try_exists(unsigned_file).await.unwrap(),
        "unsigned file not found!"
    );
    let signed_file = project_dir.join("build/dog_fetcher_s.wasm");
    assert!(
        tokio::fs::try_exists(signed_file).await.unwrap(),
        "signed file not found!"
    );
    let lock_file = project_dir.join(PACKAGE_LOCK_FILE_NAME);
    assert!(
        tokio::fs::try_exists(lock_file).await.unwrap(),
        "lock file not found!"
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_build_uses_wkg_lock() -> Result<()> {
    let test_setup = init_path(
        /* component_name= */ "hello",
        /* template_name= */ "examples/rust/components/dog-fetcher",
    )
    .await?;
    let project_dir = test_setup.project_dir.clone();

    // Move the wasmcloud.lock to be wkg.lock
    tokio::fs::rename(
        project_dir.join("wasmcloud.lock"),
        project_dir.join("wkg.lock"),
    )
    .await
    .unwrap();

    let status = test_setup
        .base_command()
        .args(["build"])
        .kill_on_drop(true)
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("build/dog_fetcher.wasm");
    assert!(
        tokio::fs::try_exists(unsigned_file).await.unwrap(),
        "unsigned file not found!"
    );
    let signed_file = project_dir.join("build/dog_fetcher_s.wasm");
    assert!(
        tokio::fs::try_exists(signed_file).await.unwrap(),
        "signed file not found!"
    );
    let lock_file = project_dir.join(wasm_pkg_core::lock::LOCK_FILE_NAME);
    assert!(
        tokio::fs::try_exists(lock_file).await.unwrap(),
        "lock file not found!"
    );
    // Make sure wasmcloud.lock is not present
    assert!(
        !tokio::fs::try_exists(project_dir.join("wasmcloud.lock"))
            .await
            .unwrap(),
        "wasmcloud.lock should not exist!"
    );
    Ok(())
}

#[ignore]
#[tokio::test]
// #[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
// TODO: This test should be re-enabled after the transitional period for dependencies manipulation
// (wit-deps -> wkg)
async fn integration_build_rust_component_with_existing_deps_signed() -> Result<()> {
    let test_setup = init_path(
        /* component_name= */ "hello",
        /* template_name= */
        "crates/wash/tests/fixtures/old-examples/http-hello-world-rust",
    )
    .await?;
    let project_dir = test_setup.project_dir.clone();

    let status = test_setup
        .base_command()
        .args(["build"])
        .kill_on_drop(true)
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("build/http_hello_world.wasm");
    assert!(
        tokio::fs::try_exists(unsigned_file).await.unwrap(),
        "unsigned file not found!"
    );
    let signed_file = project_dir.join("build/http_hello_world_s.wasm");
    assert!(
        tokio::fs::try_exists(signed_file).await.unwrap(),
        "signed file not found!"
    );
    let lock_file = project_dir.join(PACKAGE_LOCK_FILE_NAME);
    assert!(
        tokio::fs::try_exists(lock_file).await.unwrap(),
        "lock file not found!"
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_build_with_wasmcloud_toml_overrides() -> Result<()> {
    // We test a dep from git above, so this tests with a local path so we can test local changes
    let test_setup = load_fixture("integrated-wkg").await?;
    let project_dir = test_setup.project_dir.clone();

    let status = test_setup
        .base_command()
        .args(["build"])
        .kill_on_drop(true)
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("build/ponger_config_component.wasm");
    assert!(
        tokio::fs::try_exists(unsigned_file).await.unwrap(),
        "unsigned file not found!"
    );
    let signed_file = project_dir.join("build/ponger_config_component_s.wasm");
    assert!(
        tokio::fs::try_exists(signed_file).await.unwrap(),
        "signed file not found!"
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_build_with_wkg_toml_overrides() -> Result<()> {
    // We test a dep from git above, so this tests with a local path so we can test local changes
    let test_setup = load_fixture("separate-wkg").await?;
    let project_dir = test_setup.project_dir.clone();

    let status = test_setup
        .base_command()
        .args(["build"])
        .kill_on_drop(true)
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("build/ponger_config_component.wasm");
    assert!(
        tokio::fs::try_exists(unsigned_file).await.unwrap(),
        "unsigned file not found!"
    );
    let signed_file = project_dir.join("build/ponger_config_component_s.wasm");
    assert!(
        tokio::fs::try_exists(signed_file).await.unwrap(),
        "signed file not found!"
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_build_with_logging_interface() -> Result<()> {
    // We test a dep from git above, so this tests with a local path so we can test local changes
    let test_setup = load_fixture("unversioned-logging").await?;
    let project_dir = test_setup.project_dir.clone();

    let status = test_setup
        .base_command()
        .args(["build"])
        .kill_on_drop(true)
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("build/blobby.wasm");
    assert!(
        tokio::fs::try_exists(unsigned_file).await.unwrap(),
        "unsigned file not found!"
    );
    let signed_file = project_dir.join("build/blobby_s.wasm");
    assert!(
        tokio::fs::try_exists(signed_file).await.unwrap(),
        "signed file not found!"
    );
    let lock_file = project_dir.join(PACKAGE_LOCK_FILE_NAME);
    assert!(
        tokio::fs::try_exists(lock_file).await.unwrap(),
        "lock file not found!"
    );
    Ok(())
}

#[tokio::test]
async fn integration_build_rust_component_with_no_fetch() -> Result<()> {
    let test_setup = init_path(
        /* component_name= */ "hello",
        /* template_name= */
        "crates/wash/tests/fixtures/old-examples/http-hello-world-rust",
    )
    .await?;
    let project_dir = test_setup.project_dir.clone();

    let status = test_setup
        .base_command()
        .args(["build", "--skip-fetch"])
        .kill_on_drop(true)
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("build/http_hello_world.wasm");
    assert!(
        tokio::fs::try_exists(unsigned_file).await.unwrap(),
        "unsigned file not found!"
    );
    let signed_file = project_dir.join("build/http_hello_world_s.wasm");
    assert!(
        tokio::fs::try_exists(signed_file).await.unwrap(),
        "signed file not found!"
    );
    let lock_file = project_dir.join(PACKAGE_LOCK_FILE_NAME);
    assert!(
        !tokio::fs::try_exists(lock_file).await.unwrap(),
        "lock file should not have been generated!"
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_build_rust_component_signed_with_signing_keys_directory_configuration(
) -> Result<()> {
    let test_setup = init(
        /* component_name= */ "hello",
        /* template_name= */ "hello-world-rust",
    )
    .await?;
    let project_dir = test_setup.project_dir.clone();

    // base case: no keys directory configured
    let mut expected_default_key_dir = etcetera::home_dir()?;
    expected_default_key_dir.push(".wash/keys");

    let cmd = test_setup
        .base_command()
        .args(["build"])
        .stderr(std::process::Stdio::piped())
        .env("RUST_LOG", "debug")
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to build project");

    let output = cmd
        .wait_with_output()
        .await
        .context("test command failed to run and complete")?;

    assert!(output.status.success());

    // Ensure that the key was generated in the default directory
    let generated_key = expected_default_key_dir.join("http_hello_world_module.nk");
    assert!(
        std::fs::metadata(&generated_key).is_ok(),
        "Key should be present and accessible in ~/.wash/keys"
    );

    // assert ./keys directory is not created for generated keys
    assert!(std::fs::metadata(project_dir.join("keys")).is_err());

    // case: keys directory configured via cli arg --keys-directory
    let key_directory = project_dir.join("batmankeys").to_string_lossy().to_string();
    let cmd = test_setup
        .base_command()
        .args(["build", "--keys-directory", &key_directory])
        .stderr(std::process::Stdio::piped())
        .env("RUST_LOG", "debug")
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to build project");

    let output = cmd
        .wait_with_output()
        .await
        .context("test command failed to run and complete")?;

    assert!(output.status.success());
    let output =
        String::from_utf8(output.stderr).context("Failed to convert output bytes to String")?;
    assert!(output.contains(format!("{key_directory}/http_hello_world_module.nk").as_str()));

    // case: keys directory configured via cli arg --keys-directory and --disable-keygen=true
    let key_directory = project_dir
        .join("spidermankeys")
        .to_string_lossy()
        .to_string();
    let cmd = test_setup
        .base_command()
        .args([
            "build",
            "--keys-directory",
            &key_directory,
            "--disable-keygen",
        ])
        .env("RUST_LOG", "debug")
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to build project");

    let output = cmd
        .wait_with_output()
        .await
        .context("test command failed to run and complete")?;

    assert!(!output.status.success());
    let output =
        String::from_utf8(output.stderr).context("Failed to convert output bytes to String")?;
    assert!(output.contains("No keypair found"));
    assert!(output.contains("hello/spidermankeys"));

    // case: keys directory configured via env var WASH_KEYS
    let key_directory = project_dir.join("flashkeys").to_string_lossy().to_string();
    let cmd = test_setup
        .base_command()
        .args(["build"])
        .env("WASH_KEYS", &key_directory)
        .env("RUST_LOG", "debug")
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to build project");

    let output = cmd
        .wait_with_output()
        .await
        .context("test command failed to run and complete")?;

    assert!(output.status.success());
    let output =
        String::from_utf8(output.stderr).context("Failed to convert output bytes to String")?;
    assert!(output.contains(format!("{key_directory}/http_hello_world_module.nk").as_str()));

    // case: keys directory configured via wasmcloud.toml. The config that is written to file does affect all the remaining test cases.
    let key_directory = project_dir
        .join("haljordankeys")
        .to_string_lossy()
        .to_string();

    tokio::fs::write(
        project_dir.join("wasmcloud.toml"),
        r#"
    name = "Hello World"
    language = "rust"
    type = "component"

    [component]
    claims = ["wasmcloud:httpserver"]
    key_directory = "./haljordankeys"
    "#,
    )
    .await
    .context("failed to update wasmcloud.toml file content for test case")?;

    let cmd = test_setup
        .base_command()
        .args(["build"])
        .stderr(std::process::Stdio::piped())
        .env("RUST_LOG", "debug")
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to build project");

    let output = cmd
        .wait_with_output()
        .await
        .context("test command failed to run and complete")?;

    assert!(output.status.success());
    let output =
        String::from_utf8(output.stderr).context("Failed to convert output bytes to String")?;
    assert!(output.contains(format!("{key_directory}/http_hello_world_module.nk").as_str()));

    // case when keys directory is configured via cli arg --keys-directory and wasmcloud.toml. cli arg should take precedence
    let key_directory = project_dir
        .join("wonderwomankeys")
        .to_string_lossy()
        .to_string();

    let cmd = test_setup
        .base_command()
        .args(["build", "--keys-directory", &key_directory])
        .env("RUST_LOG", "debug")
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to build project");

    let output = cmd
        .wait_with_output()
        .await
        .context("test command failed to run and complete")?;

    assert!(output.status.success());
    let output =
        String::from_utf8(output.stderr).context("Failed to convert output bytes to String")?;
    assert!(output.contains(format!("{key_directory}/http_hello_world_module.nk").as_str()));

    // case when keys directory is configured via env var $WASH_KEYS, cli arg --keys-directory and wasmcloud.toml. cli arg should take precedence
    let env_key_directory = project_dir.join("flashkeys").to_string_lossy().to_string();

    let key_directory = project_dir
        .join("aquamankeys")
        .to_string_lossy()
        .to_string();

    let cmd = test_setup
        .base_command()
        .args(["build", "--keys-directory", &key_directory])
        .env("WASH_KEYS", &env_key_directory)
        .env("RUST_LOG", "debug")
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to build project");

    let output = cmd
        .wait_with_output()
        .await
        .context("test command failed to run and complete")?;

    assert!(output.status.success());
    let output =
        String::from_utf8(output.stderr).context("Failed to convert output bytes to String")?;
    assert!(output.contains(format!("{key_directory}/http_hello_world_module.nk").as_str()));

    // case when keys directory is configured via env var $WASH_KEYS and wasmcloud.toml. env var should take precedence
    let env_key_directory = project_dir.join("orionkeys").to_string_lossy().to_string();
    let cmd = test_setup
        .base_command()
        .args(["build"])
        .env("WASH_KEYS", &env_key_directory)
        .env("RUST_LOG", "debug")
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("Failed to build project");

    let output = cmd
        .wait_with_output()
        .await
        .context("test command failed to run and complete")?;

    assert!(output.status.success());
    let output =
        String::from_utf8(output.stderr).context("Failed to convert output bytes to String")?;
    assert!(output.contains(format!("{env_key_directory}/http_hello_world_module.nk").as_str()));

    Ok(())
}

#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_build_rust_component_in_workspace_unsigned() -> Result<()> {
    let test_setup = init_workspace(vec![/* component_names= */ "hello-1", "hello-2"]).await?;
    let project_dir = test_setup.project_dirs.first().unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["build", "--build-only"])
        .current_dir(project_dir)
        .kill_on_drop(true)
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("build/http_hello_world.wasm");
    assert!(
        tokio::fs::try_exists(unsigned_file).await.unwrap(),
        "unsigned file not found!"
    );
    let signed_file = project_dir.join("build/http_hello_world_s.wasm");
    assert!(
        !tokio::fs::try_exists(signed_file).await.unwrap(),
        "signed file should not exist when using --build-only!"
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_build_tinygo_component_unsigned() -> Result<()> {
    let test_setup = init(
        /* component_name= */ "hello-world-tinygo",
        /* template_name= */ "hello-world-tinygo",
    )
    .await?;
    let project_dir = test_setup.project_dir.clone();

    let status = test_setup
        .base_command()
        .args(["build", "--build-only"])
        .kill_on_drop(true)
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("build/hello-world-tinygo.wasm");
    assert!(
        tokio::fs::try_exists(unsigned_file).await.unwrap(),
        "unsigned file not found!"
    );
    let signed_file = project_dir.join("build/hello_world_tinygo_s.wasm");
    assert!(
        !tokio::fs::try_exists(signed_file).await.unwrap(),
        "signed file should not exist when using --build-only!"
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_build_tinygo_component_signed() -> Result<()> {
    let test_setup = init_path(
        /* component_name= */ "http-client-tinygo",
        /* template_name= */ "examples/golang/components/http-client-tinygo",
    )
    .await?;
    let project_dir = test_setup.project_dir.clone();

    let status = test_setup
        .base_command()
        .args(["build"])
        .kill_on_drop(true)
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("build/http-client-tinygo.wasm");
    assert!(
        tokio::fs::try_exists(unsigned_file).await.unwrap(),
        "unsigned file not found!"
    );
    let signed_file = project_dir.join("build/http_client_tinygo_s.wasm");
    assert!(
        tokio::fs::try_exists(signed_file).await.unwrap(),
        "signed file not found!"
    );
    let lock_file = project_dir.join(PACKAGE_LOCK_FILE_NAME);
    assert!(
        tokio::fs::try_exists(lock_file).await.unwrap(),
        "lock file not found!"
    );
    Ok(())
}

#[ignore]
#[tokio::test]
// #[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
// TODO: This test should be re-enabled after the transitional period for dependencies manipulation
// (wit-deps -> wkg)
async fn integration_build_tinygo_component_with_existing_deps_signed() -> Result<()> {
    let test_setup = init_path(
        /* component_name= */ "hello-world-tinygo",
        /* template_name= */
        "crates/wash/tests/fixtures/old-examples/http-hello-world-go",
    )
    .await?;
    let project_dir = test_setup.project_dir.clone();

    let status = test_setup
        .base_command()
        .args(["build"])
        .kill_on_drop(true)
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("build/http-hello-world.wasm");
    assert!(
        tokio::fs::try_exists(unsigned_file).await.unwrap(),
        "unsigned file not found!"
    );
    let signed_file = project_dir.join("build/http_hello_world_s.wasm");
    assert!(
        tokio::fs::try_exists(signed_file).await.unwrap(),
        "signed file not found!"
    );
    let lock_file = project_dir.join(PACKAGE_LOCK_FILE_NAME);
    assert!(
        tokio::fs::try_exists(lock_file).await.unwrap(),
        "lock file not found!"
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_build_handles_dashed_names() -> Result<()> {
    let component_name = "dashed-component";
    // This tests runs against a temp directory since cargo gets confused
    // about workspace projects if done from within wash
    let root_dir = tempfile::tempdir()?;
    let component_dir = root_dir.path().join(component_name);
    let stdout_path = root_dir
        .path()
        .join(format!("wash-test.{component_name}.stdout.log"));
    let stdout = File::create(stdout_path).await?.into_std().await;

    // Execute wash new to create an component with the given name
    let mut new_cmd = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "new",
            "component",
            "dashed-component",
            "-t",
            "hello-world-rust",
        ])
        .kill_on_drop(true)
        .current_dir(&root_dir)
        .stdout(stdout.try_clone()?)
        .spawn()?;
    assert!(new_cmd.wait().await?.success());

    // Ensure that the component dir was created as expected
    assert!(tokio::fs::try_exists(&component_dir).await?);

    let mut build_cmd = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["build"])
        .kill_on_drop(true)
        .stdout(stdout)
        .current_dir(&component_dir)
        .spawn()?;

    assert!(build_cmd.wait().await?.success());

    Ok(())
}

/// Ensure that wash build can handle absolute and relative paths changing for the
/// project directory, build directory, WIT directory, and wasmcloud.toml file.
#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_build_tinygo_component_separate_paths() -> Result<()> {
    let test_setup = init_path(
        /* component_name= */ "http-client-tinygo",
        /* template_name= */ "examples/golang/components/http-client-tinygo",
    )
    .await?;
    let project_dir = test_setup.project_dir.clone();

    // Rename the WIT directory
    tokio::fs::rename(project_dir.join("wit"), project_dir.join("wow"))
        .await
        .context("failed to rename wit directory")?;
    // Move the wasmcloud.toml to a different directory
    tokio::fs::remove_file(project_dir.join("wasmcloud.toml"))
        .await
        .context("failed to remove wasmcloud.toml")?;
    tokio::fs::create_dir(project_dir.join("config"))
        .await
        .context("failed to create config directory")?;
    tokio::fs::write(
        project_dir.join("config").join("wasmcloud.toml"),
        r#"
    name = "tinygo-moved"
    version = "0.1.0"
    language = "tinygo"
    type = "component"
    path = "../"
    wit = "../wow"
    build = "artifacts"

    [component]
    wit_world = "hello"
    wasm_target = "wasm32-wasip2"
    "#,
    )
    .await
    .context("failed to update wasmcloud.toml file content for test case")?;

    // Make sure the go generate command uses the `wow` WIT directory
    let tiny_go_main_dot_go = tokio::fs::read_to_string(project_dir.join("hello.go"))
        .await
        .context("failed to read tinygo hello.go")?;
    let new_main_dot_go = tiny_go_main_dot_go.replace(
        "generate --world hello --out gen ./wit",
        "generate --world hello --out gen ./wow",
    );
    tokio::fs::write(project_dir.join("hello.go"), new_main_dot_go)
        .await
        .context("failed to write new main.go")?;

    let status = test_setup
        .base_command()
        .args(["build", "-p", "config/wasmcloud.toml"])
        .kill_on_drop(true)
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("config/artifacts/tinygo-moved.wasm");
    assert!(
        tokio::fs::try_exists(unsigned_file).await.unwrap(),
        "unsigned file not found!"
    );
    let signed_file = project_dir.join("config/artifacts/tinygo-moved_s.wasm");
    assert!(
        tokio::fs::try_exists(signed_file).await.unwrap(),
        "signed file not found!"
    );
    let lock_file = project_dir.join(PACKAGE_LOCK_FILE_NAME);
    assert!(
        tokio::fs::try_exists(lock_file).await.unwrap(),
        "lock file not found!"
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_build_provider_debug_mode() -> Result<()> {
    let test_setup = init_provider(
        /* provider_name= */ "hello-world",
        /* template_name= */ "messaging-nats",
    )
    .await?;

    let project_dir = test_setup.project_dir.clone();

    tokio::fs::write(
        &project_dir.join("wasmcloud.toml"),
        r#"
    name = "Messaging NATS"
    language = "rust"
    type = "provider"

    [provider]
    vendor = "wasmcloud"

    [rust]
    debug = true
    "#,
    )
    .await
    .context("failed to update wasmcloud.toml file content for test case")?;

    let status = test_setup
        .base_command()
        .args(["build"])
        .kill_on_drop(true)
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());

    Ok(())
}
