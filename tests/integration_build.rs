use anyhow::{Context, Result};
use std::{fs::File, io::Write, path::PathBuf};
use tempfile::TempDir;
use tokio::process::Command;

mod common;

#[tokio::test]
async fn build_rust_actor_unsigned_serial() -> Result<()> {
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
async fn build_rust_actor_signed_serial() -> Result<()> {
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
async fn build_rust_actor_in_workspace_unsigned_serial() -> Result<()> {
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
async fn build_tinygo_actor_unsigned_serial() -> Result<()> {
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
async fn build_tinygo_actor_signed_serial() -> Result<()> {
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

struct TestSetup {
    /// The path to the directory for the test.
    /// Added here so that the directory is not deleted until the end of the test.
    #[allow(dead_code)]
    test_dir: TempDir,
    /// The path to the created actor's directory.
    project_dir: PathBuf,
}

struct WorkspaceTestSetup {
    /// The path to the directory for the test.
    /// Added here so that the directory is not deleted until the end of the test.
    #[allow(dead_code)]
    test_dir: TempDir,
    /// The path to the created actor's directory.
    project_dirs: Vec<PathBuf>,
}

/// Inits an actor build test by setting up a test directory and creating an actor from a template.
/// Returns the paths of the test directory and actor directory.
async fn init(actor_name: &str, template_name: &str) -> Result<TestSetup> {
    let test_dir = TempDir::new()?;
    std::env::set_current_dir(&test_dir)?;
    let project_dir = init_actor_from_template(actor_name, template_name).await?;
    std::env::set_current_dir(&project_dir)?;
    Ok(TestSetup {
        test_dir,
        project_dir,
    })
}

/// Inits an actor build test by setting up a test directory and creating an actor from a template.
/// Returns the paths of the test directory and actor directory.
async fn init_workspace(actor_names: Vec<&str>) -> Result<WorkspaceTestSetup> {
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
    let mut file = File::create(cargo_path)?;
    file.write_all(cargo_toml.as_bytes())?;
    Ok(WorkspaceTestSetup {
        test_dir,
        project_dirs,
    })
}

/// Initializes a new actor from a wasmCloud template, and sets the environment to use the created actor's directory.
async fn init_actor_from_template(actor_name: &str, template_name: &str) -> Result<PathBuf> {
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

#[tokio::test]
async fn integration_build_handles_dashed_names_serial() -> Result<()> {
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
