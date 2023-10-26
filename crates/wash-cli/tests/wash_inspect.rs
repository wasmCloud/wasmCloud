mod common;

use common::{get_json_output, test_dir_file, test_dir_with_subfolder, wash, LOCAL_REGISTRY};

use std::{
    env::temp_dir,
    fs::{remove_dir_all, remove_file},
};

use assert_json_diff::assert_json_include;
use serde_json::json;

#[test]
fn integration_inspect_actor() {
    const SUBFOLDER: &str = "inspect";
    const ECHO_OCI: &str = "wasmcloud.azurecr.io/echo:0.2.1";
    const ECHO_ACC: &str = "ACOJJN6WUP4ODD75XEBKKTCCUJJCY5ZKQ56XVKYK4BEJWGVAOOQHZMCW";
    const ECHO_MOD: &str = "MBCFOPM6JW2APJLXJD3Z5O4CN7CPYJ2B4FTKLJUR5YR5MITIU7HD3WD5";
    let inspect_dir = test_dir_with_subfolder(SUBFOLDER);
    let echo_inspect = &format!("{}/echo:inspect", LOCAL_REGISTRY);

    // Pull the echo module and push to local registry to test local inspect
    let echo = test_dir_file(SUBFOLDER, "echo.wasm");
    let get_hello_wasm = wash()
        .args([
            "reg",
            "pull",
            ECHO_OCI,
            "--destination",
            echo.to_str().unwrap(),
        ])
        .output()
        .expect("failed to pull echo for claims sign test");
    assert!(get_hello_wasm.status.success());
    let push_echo = wash()
        .args([
            "reg",
            "push",
            echo_inspect,
            echo.to_str().unwrap(),
            "--insecure",
        ])
        .output()
        .expect("failed to push echo.wasm to local registry");
    assert!(push_echo.status.success());

    // Inspect local, local registry, and remote registry actor wasm
    let local_inspect = wash()
        .args(["inspect", echo.to_str().unwrap(), "--output", "json"])
        .output()
        .expect("failed to inspect local wasm");
    assert!(local_inspect.status.success());

    let local_inspect_output = get_json_output(local_inspect).unwrap();

    let expected_inspect_output = json!({
            "account": ECHO_ACC,
            "module": ECHO_MOD,
            "can_be_used": "immediately",
            "capabilities": ["HTTP Server"],
            "expires": "never",
            "tags": "None",
            "version": "0.2.1"

    });

    assert_json_include!(
        actual: local_inspect_output,
        expected: expected_inspect_output
    );

    let local_reg_inspect = wash()
        .args(["inspect", echo_inspect, "--insecure", "-o", "json"])
        .output()
        .expect("failed to inspect local registry wasm");
    assert!(local_reg_inspect.status.success());
    let local_reg_inspect_output = get_json_output(local_reg_inspect).unwrap();

    assert_json_include!(
        actual: local_reg_inspect_output,
        expected: expected_inspect_output
    );

    let remote_inspect = wash()
        .args([
            "inspect",
            ECHO_OCI,
            "--digest",
            "sha256:55689502d1bc9c48f22b278c54efeee206a839b8e8eedd4ea6b19e6861f66b3c",
            "-o",
            "json",
        ])
        .output()
        .expect("failed to inspect local registry wasm");
    assert!(remote_inspect.status.success());
    let remote_inspect_output = get_json_output(remote_inspect).unwrap();

    assert_json_include!(
        actual: remote_inspect_output,
        expected: expected_inspect_output
    );

    remove_dir_all(inspect_dir).unwrap();
}

#[test]
fn integration_inspect_provider() {
    const SUBFOLDER: &str = "inspect_test";
    const HTTP_OCI: &str = "wasmcloud.azurecr.io/httpclient:0.3.5";
    const HTTP_ISSUER: &str = "ACOJJN6WUP4ODD75XEBKKTCCUJJCY5ZKQ56XVKYK4BEJWGVAOOQHZMCW";
    const HTTP_SERVICE: &str = "VCCVLH4XWGI3SGARFNYKYT2A32SUYA2KVAIV2U2Q34DQA7WWJPFRKIKM";
    let inspect_dir = test_dir_with_subfolder(SUBFOLDER);
    let httpclient_inspect = &format!("{}/httpclient:inspect", LOCAL_REGISTRY);

    // Pull the httpclient provider and push to local registry to test local inspect
    let local_http_client_path = test_dir_file(SUBFOLDER, "httpclient.wasm");
    let get_http_client = wash()
        .args([
            "reg",
            "pull",
            HTTP_OCI,
            "--destination",
            local_http_client_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to pull https server for inspect test");
    assert!(get_http_client.status.success());
    let push_http_client = wash()
        .args([
            "reg",
            "push",
            httpclient_inspect,
            local_http_client_path.to_str().unwrap(),
            "--insecure",
        ])
        .output()
        .expect("failed to push echo.wasm to local registry");
    assert!(push_http_client.status.success());

    // Inspect local, local registry, and remote registry actor wasm
    // `String.contains` is used here to ensure we aren't relying on relative json field position.
    // This also allows tests to pass if information is _added_ but not if information is _omitted_
    // from the command output
    let local_inspect = wash()
        .args([
            "inspect",
            local_http_client_path.to_str().unwrap(),
            "--output",
            "json",
        ])
        .output()
        .expect("failed to inspect local http server");
    assert!(local_inspect.status.success());
    let local_inspect_output = get_json_output(local_inspect).unwrap();
    let inspect_expected = json!({
        "issuer": HTTP_ISSUER,
        "service": HTTP_SERVICE,
        "capability_contract_id": "wasmcloud:httpclient",
    });
    assert_json_include!(actual: local_inspect_output, expected: inspect_expected);

    let local_reg_inspect = wash()
        .args(["inspect", httpclient_inspect, "--insecure", "-o", "json"])
        .output()
        .expect("failed to inspect local registry wasm");
    assert!(local_reg_inspect.status.success());
    let local_reg_inspect_output = get_json_output(local_reg_inspect).unwrap();
    assert_json_include!(actual: local_reg_inspect_output, expected: inspect_expected);

    let remote_inspect = wash()
        .args(["inspect", HTTP_OCI, "-o", "json"])
        .output()
        .expect("failed to inspect local registry wasm");
    assert!(remote_inspect.status.success());
    let remote_inspect_output = get_json_output(remote_inspect).unwrap();
    assert_json_include!(actual: remote_inspect_output, expected: inspect_expected);

    remove_dir_all(inspect_dir).unwrap();
}

#[test]
fn integration_inspect_cached() {
    const HTTP_OCI: &str = "wasmcloud.azurecr.io/httpclient:0.3.5";
    const HTTP_FAKE_OCI: &str = "foo.bar.io/httpclient:0.3.5";
    const HTTP_FAKE_CACHED: &str = "foo_bar_io_httpclient_0_3_5";
    const HTTP_ISSUER: &str = "ACOJJN6WUP4ODD75XEBKKTCCUJJCY5ZKQ56XVKYK4BEJWGVAOOQHZMCW";
    const HTTP_SERVICE: &str = "VCCVLH4XWGI3SGARFNYKYT2A32SUYA2KVAIV2U2Q34DQA7WWJPFRKIKM";

    let mut http_client_cache_path = temp_dir().join("wasmcloud_ocicache").join(HTTP_FAKE_CACHED);
    let _ = ::std::fs::create_dir_all(&http_client_cache_path);
    http_client_cache_path.set_extension("bin");

    let get_http_client = wash()
        .args([
            "reg",
            "pull",
            HTTP_OCI,
            "--destination",
            http_client_cache_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to pull echo for claims sign test");
    assert!(get_http_client.status.success());

    let remote_inspect = wash()
        .args(["inspect", HTTP_FAKE_OCI, "-o", "json"])
        .output()
        .expect("failed to inspect remote cached registry");
    assert!(remote_inspect.status.success());
    let remote_inspect_output = get_json_output(remote_inspect).unwrap();
    let expected_output = json!({
        "issuer": HTTP_ISSUER,
        "service": HTTP_SERVICE,
        "capability_contract_id": "wasmcloud:httpclient",
    });
    assert_json_include!(actual: remote_inspect_output, expected: expected_output);

    let remote_inspect_no_cache = wash()
        .args(["inspect", HTTP_FAKE_OCI, "-o", "json", "--no-cache"])
        .output()
        .expect("failed to inspect remote cached registry");

    assert!(!remote_inspect_no_cache.status.success());

    let _ = remove_file(http_client_cache_path);
}
