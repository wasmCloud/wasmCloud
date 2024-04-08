mod common;

use common::{init, init_workspace};

use anyhow::{Context, Result};
use std::env;
use std::fs::File;
use tokio::process::Command;

#[tokio::test]
async fn integration_build_rust_actor_unsigned() -> Result<()> {
    let test_setup = init(
        /* actor_name= */ "hello-unsigned",
        /* template_name= */ "hello-world-rust",
    )
    .await?;
    let project_dir = test_setup.project_dir;

    let status = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["build", "--build-only"])
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("build/http_hello_world.wasm");
    assert!(unsigned_file.exists(), "unsigned file not found!");
    let signed_file = project_dir.join("build/http_hello_world_s.wasm");
    assert!(
        !signed_file.exists(),
        "signed file should not exist when using --build-only!"
    );
    Ok(())
}

#[tokio::test]
async fn integration_build_rust_actor_signed() -> Result<()> {
    let test_setup = init(
        /* actor_name= */ "hello",
        /* template_name= */ "hello-world-rust",
    )
    .await?;
    let project_dir = test_setup.project_dir;

    let status = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["build"])
        .kill_on_drop(true)
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("build/http_hello_world.wasm");
    assert!(unsigned_file.exists(), "unsigned file not found!");
    let signed_file = project_dir.join("build/http_hello_world_s.wasm");
    assert!(signed_file.exists(), "signed file not found!");
    Ok(())
}

#[tokio::test]
async fn integration_build_rust_actor_signed_with_signing_keys_directory_configuration(
) -> Result<()> {
    let test_setup = init(
        /* actor_name= */ "hello",
        /* template_name= */ "hello-world-rust",
    )
    .await?;
    let project_dir = test_setup.project_dir;
    env::set_current_dir(&project_dir)?;
    env::set_var("RUST_LOG", "debug");

    // base case: no keys directory configured
    let mut expected_default_key_dir = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Unable to determine the user's home directory"))?;
    expected_default_key_dir.push(".wash/keys");

    let cmd = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["build"])
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
    assert!(output.contains(expected_default_key_dir.to_str().unwrap()));

    // case: keys directory configured via cli arg --keys-directory
    let key_directory = project_dir.join("batmankeys").to_string_lossy().to_string();
    let cmd = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["build", "--keys-directory", &key_directory])
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

    // case: keys directory configured via cli arg --keys-directory and --disable-keygen=true
    let key_directory = project_dir
        .join("spidermankeys")
        .to_string_lossy()
        .to_string();
    let cmd = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "build",
            "--keys-directory",
            &key_directory,
            "--disable-keygen",
        ])
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
    let cmd = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["build"])
        .env("WASH_KEYS", &key_directory)
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
    type = "actor"
    
    [actor]
    claims = ["wasmcloud:httpserver"]
    key_directory = "./haljordankeys"
    "#,
    )
    .await
    .context("failed to update wasmcloud.toml file content for test case")?;

    let cmd = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["build"])
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

    // case when keys directory is configured via cli arg --keys-directory and wasmcloud.toml. cli arg should take precedence
    let key_directory = project_dir
        .join("wonderwomankeys")
        .to_string_lossy()
        .to_string();

    let cmd = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["build", "--keys-directory", &key_directory])
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

    let cmd = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["build", "--keys-directory", &key_directory])
        .env("WASH_KEYS", &env_key_directory)
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
    let cmd = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["build"])
        .env("WASH_KEYS", &env_key_directory)
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
async fn integration_build_rust_actor_in_workspace_unsigned() -> Result<()> {
    let test_setup = init_workspace(vec![/* actor_names= */ "hello-1", "hello-2"]).await?;
    let project_dir = test_setup.project_dirs.first().unwrap();
    std::env::set_current_dir(project_dir)?;

    let status = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["build", "--build-only"])
        .kill_on_drop(true)
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("build/http_hello_world.wasm");
    assert!(unsigned_file.exists(), "unsigned file not found!");
    let signed_file = project_dir.join("build/http_hello_world_s.wasm");
    assert!(
        !signed_file.exists(),
        "signed file should not exist when using --build-only!"
    );
    Ok(())
}

#[tokio::test]
async fn integration_build_tinygo_actor_unsigned() -> Result<()> {
    let test_setup = init(
        /* actor_name= */ "hello-world-tinygo",
        /* template_name= */ "hello-world-tinygo",
    )
    .await?;
    let project_dir = test_setup.project_dir;

    let status = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["build", "--build-only"])
        .kill_on_drop(true)
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("build/http-hello-world.wasm");
    assert!(unsigned_file.exists(), "unsigned file not found!");
    let signed_file = project_dir.join("build/http_hello_world_s.wasm");
    assert!(
        !signed_file.exists(),
        "signed file should not exist when using --build-only!"
    );
    Ok(())
}

#[tokio::test]
async fn integration_build_tinygo_actor_signed() -> Result<()> {
    let test_setup = init(
        /* actor_name= */ "hello-world-tinygo",
        /* template_name= */ "hello-world-tinygo",
    )
    .await?;
    let project_dir = test_setup.project_dir;

    let status = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["build"])
        .kill_on_drop(true)
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("build/http-hello-world.wasm");
    assert!(unsigned_file.exists(), "unsigned file not found!");
    let signed_file = project_dir.join("build/http_hello_world_s.wasm");
    assert!(signed_file.exists(), "signed file not found!");
    Ok(())
}

#[tokio::test]
async fn integration_build_handles_dashed_names() -> Result<()> {
    let actor_name = "dashed-actor";
    // This tests runs against a temp directory since cargo gets confused
    // about workspace projects if done from within wash
    let root_dir = tempfile::tempdir()?;
    let actor_dir = root_dir.path().join(actor_name);
    let stdout_path = root_dir
        .path()
        .join(format!("wash-test.{actor_name}.stdout.log"));
    let stdout = File::create(stdout_path)?;

    // Execute wash new to create an actor with the given name
    let mut new_cmd = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["new", "actor", "dashed-actor", "-t", "hello-world-rust"])
        .kill_on_drop(true)
        .current_dir(&root_dir)
        .stdout(stdout.try_clone()?)
        .spawn()?;
    assert!(new_cmd.wait().await?.success());

    // Ensure that the actor dir was created as expected
    assert!(actor_dir.exists());

    let mut build_cmd = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["build"])
        .kill_on_drop(true)
        .stdout(stdout)
        .current_dir(&actor_dir)
        .spawn()?;

    assert!(build_cmd.wait().await?.success());

    Ok(())
}
