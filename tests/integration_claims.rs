mod common;

use common::{output_to_string, test_dir_file, test_dir_with_subfolder, wash};
use std::env::temp_dir;
use std::fs::{remove_dir_all, remove_file};

#[test]
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
        .args(&[
            "reg",
            "pull",
            "wasmcloud.azurecr.io/echo:0.2.1",
            "--destination",
            echo.to_str().unwrap(),
        ])
        .output()
        .expect("failed to pull echo for claims sign test");
    assert!(get_hello_wasm.status.success());

    let signed_wasm_path = test_dir_file(SUBFOLDER, "echo_signed.wasm");
    let sign_echo = wash()
        .args(&[
            "claims",
            "sign",
            echo.to_str().unwrap(),
            "--name",
            "EchoSigned",
            "--http_server",
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
        output_to_string(sign_echo),
        format!(
            "Successfully signed {} with capabilities: wasmcloud:httpserver\n",
            signed_wasm_path.to_str().unwrap()
        )
    );

    remove_dir_all(sign_dir).unwrap();
}

#[test]
fn integration_claims_inspect() {
    const SUBFOLDER: &str = "claims_inspect";
    const ECHO_OCI: &str = "wasmcloud.azurecr.io/echo:0.2.1";
    const ECHO_ACC: &str = "ACOJJN6WUP4ODD75XEBKKTCCUJJCY5ZKQ56XVKYK4BEJWGVAOOQHZMCW";
    const ECHO_MOD: &str = "MBCFOPM6JW2APJLXJD3Z5O4CN7CPYJ2B4FTKLJUR5YR5MITIU7HD3WD5";
    let inspect_dir = test_dir_with_subfolder(SUBFOLDER);

    // Pull the echo module and push to local registry to test local inspect
    let echo = test_dir_file(SUBFOLDER, "echo.wasm");
    let get_hello_wasm = wash()
        .args(&[
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
        .args(&[
            "reg",
            "push",
            "localhost:5000/echo:claimsinspect",
            echo.to_str().unwrap(),
            "--insecure",
        ])
        .output()
        .expect("failed to push echo.wasm to local registry");
    assert!(push_echo.status.success());

    // Inspect local, local registry, and remote registry actor wasm
    let local_inspect = wash()
        .args(&[
            "claims",
            "inspect",
            echo.to_str().unwrap(),
            "--output",
            "json",
        ])
        .output()
        .expect("failed to inspect local wasm");
    assert!(local_inspect.status.success());
    let local_inspect_output = output_to_string(local_inspect);
    assert!(local_inspect_output.contains(&format!("\"account\":\"{}\"", ECHO_ACC)));
    assert!(local_inspect_output.contains(&format!("\"module\":\"{}\"", ECHO_MOD)));
    assert!(local_inspect_output.contains("\"can_be_used\":\"immediately\""));
    assert!(local_inspect_output.contains("\"capabilities\":[\"HTTP Server\"]"));
    assert!(local_inspect_output.contains("\"expires\":\"never\""));
    assert!(local_inspect_output.contains("\"tags\":\"None\""));
    assert!(local_inspect_output.contains("\"version\":\"0.2.1\""));

    let local_reg_inspect = wash()
        .args(&[
            "claims",
            "inspect",
            "localhost:5000/echo:claimsinspect",
            "--insecure",
            "-o",
            "json",
        ])
        .output()
        .expect("failed to inspect local registry wasm");
    assert!(local_reg_inspect.status.success());
    let local_reg_inspect_output = output_to_string(local_reg_inspect);
    assert!(local_reg_inspect_output.contains(&format!("\"account\":\"{}\"", ECHO_ACC)));
    assert!(local_reg_inspect_output.contains(&format!("\"module\":\"{}\"", ECHO_MOD)));
    assert!(local_reg_inspect_output.contains("\"can_be_used\":\"immediately\""));
    assert!(local_reg_inspect_output.contains("\"capabilities\":[\"HTTP Server\"]"));
    assert!(local_reg_inspect_output.contains("\"expires\":\"never\""));
    assert!(local_reg_inspect_output.contains("\"tags\":\"None\""));
    assert!(local_reg_inspect_output.contains("\"version\":\"0.2.1\""));

    let remote_inspect = wash()
        .args(&[
            "claims",
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
    let remote_inspect_output = output_to_string(remote_inspect);
    assert!(remote_inspect_output.contains(&format!("\"account\":\"{}\"", ECHO_ACC)));
    assert!(remote_inspect_output.contains(&format!("\"module\":\"{}\"", ECHO_MOD)));
    assert!(remote_inspect_output.contains("\"can_be_used\":\"immediately\""));
    assert!(remote_inspect_output.contains("\"capabilities\":[\"HTTP Server\"]"));
    assert!(remote_inspect_output.contains("\"expires\":\"never\""));
    assert!(remote_inspect_output.contains("\"tags\":\"None\""));
    assert!(remote_inspect_output.contains("\"version\":\"0.2.1\""));

    remove_dir_all(inspect_dir).unwrap();
}

#[test]
fn integration_claims_inspect_cached() {
    const ECHO_OCI: &str = "wasmcloud.azurecr.io/echo:0.2.1";
    const ECHO_FAKE_OCI: &str = "foo.bar.io/echo:0.2.1";
    const ECHO_FAKE_CACHED: &str = "foo_bar_io_echo_0_2_1";
    const ECHO_ACC: &str = "ACOJJN6WUP4ODD75XEBKKTCCUJJCY5ZKQ56XVKYK4BEJWGVAOOQHZMCW";
    const ECHO_MOD: &str = "MBCFOPM6JW2APJLXJD3Z5O4CN7CPYJ2B4FTKLJUR5YR5MITIU7HD3WD5";

    let mut echo_cache_path = temp_dir().join("wasmcloud_ocicache").join(ECHO_FAKE_CACHED);
    let _ = ::std::fs::create_dir_all(&echo_cache_path);
    echo_cache_path.set_extension("bin");

    let get_hello_wasm = wash()
        .args(&[
            "reg",
            "pull",
            ECHO_OCI,
            "--destination",
            echo_cache_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to pull echo for claims sign test");
    assert!(get_hello_wasm.status.success());

    let remote_inspect = wash()
        .args(&[
            "claims",
            "inspect",
            ECHO_FAKE_OCI,
            "--digest",
            "sha256:55689502d1bc9c48f22b278c54efeee206a839b8e8eedd4ea6b19e6861f66b3c",
            "-o",
            "json",
        ])
        .output()
        .expect("failed to inspect remote cached registry");
    assert!(remote_inspect.status.success());
    let remote_inspect_output = output_to_string(remote_inspect);
    assert!(remote_inspect_output.contains(&format!("\"account\":\"{}\"", ECHO_ACC)));
    assert!(remote_inspect_output.contains(&format!("\"module\":\"{}\"", ECHO_MOD)));
    assert!(remote_inspect_output.contains("\"can_be_used\":\"immediately\""));
    assert!(remote_inspect_output.contains("\"capabilities\":[\"HTTP Server\"]"));
    assert!(remote_inspect_output.contains("\"expires\":\"never\""));
    assert!(remote_inspect_output.contains("\"tags\":\"None\""));
    assert!(remote_inspect_output.contains("\"version\":\"0.2.1\""));

    let remote_inspect_no_cache = wash()
        .args(&[
            "claims",
            "inspect",
            ECHO_FAKE_OCI,
            "--digest",
            "sha256:55689502d1bc9c48f22b278c54efeee206a839b8e8eedd4ea6b19e6861f66b3c",
            "-o",
            "json",
            "--no-cache",
        ])
        .output()
        .expect("failed to inspect remote cached registry");

    assert!(!remote_inspect_no_cache.status.success());

    remove_file(echo_cache_path).unwrap();
}

#[test]
fn integration_claims_call_alias() {
    const SUBFOLDER: &str = "call_alias";
    let call_alias_dir = test_dir_with_subfolder(SUBFOLDER);
    const ISSUER: &str = "SAADMA65NBETHOHQTXKV7XKQMXYDUS65JOWQORDR3IOMOB3UFZSDOU7TAA";
    const ACC_PKEY: &str = "AALSO6EPE54BWUHXTVJLDIABLYOTXMCOTK52THAIKMKHD32YYWWGQQPW";
    const SUBJECT: &str = "SMAABZ62LGU4SLS4SFK3MD463TRC7ZWMZLYPSVH2AOL3WRZXPBIGZG66JE";
    const MOD_PKEY: &str = "MCLFG44AN6RKNORIDSN5JACURXNEIP5Q6CH2BOG5FCTDF7HE6ES3MCQB";

    // Pull the logger module to test signing
    // During the process of signing a module, the previous "jwt" section
    // is cleared from a signed module, so this is just as effective
    // as signing an unsigned wasm
    let logger = test_dir_file(SUBFOLDER, "logger.wasm");
    let get_wasm = wash()
        .args(&[
            "reg",
            "pull",
            "wasmcloud.azurecr.io/logger:0.1.0",
            "--destination",
            logger.to_str().unwrap(),
        ])
        .output()
        .expect("failed to pull logger for call alias test");
    assert!(get_wasm.status.success());

    let signed_wasm_path = test_dir_file(SUBFOLDER, "logger_signed.wasm");
    let sign_logger = wash()
        .args(&[
            "claims",
            "sign",
            logger.to_str().unwrap(),
            "--name",
            "Logger",
            "-l",
            "-q",
            "--issuer",
            ISSUER,
            "--subject",
            SUBJECT,
            "--disable-keygen",
            "--destination",
            signed_wasm_path.to_str().unwrap(),
            "--call-alias",
            "wasmcloud/logger_onedotzero",
        ])
        .output()
        .expect("failed to sign logger module");
    assert!(sign_logger.status.success());
    assert_eq!(
        output_to_string(sign_logger),
        format!(
            "Successfully signed {} with capabilities: wasmcloud:httpserver,wasmcloud:logging\n",
            signed_wasm_path.to_str().unwrap()
        )
    );

    // inspect actor
    let local_inspect = wash()
        .args(&[
            "claims",
            "inspect",
            signed_wasm_path.to_str().unwrap(),
            "--output",
            "json",
        ])
        .output()
        .expect("failed to inspect local wasm");
    assert!(local_inspect.status.success());
    let local_inspect_output = output_to_string(local_inspect);
    assert!(local_inspect_output.contains(&format!("\"account\":\"{}\"", ACC_PKEY)));
    assert!(local_inspect_output.contains(&format!("\"module\":\"{}\"", MOD_PKEY)));
    assert!(local_inspect_output.contains("\"can_be_used\":\"immediately\""));
    assert!(local_inspect_output.contains("\"capabilities\":["));
    assert!(local_inspect_output.contains("\"HTTP Server\""));
    assert!(local_inspect_output.contains("\"Logging\""));
    assert!(local_inspect_output.contains("\"expires\":\"never\""));
    assert!(local_inspect_output.contains("\"tags\":\"None\""));
    assert!(local_inspect_output.contains("\"version\":\"None\""));
    assert!(local_inspect_output.contains("\"call_alias\":\"wasmcloud/logger_onedotzero\""));

    remove_dir_all(call_alias_dir).unwrap();
}
