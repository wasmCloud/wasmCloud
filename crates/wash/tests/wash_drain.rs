mod common;

use common::{get_json_output, test_dir_file, test_dir_with_subfolder, wash};

use std::{
    fs::{remove_dir_all, File},
    io::prelude::*,
};

use assert_json_diff::assert_json_include;
use serde_json::json;

const LIB: &str = "wasmcloudcache";
const OCI: &str = "wasmcloud_ocicache";

#[test]
/// Runs all `drain` integration tests
/// Due to the interaction with the TMPDIR / TMP environment variables,
/// these tests cannot be run concurrently as the interactions with
/// `std::env` can affect other tests
fn integration_drain_comprehensive() {
    integration_drain_lib();
    integration_drain_oci();

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    integration_drain_all();
}

// on linux, cache folder is XDG_CACHE_DIR or $HOME/.cache/smithy
// on mac, cache folder is $HOME/Library/Caches/smithy
// on windows, cache folder is {FOLDERID_LocalAppData}\smithy , for example: C:\Users\Alice\AppData\Local\smithy
// I don't know how to set a temporary folder for testing on windows,
// so we would be clearing the user's real cache folder as a side-effect of tesitng,
// so for windows, we don't test 'drain all' or 'drain smithy' command options

/// Ensures that `wash drain` empties the `wasmcloudcache` directory
fn integration_drain_lib() {
    let test_dir = test_dir_with_subfolder("drain_lib");
    let lib_subdir = &format!("drain_lib/{LIB}");
    let lib_dir = test_dir_with_subfolder(lib_subdir);
    let _nested_dir = test_dir_with_subfolder(&format!("{lib_subdir}/a/b/c/d/e"));

    // Create dummy wasm and parJEEzy files
    let wasm = test_dir_file(lib_subdir, "hello.wasm");
    let mut wasm_file = File::create(wasm).unwrap();
    wasm_file.write_all(b"bytes_or_something_idk").unwrap();
    let provider = test_dir_file(lib_subdir, "world.par.gz");
    let mut provider_file = File::create(provider).unwrap();
    provider_file.write_all(b"parcheesi").unwrap();

    // Set TMPDIR for Unix based systems
    std::env::set_var("TMPDIR", test_dir.clone());
    // Set TMP for Windows based systems
    std::env::set_var("TMP", test_dir.clone());

    let drain_basic = wash()
        .args(["drain", "lib", "-o", "json"])
        .output()
        .unwrap_or_else(|_| panic!("failed to drain {:?}", lib_dir.clone()));
    assert!(drain_basic.status.success());

    let drain_output = get_json_output(drain_basic).unwrap();
    let expected_output = json!({
        "drained": [ lib_dir.to_str().unwrap() ],
    });
    assert_json_include!(actual: drain_output, expected: expected_output);

    // Ensures that the directory is empty (files have been removed)
    assert!(lib_dir.read_dir().unwrap().next().is_none());

    remove_dir_all(test_dir).unwrap();
}

/// Ensures that `wash drain` empties the `wasmcloudcache` directory
fn integration_drain_oci() {
    let test_dir = test_dir_with_subfolder("drain_oci");
    let oci_subdir = &format!("drain_oci/{OCI}");
    let oci_dir = test_dir_with_subfolder(oci_subdir);

    let _nested_dir = test_dir_with_subfolder(&format!("{oci_subdir}/a/b/c/d/e"));

    // Create dummy wasm and parJEEzy files
    let wasm = test_dir_file(oci_subdir, "hello.wasm");
    let mut wasm_file = File::create(wasm).unwrap();
    wasm_file.write_all(b"bytes_or_something_idk").unwrap();
    let provider = test_dir_file(oci_subdir, "world.par.gz");
    let mut provider_file = File::create(provider).unwrap();
    provider_file.write_all(b"parcheesi").unwrap();

    // Set TMPDIR for Unix based systems
    std::env::set_var("TMPDIR", test_dir.clone());
    // Set TMP for Windows based systems
    std::env::set_var("TMP", test_dir.clone());

    let drain_basic = wash()
        .args(["drain", "oci", "-o", "json"])
        .output()
        .unwrap_or_else(|_| panic!("failed to drain {:?}", oci_dir.clone()));
    assert!(drain_basic.status.success());

    let drain_output = get_json_output(drain_basic).unwrap();
    let expected_output = json!({
        "drained": [ oci_dir.to_str().unwrap() ],
    });
    assert_json_include!(actual: drain_output, expected: expected_output);

    // Ensures that the directory is empty (files have been removed)
    assert!(oci_dir.read_dir().unwrap().next().is_none());

    remove_dir_all(test_dir).unwrap();
}

/// Ensures that `wash drain` empties the `wasmcloudcache` directory
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn integration_drain_all() {
    let test_dir = test_dir_with_subfolder("drain_all");
    let oci_subdir = &format!("drain_all/{OCI}");
    let oci_dir = test_dir_with_subfolder(oci_subdir);
    let lib_subdir = &format!("drain_all/{LIB}");
    let lib_dir = test_dir_with_subfolder(lib_subdir);

    let _nested_dir = test_dir_with_subfolder(&format!("{oci_subdir}/a/b/c/d/e"));
    let _nested_dir = test_dir_with_subfolder(&format!("{lib_subdir}/a/b/c/d/e"));

    // Create dummy wasm and parJEEzy files
    let wasm = test_dir_file(oci_subdir, "hello.wasm");
    let mut wasm_file = File::create(wasm).unwrap();
    wasm_file.write_all(b"bytes_or_something_idk").unwrap();
    let provider = test_dir_file(lib_subdir, "world.par.gz");
    let mut provider_file = File::create(provider).unwrap();
    provider_file.write_all(b"parcheesi").unwrap();

    // Set TMPDIR for Unix based systems
    std::env::set_var("TMPDIR", test_dir.clone());
    // Set TMP for Windows based systems
    std::env::set_var("TMP", test_dir.clone());

    let drain_basic = wash()
        .args(["drain", "all", "-o", "json"])
        .output()
        .unwrap_or_else(|_| panic!("failed to drain {:?}", oci_dir.clone()));
    assert!(drain_basic.status.success());

    let drain_output = get_json_output(drain_basic).unwrap();
    let expected_output = json!({
        "drained": [ lib_dir.to_str().unwrap(), oci_dir.to_str().unwrap() ],
    });
    assert_json_include!(actual: drain_output, expected: expected_output);

    // Ensures that the directory is empty (files have been removed)
    assert!(lib_dir.read_dir().unwrap().next().is_none());
    assert!(oci_dir.read_dir().unwrap().next().is_none());

    remove_dir_all(test_dir).unwrap();
}
