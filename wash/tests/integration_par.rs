mod common;
use common::{output_to_string, test_dir_file, test_dir_with_subfolder, wash};
use std::fs::{remove_dir_all, File};
use std::io::prelude::*;

#[test]
/// Running create and insert tests together
fn integration_create_and_insert() {
    const SUBFOLDER: &str = "par_create_insert";
    const ISSUER: &str = "SAACTTUPKR55VUWUDK7GJ5SU5KGED455FR7BDO46RUVOTHUWKBLECLH2UU";
    const SUBJECT: &str = "SVAOZUSBWWFL65P255DOHIETPTXUQMM5ETLSYPITI5G4K4HI6M2CDAPWAU";
    let test_dir = test_dir_with_subfolder(SUBFOLDER);
    let pargz = test_dir_file(SUBFOLDER, "test.par.gz");

    integration_par_create(ISSUER, SUBJECT, pargz.to_str().unwrap());
    integration_par_insert(ISSUER, SUBJECT, pargz.to_str().unwrap());

    remove_dir_all(test_dir).unwrap();
}

//TODO: test for issuer too
/// Tests creation of a provider archive file with an initial binary
fn integration_par_create(issuer: &str, subject: &str, archive: &str) {
    const ARCH: &str = "x86_64-linux";
    const SUBFOLDER: &str = "create_bin_folder";
    let bin_folder = test_dir_with_subfolder(SUBFOLDER);
    let binary = test_dir_file(SUBFOLDER, "linux.so");
    let mut bin_file = File::create(binary.clone()).unwrap();
    bin_file.write_all(b"01100010 01110100 01110111").unwrap();

    let create = wash()
        .args(&[
            "par",
            "create",
            "-a",
            ARCH,
            "-b",
            binary.to_str().unwrap(),
            "-c",
            "wasmcloud:testing",
            "-n",
            "Test parJEEzy",
            "-v",
            "TestRunner",
            "--compress",
            "--issuer",
            issuer,
            "--subject",
            subject,
            "--disable-keygen",
            "--version",
            "3.2.1",
            "--revision",
            "42",
            "--destination",
            archive,
        ])
        .output()
        .expect("failed to create provider archive file");
    assert!(create.status.success());
    assert_eq!(
        output_to_string(create),
        format!("Successfully created archive {}\n", archive)
    );

    let inspect_created = wash()
        .args(&["par", "inspect", archive, "-o", "json"])
        .output()
        .expect("failed to inspect created provider archive file");
    assert!(inspect_created.status.success());
    let output = output_to_string(inspect_created);
    assert!(output.contains("\"capability_contract_id\":\"wasmcloud:testing\""));
    assert!(output.contains("\"name\":\"Test parJEEzy\""));
    assert!(
        output.contains("\"service\":\"VBM5JMFOVUJDHGTOJSPUJ33ZGHCRCJ3LYHUJ3HND5ZMRVORYCMAVPZQF\"")
    );
    assert!(
        output.contains("\"issuer\":\"AA7R5L74E45BJ4XVUYTELQ56P5VCOSPOAA474L7QWH4ZAILLKTZFWYYW\"")
    );
    assert!(output.contains("\"rev\":\"42\""));
    assert!(output.contains("\"targets\":[\"x86_64-linux\"]"));
    assert!(output.contains("\"vendor\":\"TestRunner\""));
    assert!(output.contains("\"ver\":\"3.2.1\""));

    remove_dir_all(bin_folder).unwrap();
}

/// Tests inserting multiple binaries into an existing provider archive file
fn integration_par_insert(issuer: &str, subject: &str, archive: &str) {
    const SUBFOLDER: &str = "insert_bin_folder";
    const ARCH1: &str = "mips64-android";
    const ARCH2: &str = "aarch64-ios";

    let insert_dir = test_dir_with_subfolder(SUBFOLDER);

    let bin1 = test_dir_file(SUBFOLDER, "android.so");
    let mut bin1_file = File::create(bin1.clone()).unwrap();
    bin1_file.write_all(b"01101100 01100111").unwrap();

    let bin2 = test_dir_file(SUBFOLDER, "ios.dylib");
    let mut bin2_file = File::create(bin2.clone()).unwrap();
    bin2_file.write_all(b"01101001 01101111 01110011").unwrap();

    let insert_bin1 = wash()
        .args(&[
            "par",
            "insert",
            archive,
            "-a",
            ARCH1,
            "-b",
            bin1.to_str().unwrap(),
            "-i",
            issuer,
            "-s",
            subject,
            "--disable-keygen",
        ])
        .output()
        .expect("failed to insert binary into provider archive");
    assert!(insert_bin1.status.success());
    assert_eq!(
        output_to_string(insert_bin1),
        format!(
            "Successfully inserted {} into archive {}\n",
            bin1.to_str().unwrap(),
            archive
        )
    );
    let inspect_after_bin1 = wash()
        .args(&["par", "inspect", archive, "-o", "json"])
        .output()
        .expect("failed to inspect created provider archive file");
    assert!(inspect_after_bin1.status.success());
    let output = output_to_string(inspect_after_bin1);
    assert!(output.contains("\"capability_contract_id\":\"wasmcloud:testing\""));
    assert!(output.contains("\"name\":\"Test parJEEzy\""));
    assert!(
        output.contains("\"service\":\"VBM5JMFOVUJDHGTOJSPUJ33ZGHCRCJ3LYHUJ3HND5ZMRVORYCMAVPZQF\"")
    );
    assert!(
        output.contains("\"issuer\":\"AA7R5L74E45BJ4XVUYTELQ56P5VCOSPOAA474L7QWH4ZAILLKTZFWYYW\"")
    );
    assert!(output.contains("\"rev\":\"42\""));
    assert!(output.contains("\"targets\":["));
    assert!(output.contains("\"x86_64-linux\""));
    assert!(output.contains("\"mips64-android\""));
    assert!(output.contains("\"vendor\":\"TestRunner\""));
    assert!(output.contains("\"ver\":\"3.2.1\""));

    let insert_bin2 = wash()
        .args(&[
            "par",
            "insert",
            archive,
            "-a",
            ARCH2,
            "-b",
            bin2.to_str().unwrap(),
            "-i",
            issuer,
            "-s",
            subject,
            "--disable-keygen",
        ])
        .output()
        .expect("failed to insert binary into provider archive");
    assert!(insert_bin2.status.success());
    assert_eq!(
        output_to_string(insert_bin2),
        format!(
            "Successfully inserted {} into archive {}\n",
            bin2.to_str().unwrap(),
            archive
        )
    );

    let inspect_after_bin2 = wash()
        .args(&["par", "inspect", archive, "-o", "json"])
        .output()
        .expect("failed to inspect created provider archive file");
    assert!(inspect_after_bin2.status.success());
    let output = output_to_string(inspect_after_bin2);
    assert!(output.contains("\"capability_contract_id\":\"wasmcloud:testing\""));
    assert!(output.contains("\"name\":\"Test parJEEzy\""));
    assert!(
        output.contains("\"service\":\"VBM5JMFOVUJDHGTOJSPUJ33ZGHCRCJ3LYHUJ3HND5ZMRVORYCMAVPZQF\"")
    );
    assert!(
        output.contains("\"issuer\":\"AA7R5L74E45BJ4XVUYTELQ56P5VCOSPOAA474L7QWH4ZAILLKTZFWYYW\"")
    );
    assert!(output.contains("\"rev\":\"42\""));
    assert!(output.contains("\"targets\":["));
    assert!(output.contains("\"x86_64-linux\""));
    assert!(output.contains("\"aarch64-ios\""));
    assert!(output.contains("\"mips64-android\""));
    assert!(output.contains("\"vendor\":\"TestRunner\""));
    assert!(output.contains("\"ver\":\"3.2.1\""));

    remove_dir_all(insert_dir).unwrap();
}

#[test]
fn integration_par_inspect() {
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
    // `String.contains` is used here to ensure we aren't relying on relative json field position.
    // This also allows tests to pass if information is _added_ but not if information is _omitted_
    // from the command output
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
