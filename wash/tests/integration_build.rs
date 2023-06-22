use anyhow::{Context, Result};
use std::fs::File;
use tokio::process::Command;

mod common;

use crate::common::{init, init_workspace};

#[tokio::test]
async fn integration_build_rust_actor_unsigned() -> Result<()> {
    let test_setup = init(
        /* actor_name= */ "hello", /* template_name= */ "hello",
    )
    .await?;
    let project_dir = test_setup.project_dir;

    let status = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["build", "--build-only"])
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("build/hello.wasm");
    assert!(unsigned_file.exists(), "unsigned file not found!");
    let signed_file = project_dir.join("build/hello_s.wasm");
    assert!(
        !signed_file.exists(),
        "signed file should not exist when using --build-only!"
    );
    Ok(())
}

#[tokio::test]
async fn integration_build_rust_actor_signed() -> Result<()> {
    let test_setup = init(
        /* actor_name= */ "hello", /* template_name= */ "hello",
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
    let unsigned_file = project_dir.join("build/hello.wasm");
    assert!(unsigned_file.exists(), "unsigned file not found!");
    let signed_file = project_dir.join("build/hello_s.wasm");
    assert!(signed_file.exists(), "signed file not found!");
    Ok(())
}

#[tokio::test]
async fn integration_build_rust_actor_in_workspace_unsigned() -> Result<()> {
    let test_setup = init_workspace(vec![/* actor_names= */ "hello-1", "hello-2"]).await?;
    let project_dir = test_setup.project_dirs.get(0).unwrap();
    std::env::set_current_dir(project_dir)?;

    let status = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["build", "--build-only"])
        .kill_on_drop(true)
        .status()
        .await
        .context("Failed to build project")?;

    assert!(status.success());
    let unsigned_file = project_dir.join("build/hello_1.wasm");
    assert!(unsigned_file.exists(), "unsigned file not found!");
    let signed_file = project_dir.join("build/hello_1_s.wasm");
    assert!(
        !signed_file.exists(),
        "signed file should not exist when using --build-only!"
    );
    Ok(())
}

#[tokio::test]
async fn integration_build_tinygo_actor_unsigned() -> Result<()> {
    let test_setup = init(
        /* actor_name= */ "echo",
        /* template_name= */ "echo-tinygo",
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
    let unsigned_file = project_dir.join("build/echo.wasm");
    assert!(unsigned_file.exists(), "unsigned file not found!");
    let signed_file = project_dir.join("build/echo_s.wasm");
    assert!(
        !signed_file.exists(),
        "signed file should not exist when using --build-only!"
    );
    Ok(())
}

#[tokio::test]
async fn integration_build_tinygo_actor_signed() -> Result<()> {
    let test_setup = init(
        /* actor_name= */ "echo",
        /* template_name= */ "echo-tinygo",
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
    let unsigned_file = project_dir.join("build/echo.wasm");
    assert!(unsigned_file.exists(), "unsigned file not found!");
    let signed_file = project_dir.join("build/echo_s.wasm");
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
        .args(["new", "actor", "dashed-actor", "-t", "hello"])
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
