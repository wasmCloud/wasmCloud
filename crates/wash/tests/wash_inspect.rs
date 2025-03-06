mod common;

use common::{get_json_output, test_dir_file, test_dir_with_subfolder, wash, LOCAL_REGISTRY};

use std::{
    env::temp_dir,
    fs::{remove_dir_all, remove_file},
};

use assert_json_diff::assert_json_include;
use serde_json::json;

#[test]
fn integration_inspect_component() {
    const SUBFOLDER: &str = "inspect";
    const ECHO_OCI: &str = "ghcr.io/wasmcloud/components/http-hello-world-rust:0.1.0";
    const ECHO_ACC: &str = "ADVIWF6Z3BFZNWUXJYT5NEAZZ2YX4T6NRKI3YOR3HKOSQQN7IVDGWSNO";
    const ECHO_MOD: &str = "MBFFVNGFK3IA2ZXXG5DQXQNYM6TNG45PHJMJIJFVFI6YKS3XTXL3DRRK";
    const ECHO_SHA: &str =
        "sha256:079275a324c0fcd0c201878f0c158120c4984472215ec3f64eb91ba9ee139f72";
    let inspect_dir = test_dir_with_subfolder(SUBFOLDER);
    let echo_inspect = &format!("{LOCAL_REGISTRY}/echo:inspect");

    // Pull the echo module and push to local registry to test local inspect
    let echo = test_dir_file(SUBFOLDER, "echo.wasm");
    let get_hello_wasm = wash()
        .args(["pull", ECHO_OCI, "--destination", echo.to_str().unwrap()])
        .output()
        .expect("failed to pull echo for claims sign test");
    assert!(get_hello_wasm.status.success());
    let push_echo = wash()
        .args(["push", echo_inspect, echo.to_str().unwrap(), "--insecure"])
        .output()
        .expect("failed to push echo.wasm to local registry");
    assert!(push_echo.status.success());

    // Inspect local, local registry, and remote registry component wasm
    let local_inspect = wash()
        .args(["inspect", echo.to_str().unwrap(), "--output", "json"])
        .output()
        .expect("failed to inspect local wasm");
    assert!(local_inspect.status.success());

    let local_inspect_output = get_json_output(local_inspect).unwrap();

    let expected_inspect_output = json!({
        "account": ECHO_ACC,
        "component": ECHO_MOD,
        "can_be_used": "immediately",
        "expires": "never",
        "version": "0.1.0"
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
        .args(["inspect", ECHO_OCI, "--digest", ECHO_SHA, "-o", "json"])
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
    let httpclient_inspect = &format!("{LOCAL_REGISTRY}/httpclient:inspect");

    // Pull the httpclient provider and push to local registry to test local inspect
    let local_http_client_path = test_dir_file(SUBFOLDER, "httpclient.wasm");
    let get_http_client = wash()
        .args([
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
            "push",
            httpclient_inspect,
            local_http_client_path.to_str().unwrap(),
            "--insecure",
        ])
        .output()
        .expect("failed to push echo.wasm to local registry");
    assert!(push_http_client.status.success());

    // Inspect local, local registry, and remote registry component wasm
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
    });
    assert_json_include!(actual: remote_inspect_output, expected: expected_output);

    let remote_inspect_no_cache = wash()
        .args(["inspect", HTTP_FAKE_OCI, "-o", "json", "--no-cache"])
        .output()
        .expect("failed to inspect remote cached registry");

    assert!(!remote_inspect_no_cache.status.success());

    let _ = remove_file(http_client_cache_path);
}
