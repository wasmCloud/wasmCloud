mod common;

use common::{get_json_output, output_to_string, test_dir_file, test_dir_with_subfolder, wash};

use std::{
    fs::{remove_dir_all, File},
    io::prelude::*,
};

use assert_json_diff::assert_json_include;
use serde_json::json;

#[test]
fn integration_keys_gen_basic() {
    let keys_gen_account = wash()
        .args(["keys", "gen", "account"])
        .output()
        .expect("failed to generate account key");

    assert!(keys_gen_account.status.success());
}

#[test]
fn integration_keys_gen_comprehensive() {
    let key_gen_types = [
        "account",
        "user",
        "module",
        "component",
        "service",
        "provider",
        "server",
        "host",
        "operator",
        "cluster",
    ];

    for cmd in &key_gen_types {
        let key_gen_command = wash()
            .args(["keys", "gen", cmd])
            .output()
            .unwrap_or_else(|_| panic!("failed to generate key type {cmd} with text output"));
        assert!(key_gen_command.status.success());
        let output = output_to_string(key_gen_command).unwrap();
        assert!(output.contains("Public Key:"));
        assert!(output.contains("Seed:"));
        assert!(output.contains("Remember that the seed is private, treat it as a secret."));
    }

    for cmd in &key_gen_types {
        let key_gen_command = wash()
            .args(["keys", "gen", cmd, "-o", "json"])
            .output()
            .unwrap_or_else(|_| panic!("failed to generate key type {cmd} with json output"));
        assert!(key_gen_command.status.success());
        let output = output_to_string(key_gen_command).unwrap();
        assert!(output.contains("\"public_key\":"));
        assert!(output.contains("\"seed\":"));
    }
}

#[test]
fn integration_keys_get_basic() {
    const KEYCONTENTS: &[u8] = b"SMAAGJ4DY4FNV4VJWA6QU7UQIL7DKJR4Z3UH7NBMNTH22V6VEIJGJUBQN4";
    const KEYNAME: &str = "keys_get_basic.nk";
    const TESTDIR: &str = "integration_get_basic";

    let get_basic_dir = test_dir_with_subfolder(TESTDIR);
    let keyfile = test_dir_file(TESTDIR, KEYNAME);
    let mut file = File::create(keyfile).unwrap();
    file.write_all(KEYCONTENTS).unwrap();

    let key_output = wash()
        .args([
            "keys",
            "get",
            KEYNAME,
            "-d",
            get_basic_dir.to_str().unwrap(),
        ])
        .output()
        .expect("failed to read key with keys get");
    assert!(key_output.status.success());
    assert_eq!(
        output_to_string(key_output).unwrap().trim(),
        String::from_utf8(KEYCONTENTS.to_vec()).unwrap()
    );

    remove_dir_all(get_basic_dir).unwrap();
}

#[test]
fn integration_keys_get_comprehensive() {
    const KEYCONTENTS: &[u8] = b"SMAAGJ4DY4FNV4VJWA6QU7UQIL7DKJR4Z3UH7NBMNTH22V6VEIJGJUBQN4";
    const KEYNAME: &str = "keys_get_comprehensive.nk";
    const TESTDIR: &str = "integration_get_comprehensive";

    let get_comprehensive_dir = test_dir_with_subfolder(TESTDIR);
    let keyfile = test_dir_file(TESTDIR, KEYNAME);
    let mut file = File::create(keyfile).unwrap();
    file.write_all(KEYCONTENTS).unwrap();

    let key_output = wash()
        .args([
            "keys",
            "get",
            KEYNAME,
            "-d",
            get_comprehensive_dir.to_str().unwrap(),
            "-o",
            "json",
        ])
        .output()
        .expect("failed to read key with keys get");
    assert!(key_output.status.success());
    let json_output = get_json_output(key_output).unwrap();
    let expected_output = json!({
        "seed": String::from_utf8(KEYCONTENTS.to_vec()).unwrap()
    });
    assert_json_include!(actual: json_output, expected: expected_output);

    remove_dir_all(get_comprehensive_dir).unwrap();
}

#[test]
fn integration_list_comprehensive() {
    const KEYONE: &str = "listcomprehensive_test_keyone";
    const KEYTWO: &str = "listcomprehensive_test_keytwo";
    const KEYTHREE: &str = "listcomprehensive_test_keythree";
    const KEYONECONTENTS: &[u8] = b"SMAPZS3ZZB5IEVTFKIYVMHTQ7GWTA6K5DC47LRVVWQW2WXRRUISA63Q2DA";
    const KEYTWOCONTENTS: &[u8] = b"SMACCDXTF7C4Q4AV7UU2U7J6PRLDQ4DTSVJNATO53RDIQCIZQ2JSLGYQRI";
    const KEYTHREECONTENTS: &[u8] = b"SMANLG7XYYUWLMZSNHG2I7XWFS67RDRRDV632XCUKD4W6IQEJ33HAG6P74";
    const TESTDIR: &str = "integration_list_comprehensive";

    let list_comprehensive_dir = test_dir_with_subfolder(TESTDIR);
    let keyonefile = test_dir_file(TESTDIR, &format!("{KEYONE}.nk"));
    let keytwofile = test_dir_file(TESTDIR, &format!("{KEYTWO}.nk"));
    let keythreefile = test_dir_file(TESTDIR, &format!("{KEYTHREE}.nk"));

    let mut file = File::create(keyonefile).unwrap();
    file.write_all(KEYONECONTENTS).unwrap();
    let mut file = File::create(keytwofile).unwrap();
    file.write_all(KEYTWOCONTENTS).unwrap();
    let mut file = File::create(keythreefile).unwrap();
    file.write_all(KEYTHREECONTENTS).unwrap();

    let list_output = wash()
        .args([
            "keys",
            "list",
            "-d",
            list_comprehensive_dir.to_str().unwrap(),
        ])
        .output()
        .expect("failed to list keys with keys list");
    assert!(list_output.status.success());
    let output = output_to_string(list_output).unwrap();
    assert!(output.contains(&format!(
        "====== Keys found in {} ======",
        list_comprehensive_dir.to_str().unwrap()
    )));
    assert!(output.contains(KEYONE));
    assert!(output.contains(KEYTWO));
    assert!(output.contains(KEYTHREE));

    let list_output_json = wash()
        .args([
            "keys",
            "list",
            "-d",
            list_comprehensive_dir.to_str().unwrap(),
            "-o",
            "json",
        ])
        .output()
        .expect("failed to list keys with keys list with json output");

    assert!(list_output_json.status.success());
    let output_json = output_to_string(list_output_json).unwrap();
    assert!(output_json.contains(KEYONE));
    assert!(output_json.contains(KEYTWO));
    assert!(output_json.contains(KEYTHREE));

    remove_dir_all(list_comprehensive_dir).unwrap();
}
