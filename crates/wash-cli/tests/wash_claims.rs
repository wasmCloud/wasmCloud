mod common;

use common::{
    get_json_output, output_to_string, test_dir_file, test_dir_with_subfolder, wash, LOCAL_REGISTRY,
};

use std::{
    env::temp_dir,
    fs::{remove_dir_all, remove_file},
};

use assert_json_diff::assert_json_include;
use serde_json::json;

const ECHO_OCI: &str = "ghcr.io/wasmcloud/components/http-hello-world-rust:0.1.0";
const ECHO_ACC: &str = "ADVIWF6Z3BFZNWUXJYT5NEAZZ2YX4T6NRKI3YOR3HKOSQQN7IVDGWSNO";
const ECHO_MOD: &str = "MBFFVNGFK3IA2ZXXG5DQXQNYM6TNG45PHJMJIJFVFI6YKS3XTXL3DRRK";
const ECHO_SHA: &str = "sha256:079275a324c0fcd0c201878f0c158120c4984472215ec3f64eb91ba9ee139f72";

#[test]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
fn integration_claims_sign() {
    const SUBFOLDER: &str = "claims_sign";
    let sign_dir = test_dir_with_subfolder(SUBFOLDER);
    const ISSUER: &str = "SAAAF62YYA6UCKZNSE7UF7GVWEHHYASDSUSSVCEEHTH3WY57DJKVXKIKOY";
    const SUBJECT: &str = "SMAJFHSZUOYLLPIW3HLYOMM7F6GABRPCETIVQ27BVYZ55XQNFYNTUTH57Y";

    // Pull the echo module to test signing
    // During the process of signing a module, the previous "jwt" section
    // is cleared from a signed module, so this is just as effective
    // as signing an unsigned wasm
    let echo = test_dir_file(SUBFOLDER, "echo.wasm");
    let get_hello_wasm = wash()
        .args(["pull", ECHO_OCI, "--destination", echo.to_str().unwrap()])
        .output()
        .expect("failed to pull echo for claims sign test");
    assert!(get_hello_wasm.status.success());

    let signed_wasm_path = test_dir_file(SUBFOLDER, "echo_signed.wasm");
    let sign_echo = wash()
        .args([
            "claims",
            "sign",
            echo.to_str().unwrap(),
            "--name",
            "EchoSigned",
            "--ver",
            "0.1.0",
            "--rev",
            "1",
            "--issuer",
            ISSUER,
            "--subject",
            SUBJECT,
            "--disable-keygen",
            "--destination",
            signed_wasm_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to sign echo module");
    assert!(sign_echo.status.success());
    assert_eq!(
        output_to_string(sign_echo).unwrap(),
        format!(
            "\nSuccessfully signed {}\n",
            signed_wasm_path.to_str().unwrap()
        )
    );

    // signing should fail when revision or/and version are not provided
    let sign_echo = wash()
        .args([
            "claims",
            "sign",
            echo.to_str().unwrap(),
            "--name",
            "EchoSigned",
            "--ver",
            "0.1.0",
            // "--rev",
            // "1",
            "--http_server",
            "--issuer",
            ISSUER,
            "--subject",
            SUBJECT,
            "--disable-keygen",
            "--destination",
            signed_wasm_path.to_str().unwrap(),
        ])
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("failed to run sign command");
    assert!(!sign_echo.status.success());

    let sign_echo = wash()
        .args([
            "claims",
            "sign",
            echo.to_str().unwrap(),
            "--name",
            "EchoSigned",
            // "--ver",
            // "0.1.0",
            "--rev",
            "1",
            "--issuer",
            ISSUER,
            "--subject",
            SUBJECT,
            "--disable-keygen",
            "--destination",
            signed_wasm_path.to_str().unwrap(),
        ])
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("failed to run sign command");
    assert!(!sign_echo.status.success());

    let sign_echo = wash()
        .args([
            "claims",
            "sign",
            echo.to_str().unwrap(),
            "--name",
            "EchoSigned",
            "--issuer",
            ISSUER,
            "--subject",
            SUBJECT,
            "--disable-keygen",
            "--destination",
            signed_wasm_path.to_str().unwrap(),
        ])
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("failed to run sign command");
    assert!(!sign_echo.status.success());

    remove_dir_all(sign_dir).unwrap();
}

#[test]
#[cfg_attr(
    not(can_reach_wasmcloud_azurecr_io),
    ignore = "wasmcloud.azurecr.io is not reachable"
)]
fn integration_claims_inspect() {
    const SUBFOLDER: &str = "claims_inspect";
    let inspect_dir = test_dir_with_subfolder(SUBFOLDER);
    let echo_claims = &format!("{LOCAL_REGISTRY}/echo:claimsinspect");

    // Pull the echo module and push to local registry to test local inspect
    let echo = test_dir_file(SUBFOLDER, "echo.wasm");
    let get_hello_wasm = wash()
        .args(["pull", ECHO_OCI, "--destination", echo.to_str().unwrap()])
        .output()
        .expect("failed to pull echo for claims sign test");
    assert!(get_hello_wasm.status.success());
    let push_echo = wash()
        .args(["push", echo_claims, echo.to_str().unwrap(), "--insecure"])
        .output()
        .expect("failed to push echo.wasm to local registry");
    assert!(push_echo.status.success());

    // Inspect local, local registry, and remote registry component wasm
    let local_inspect = wash()
        .args([
            "claims",
            "inspect",
            echo.to_str().unwrap(),
            "--output",
            "json",
        ])
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
        .args(["claims", "inspect", echo_claims, "--insecure", "-o", "json"])
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
            "claims", "inspect", ECHO_OCI, "--digest", ECHO_SHA, "-o", "json",
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
#[cfg_attr(
    not(can_reach_wasmcloud_azurecr_io),
    ignore = "wasmcloud.azurecr.io is not reachable"
)]
fn integration_claims_inspect_cached() {
    const ECHO_FAKE_OCI: &str = "foo.bar.io/echo:0.2.1";
    const ECHO_FAKE_CACHED: &str = "foo_bar_io_echo_0_2_1";

    let mut echo_cache_path = temp_dir().join("wasmcloud_ocicache").join(ECHO_FAKE_CACHED);
    let _ = ::std::fs::create_dir_all(&echo_cache_path);
    echo_cache_path.set_extension("bin");

    let get_hello_wasm = wash()
        .args([
            "pull",
            ECHO_OCI,
            "--destination",
            echo_cache_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to pull echo for claims sign test");
    assert!(get_hello_wasm.status.success());

    let remote_inspect = wash()
        .args([
            "claims",
            "inspect",
            ECHO_FAKE_OCI,
            "--digest",
            ECHO_SHA,
            "-o",
            "json",
        ])
        .output()
        .expect("failed to inspect remote cached registry");
    assert!(remote_inspect.status.success());
    let remote_inspect_output = get_json_output(remote_inspect).unwrap();
    let expected_inspect_output = json!({
        "account": ECHO_ACC,
        "component": ECHO_MOD,
        "can_be_used": "immediately",
        "expires": "never",
        "version": "0.1.0"
    });

    assert_json_include!(
        actual: remote_inspect_output,
        expected: expected_inspect_output
    );

    let remote_inspect_no_cache = wash()
        .args([
            "claims",
            "inspect",
            ECHO_FAKE_OCI,
            "--digest",
            ECHO_SHA,
            "-o",
            "json",
            "--no-cache",
        ])
        .output()
        .expect("failed to inspect remote cached registry");

    assert!(!remote_inspect_no_cache.status.success());

    remove_file(echo_cache_path).unwrap();
}
