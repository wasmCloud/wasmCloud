mod common;

use common::{get_json_output, output_to_string, test_dir_file, test_dir_with_subfolder, wash};

use std::{
    fs::{remove_dir_all, File},
    io::prelude::*,
};

use serde_json::json;

const ECHO_WASM: &str = "wasmcloud.azurecr.io/echo:0.2.0";
const LOGGING_PAR: &str = "wasmcloud.azurecr.io/logging:0.9.1";
const LOCAL_REGISTRY: &str = "localhost:5001";

#[test]
#[cfg_attr(
    not(can_reach_wasmcloud_azurecr_io),
    ignore = "wasmcloud.azurecr.io is not reachable"
)]
fn integration_reg_pull_basic() {
    const SUBFOLDER: &str = "pull_basic";
    let pull_dir = test_dir_with_subfolder(SUBFOLDER);

    let basic_echo = test_dir_file(SUBFOLDER, "basic_echo.wasm");

    let pull_basic = wash()
        .args([
            "pull",
            ECHO_WASM,
            "--destination",
            basic_echo.to_str().unwrap(),
            "--allow-latest",
        ])
        .output()
        .unwrap_or_else(|_| panic!("failed to pull {ECHO_WASM}"));
    assert!(pull_basic.status.success());
    // Very important
    assert!(output_to_string(pull_basic).unwrap().contains('\u{1F6BF}'));

    remove_dir_all(pull_dir).unwrap();
}

#[test]
#[cfg_attr(
    not(can_reach_wasmcloud_azurecr_io),
    ignore = "wasmcloud.azurecr.io is not reachable"
)]
fn integration_reg_pull_comprehensive() {
    const SUBFOLDER: &str = "pull_comprehensive";
    let pull_dir = test_dir_with_subfolder(SUBFOLDER);

    let comprehensive_echo = test_dir_file(SUBFOLDER, "comprehensive_echo.wasm");
    let comprehensive_logging = test_dir_file(SUBFOLDER, "comprehensive_logging.par.gz");

    let pull_echo_comprehensive = wash()
        .args([
            "pull",
            ECHO_WASM,
            "--destination",
            comprehensive_echo.to_str().unwrap(),
            "--digest",
            "sha256:a17a163afa8447622055deb049587641a9e23243a6cc4411eb33bd4267214cf3",
            "--output",
            "json",
        ])
        .output()
        .unwrap_or_else(|_| panic!("failed to pull {ECHO_WASM}"));

    assert!(pull_echo_comprehensive.status.success());
    let output = get_json_output(pull_echo_comprehensive).unwrap();

    let expected_json = json!({"file": comprehensive_echo.to_str().unwrap(), "success": true});

    assert_eq!(output, expected_json);

    let pull_logging_comprehensive = wash()
        .args([
            "pull",
            LOGGING_PAR,
            "--destination",
            comprehensive_logging.to_str().unwrap(),
            "--digest",
            "sha256:169f2764e529c2b57ad20abb87e0854d67bf6f0912896865e2911dee1bf6af98",
            "--output",
            "json",
        ])
        .output()
        .unwrap_or_else(|_| panic!("failed to pull {ECHO_WASM}"));

    assert!(pull_logging_comprehensive.status.success());
    let output = get_json_output(pull_logging_comprehensive).unwrap();

    let expected_json = json!({"file": comprehensive_logging.to_str().unwrap(), "success": true});

    assert_eq!(output, expected_json);

    remove_dir_all(pull_dir).unwrap();
}

// NOTE: This test will fail without a local docker registry running
#[test]
#[cfg_attr(
    not(can_reach_wasmcloud_azurecr_io),
    ignore = "wasmcloud.azurecr.io is not reachable"
)]
fn integration_reg_push_basic() {
    const SUBFOLDER: &str = "push_basic";
    let push_dir = test_dir_with_subfolder(SUBFOLDER);

    let pull_echo_wasm = test_dir_file(SUBFOLDER, "echo.wasm");

    // Pull echo.wasm for push tests
    wash()
        .args([
            "pull",
            ECHO_WASM,
            "--destination",
            pull_echo_wasm.to_str().unwrap(),
        ])
        .stderr(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .output()
        .unwrap_or_else(|_| panic!("failed to pull {ECHO_WASM} for push basic"));

    // Push echo.wasm and pull from local registry
    let echo_push_basic = &format!("{LOCAL_REGISTRY}/echo:pushbasic");
    let localregistry_echo_wasm = test_dir_file(SUBFOLDER, "echo_local.wasm");
    let push_echo = wash()
        .args([
            "reg",
            "push",
            echo_push_basic,
            pull_echo_wasm.to_str().unwrap(),
            "--insecure",
        ])
        .stderr(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .output()
        .expect("failed to push echo.wasm to local registry");
    assert!(
        push_echo.status.success(),
        "failed to push to local registry"
    );

    let pull_local_registry_echo = wash()
        .args([
            "reg",
            "pull",
            echo_push_basic,
            "--insecure",
            "--destination",
            localregistry_echo_wasm.to_str().unwrap(),
        ])
        .stderr(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .output()
        .expect("failed to pull echo.wasm from local registry");

    assert!(
        pull_local_registry_echo.status.success(),
        "failed to pull echo.wasm from local registry"
    );

    remove_dir_all(push_dir).unwrap();
}

// NOTE: This test will fail without a local docker registry running
#[test]
#[cfg_attr(
    not(can_reach_wasmcloud_azurecr_io),
    ignore = "wasmcloud.azurecr.io is not reachable"
)]
fn integration_reg_push_comprehensive() {
    const SUBFOLDER: &str = "push_comprehensive";
    let push_dir = test_dir_with_subfolder(SUBFOLDER);

    let pull_echo_wasm = test_dir_file(SUBFOLDER, "echo.wasm");
    let pull_logging_par = test_dir_file(SUBFOLDER, "logging.par.gz");

    // Pull echo.wasm and logging.par.gz for push tests
    wash()
        .args([
            "pull",
            ECHO_WASM,
            "--destination",
            pull_echo_wasm.to_str().unwrap(),
        ])
        .output()
        .unwrap_or_else(|_| panic!("failed to pull {ECHO_WASM} for push basic"));
    wash()
        .args([
            "reg",
            "pull",
            LOGGING_PAR,
            "--destination",
            pull_logging_par.to_str().unwrap(),
        ])
        .output()
        .unwrap_or_else(|_| panic!("failed to pull {LOGGING_PAR} for push basic"));

    let config_json = test_dir_file(SUBFOLDER, "config.json");
    let mut config = File::create(config_json.clone()).unwrap();
    config.write_all(b"{}").unwrap();

    let logging_push_all_options = &format!("{LOCAL_REGISTRY}/logging:alloptions");
    let push_all_options = wash()
        .args([
            "push",
            logging_push_all_options,
            pull_logging_par.to_str().unwrap(),
            "--allow-latest",
            "--insecure",
            "--config",
            config_json.to_str().unwrap(),
            "--output",
            "json",
            "--password",
            "supers3cr3t",
            "--user",
            "localuser",
        ])
        .output()
        .unwrap_or_else(|_| panic!("failed to push {LOGGING_PAR} for push comprehensive"));
    assert!(push_all_options.status.success());

    let output = get_json_output(push_all_options).unwrap();

    let expected_json = json!({"url": logging_push_all_options, "success": true});

    assert_eq!(output, expected_json);

    remove_dir_all(push_dir).unwrap();
}
