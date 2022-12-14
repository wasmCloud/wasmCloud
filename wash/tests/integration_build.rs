use anyhow::{anyhow, Result};
use cmd_lib::run_cmd;

mod common;
use scopeguard::defer;
use serial_test::serial;
use std::{
    env::temp_dir,
    fs::{create_dir_all, remove_dir_all},
    path::PathBuf,
};

#[test]
#[serial]
fn build_rust_actor_unsigned() -> Result<()> {
    build_new_project("actor/hello", "hello", "build/hello.wasm", false)
}

#[test]
#[serial]
fn build_rust_actor_signed() -> Result<()> {
    build_new_project("actor/hello", "hello", "build/hello_s.wasm", true)
}

#[test]
#[serial]
fn build_tinygo_actor() -> Result<()> {
    build_new_project("actor/echo-tinygo", "echo", "build/echo_s.wasm", true)
}

fn build_new_project(template: &str, subdir: &str, build_result: &str, signed: bool) -> Result<()> {
    let test_dir = temp_dir().join(template.replace('/', "_"));
    if test_dir.exists() {
        remove_dir_all(&test_dir)?;
    }
    create_dir_all(&test_dir)?;
    defer! {
        remove_dir_all(&test_dir).unwrap();
    }

    std::env::set_current_dir(&test_dir)?;
    let wash = env!("CARGO_BIN_EXE_wash");
    run_cmd!(
        $wash new actor $subdir --git wasmcloud/project-templates --subfolder $template --silent --no-git-init
    ).map_err(|e| anyhow!("wash new actor failed: {}", e))?;

    std::env::set_current_dir(test_dir.join(subdir))?;
    if signed {
        run_cmd!( $wash build )
    } else {
        run_cmd!( $wash build --build-only )
    }
    .map_err(|e| anyhow!("wash build failed: {}", e))?;

    let build_result = PathBuf::from(build_result);
    assert!(build_result.exists(), "build result missing");

    Ok(())
}
