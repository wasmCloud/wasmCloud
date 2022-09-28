use anyhow::Result;

mod common;
use common::wash;
use scopeguard::defer;
use serial_test::serial;
use std::{
    env::temp_dir,
    fs::{create_dir_all, remove_dir_all},
};

#[test]
#[serial]
fn build_rust_actor() -> Result<()> {
    const SUBFOLDER: &str = "build_rust_actor";

    let test_dir = temp_dir().join(SUBFOLDER);
    if test_dir.exists() {
        remove_dir_all(&test_dir)?;
    }
    create_dir_all(&test_dir)?;

    defer! {
        remove_dir_all(&test_dir).unwrap();
    }

    std::env::set_current_dir(&test_dir)?;

    let status = wash()
        .args(&[
            "new",
            "actor",
            "hello",
            "--git",
            "wasmcloud/project-templates",
            "--subfolder",
            "actor/hello",
            "--silent",
            "--no-git-init",
        ])
        .status()
        .expect("Failed to generate project");

    assert!(status.success());

    std::env::set_current_dir(&test_dir.join("hello"))?;

    let status = wash()
        .args(&["build", "--no-sign"])
        .status()
        .expect("Failed to build project");

    assert!(status.success());

    let unsigned_file = test_dir.join("hello/build/hello.wasm");
    assert!(unsigned_file.exists(), "unsigned file not found!");

    let signed_file = test_dir.join("hello/build/hello_s.wasm");
    assert!(
        !signed_file.exists(),
        "signed file should not exist when using --no-sign!"
    );

    Ok(())
}

#[test]
#[serial]
fn build_and_sign_rust_actor() -> Result<()> {
    const SUBFOLDER: &str = "build_and_sign_rust_actor";

    let test_dir = temp_dir().join(SUBFOLDER);
    if test_dir.exists() {
        remove_dir_all(&test_dir)?;
    }
    create_dir_all(&test_dir)?;

    defer! {
        remove_dir_all(&test_dir).unwrap();
    }

    std::env::set_current_dir(&test_dir)?;

    let status = wash()
        .args(&[
            "new",
            "actor",
            "hello",
            "--git",
            "wasmcloud/project-templates",
            "--subfolder",
            "actor/hello",
            "--silent",
            "--no-git-init",
        ])
        .status()
        .expect("Failed to generate project");

    assert!(status.success());

    std::env::set_current_dir(&test_dir.join("hello"))?;

    let status = wash()
        .args(&["build"])
        .status()
        .expect("Failed to build project");

    assert!(status.success());

    let unsigned_file = test_dir.join("hello/build/hello.wasm");
    assert!(unsigned_file.exists(), "unsigned file not found!");

    let signed_file = test_dir.join("hello/build/hello_s.wasm");
    assert!(signed_file.exists(), "signed file not found!");

    Ok(())
}

#[test]
#[serial]
fn build_and_sign_tinygo_actor() -> Result<()> {
    const SUBFOLDER: &str = "build_and_sign_tinygo_actor";

    let test_dir = temp_dir().join(SUBFOLDER);
    if test_dir.exists() {
        remove_dir_all(&test_dir)?;
    }
    create_dir_all(&test_dir)?;

    defer! {
        remove_dir_all(&test_dir).unwrap();
    }

    std::env::set_current_dir(&test_dir)?;

    let status = wash()
        .args(&[
            "new",
            "actor",
            "echo",
            "--git",
            "wasmcloud/project-templates",
            "--subfolder",
            "actor/echo-tinygo",
            "--silent",
            "--no-git-init",
        ])
        .status()
        .expect("Failed to generate project");

    assert!(status.success());

    std::env::set_current_dir(&test_dir.join("echo"))?;

    wash()
        .args(&["build"])
        .status()
        .expect("Failed to build project");

    let unsigned_file = test_dir.join("echo/build/echo.wasm");
    assert!(unsigned_file.exists(), "unsigned file not found!");

    let signed_file = test_dir.join("echo/build/echo_s.wasm");
    assert!(signed_file.exists(), "signed file not found!");

    Ok(())
}
