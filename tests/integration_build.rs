use anyhow::Result;

mod common;
use common::wash;
use serial_test::serial;
use std::{fs::File, path::PathBuf};
use tempfile::TempDir;

#[test]
fn build_rust_actor_unsigned() -> Result<()> {
    let test_setup = init(
        /* actor_name= */ "hello", /* template_name= */ "hello",
    )?;
    let project_dir = test_setup.project_dir;

    let status = wash()
        .args(["build", "--build-only"])
        .status()
        .expect("Failed to build project");

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

#[test]
fn build_rust_actor_signed() -> Result<()> {
    let test_setup = init(
        /* actor_name= */ "hello", /* template_name= */ "hello",
    )?;
    let project_dir = test_setup.project_dir;

    let status = wash()
        .args(["build"])
        .status()
        .expect("Failed to build project");

    assert!(status.success());
    let unsigned_file = project_dir.join("build/hello.wasm");
    assert!(unsigned_file.exists(), "unsigned file not found!");
    let signed_file = project_dir.join("build/hello_s.wasm");
    assert!(signed_file.exists(), "signed file not found!");
    Ok(())
}

#[test]
fn build_tinygo_actor_unsigned() -> Result<()> {
    let test_setup = init(
        /* actor_name= */ "echo",
        /* template_name= */ "echo-tinygo",
    )?;
    let project_dir = test_setup.project_dir;

    let status = wash()
        .args(["build", "--build-only"])
        .status()
        .expect("Failed to build project");

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

#[test]
fn build_tinygo_actor_signed() -> Result<()> {
    let test_setup = init(
        /* actor_name= */ "echo",
        /* template_name= */ "echo-tinygo",
    )?;
    let project_dir = test_setup.project_dir;

    let status = wash()
        .args(["build"])
        .status()
        .expect("Failed to build project");

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

/// Inits an actor build test by setting up a test directory and creating an actor from a template.
/// Returns the paths of the test directory and actor directory.
fn init(actor_name: &str, template_name: &str) -> Result<TestSetup> {
    let test_dir = TempDir::new()?;
    std::env::set_current_dir(&test_dir)?;
    let project_dir = init_actor_from_template(actor_name, template_name)?;
    std::env::set_current_dir(&project_dir)?;
    Ok(TestSetup {
        test_dir,
        project_dir,
    })
}

/// Initializes a new actor from a wasmCloud template, and sets the environment to use the created actor's directory.
fn init_actor_from_template(actor_name: &str, template_name: &str) -> Result<PathBuf> {
    let status = wash()
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
        .status()
        .expect("Failed to generate project");

    assert!(status.success());

    let project_dir = std::env::current_dir()?.join(actor_name);
    Ok(project_dir)
}

#[test]
#[serial]
fn integration_build_handles_dashed_names() -> Result<()> {
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
    let mut new_cmd = wash()
        .args(["new", "actor", "dashed-actor", "-t", "hello"])
        .current_dir(&root_dir)
        .stdout(stdout.try_clone()?)
        .spawn()?;
    assert!(new_cmd.wait()?.success());

    // Ensure that the actor dir was created as expected
    assert!(actor_dir.exists());

    let mut build_cmd = wash()
        .args(["build"])
        .stdout(stdout)
        .current_dir(&actor_dir)
        .spawn()?;

    assert!(build_cmd.wait()?.success());

    Ok(())
}
