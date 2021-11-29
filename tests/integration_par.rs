mod common;

use common::{output_to_string, test_dir_file, test_dir_with_subfolder, wash};
use std::env::temp_dir;
use std::fs::{remove_dir_all, remove_file, File};
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
    const SUBFOLDER: &str = "par_inspect";
    const HTTP_OCI: &str = "wasmcloud.azurecr.io/httpclient:0.3.5";
    const HTTP_ISSUER: &str = "ACOJJN6WUP4ODD75XEBKKTCCUJJCY5ZKQ56XVKYK4BEJWGVAOOQHZMCW";
    const HTTP_SERVICE: &str = "VCCVLH4XWGI3SGARFNYKYT2A32SUYA2KVAIV2U2Q34DQA7WWJPFRKIKM";
    let inspect_dir = test_dir_with_subfolder(SUBFOLDER);

    // Pull the echo module and push to local registry to test local inspect
    let local_http_client_path = test_dir_file(SUBFOLDER, "httpclient.wasm");
    let get_http_client = wash()
        .args(&[
            "reg",
            "pull",
            HTTP_OCI,
            "--destination",
            local_http_client_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to pull https server for par inspect test");
    assert!(get_http_client.status.success());
    let push_echo = wash()
        .args(&[
            "reg",
            "push",
            "localhost:5000/httpclient:parinspect",
            local_http_client_path.to_str().unwrap(),
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
            "par",
            "inspect",
            local_http_client_path.to_str().unwrap(),
            "--output",
            "json",
        ])
        .output()
        .expect("failed to inspect local http server");
    assert!(local_inspect.status.success());
    let local_inspect_output = output_to_string(local_inspect);
    assert!(local_inspect_output.contains(&format!("\"issuer\":\"{}\"", HTTP_ISSUER)));
    assert!(local_inspect_output.contains(&format!("\"service\":\"{}\"", HTTP_SERVICE)));
    assert!(local_inspect_output.contains("\"capability_contract_id\":\"wasmcloud:httpclient\""));

    let local_reg_inspect = wash()
        .args(&[
            "par",
            "inspect",
            "localhost:5000/httpclient:parinspect",
            "--insecure",
            "-o",
            "json",
        ])
        .output()
        .expect("failed to inspect local registry wasm");
    assert!(local_reg_inspect.status.success());
    let local_reg_inspect_output = output_to_string(local_reg_inspect);
    assert!(local_reg_inspect_output.contains(&format!("\"issuer\":\"{}\"", HTTP_ISSUER)));
    assert!(local_reg_inspect_output.contains(&format!("\"service\":\"{}\"", HTTP_SERVICE)));
    assert!(
        local_reg_inspect_output.contains("\"capability_contract_id\":\"wasmcloud:httpclient\"")
    );

    let remote_inspect = wash()
        .args(&["par", "inspect", HTTP_OCI, "-o", "json"])
        .output()
        .expect("failed to inspect local registry wasm");
    assert!(remote_inspect.status.success());
    let remote_inspect_output = output_to_string(remote_inspect);
    assert!(remote_inspect_output.contains(&format!("\"issuer\":\"{}\"", HTTP_ISSUER)));
    assert!(remote_inspect_output.contains(&format!("\"service\":\"{}\"", HTTP_SERVICE)));
    assert!(remote_inspect_output.contains("\"capability_contract_id\":\"wasmcloud:httpclient\""));

    remove_dir_all(inspect_dir).unwrap();
}

#[test]
fn integration_par_inspect_cached() {
    const HTTP_OCI: &str = "wasmcloud.azurecr.io/httpclient:0.3.5";
    const HTTP_FAKE_OCI: &str = "foo.bar.io/httpclient:0.3.5";
    const HTTP_FAKE_CACHED: &str = "foo_bar_io_httpclient_0_3_5";
    const HTTP_ISSUER: &str = "ACOJJN6WUP4ODD75XEBKKTCCUJJCY5ZKQ56XVKYK4BEJWGVAOOQHZMCW";
    const HTTP_SERVICE: &str = "VCCVLH4XWGI3SGARFNYKYT2A32SUYA2KVAIV2U2Q34DQA7WWJPFRKIKM";

    let mut http_client_cache_path = temp_dir().join("wasmcloud_ocicache").join(HTTP_FAKE_CACHED);
    let _ = ::std::fs::create_dir_all(&http_client_cache_path);
    http_client_cache_path.set_extension("bin");

    let get_http_client = wash()
        .args(&[
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
        .args(&["par", "inspect", HTTP_FAKE_OCI, "-o", "json"])
        .output()
        .expect("failed to inspect remote cached registry");
    assert!(remote_inspect.status.success());
    let remote_inspect_output = output_to_string(remote_inspect);
    assert!(remote_inspect_output.contains(&format!("\"issuer\":\"{}\"", HTTP_ISSUER)));
    assert!(remote_inspect_output.contains(&format!("\"service\":\"{}\"", HTTP_SERVICE)));
    assert!(remote_inspect_output.contains("\"capability_contract_id\":\"wasmcloud:httpclient\""));

    let remote_inspect_no_cache = wash()
        .args(&["par", "inspect", HTTP_FAKE_OCI, "-o", "json", "--no-cache"])
        .output()
        .expect("failed to inspect remote cached registry");

    assert!(!remote_inspect_no_cache.status.success());

    remove_file(http_client_cache_path).unwrap();
}
