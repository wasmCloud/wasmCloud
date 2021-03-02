mod common;
use common::{output_to_string, test_dir_file, test_dir_with_subfolder, wash};
use std::fs::remove_dir_all;

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
