mod common;
use common::{output_to_string, test_dir_file, test_dir_with_subfolder, wash};
use std::fs::{remove_dir_all, File};
use std::io::prelude::*;

#[test]
fn integration_keys_gen_basic() {
    let keys_gen_account = wash()
        .args(&["keys", "gen", "account"])
        .output()
        .expect("failed to generate account key");

    assert!(keys_gen_account.status.success());
}

#[test]
fn integration_keys_gen_comprehensive() {
    let key_gen_types = vec![
        "account", "user", "module", "service", "server", "operator", "cluster",
    ];

    key_gen_types.iter().for_each(|cmd| {
        let key_gen_command = wash()
            .args(&["keys", "gen", cmd])
            .output()
            .unwrap_or_else(|_| panic!("failed to generate key type {} with text output", cmd));
        assert!(key_gen_command.status.success());
        let output = output_to_string(key_gen_command);
        assert!(output.contains("Public Key:"));
        assert!(output.contains("Seed:"));
        assert!(output.contains("Remember that the seed is private, treat it as a secret."));
    });

    key_gen_types.iter().for_each(|cmd| {
        let key_gen_command = wash()
            .args(&["keys", "gen", cmd, "-o", "json"])
            .output()
            .unwrap_or_else(|_| panic!("failed to generate key type {} with json output", cmd));
        assert!(key_gen_command.status.success());
        let output = output_to_string(key_gen_command);
        assert!(output.contains("\"public_key\":"));
        assert!(output.contains("\"seed\":"));
    });
}

#[test]
fn integration_keys_get_basic() {
    const KEYCONTENTS: &[u8] = b"SMAGCRMDVSCIK5TGBAESKJUTWNJKCRRWJK5FQXQZ2POTYWA3JSS63HILFU";
    const KEYNAME: &str = "keys_get_basic.nk";
    const TESTDIR: &str = "integration_get_basic";

    let get_basic_dir = test_dir_with_subfolder(TESTDIR);
    let keyfile = test_dir_file(TESTDIR, KEYNAME);
    let mut file = File::create(keyfile).unwrap();
    file.write_all(KEYCONTENTS).unwrap();

    let key_output = wash()
        .args(&[
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
        output_to_string(key_output).trim(),
        String::from_utf8(KEYCONTENTS.to_vec()).unwrap()
    );

    remove_dir_all(get_basic_dir).unwrap();
}

#[test]
fn integration_keys_get_comprehensive() {
    const KEYCONTENTS: &[u8] = b"SMAGCRMDVSCKDLSJBAESKJUTWNJKCRRWJK5FQXQZ2POTYWA3JSS63HILFU";
    const KEYNAME: &str = "keys_get_comprehensive.nk";
    const TESTDIR: &str = "integration_get_comprehensive";

    let get_comprehensive_dir = test_dir_with_subfolder(TESTDIR);
    let keyfile = test_dir_file(TESTDIR, KEYNAME);
    let mut file = File::create(keyfile).unwrap();
    file.write_all(KEYCONTENTS).unwrap();

    let key_output = wash()
        .args(&[
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
    assert_eq!(
        output_to_string(key_output).trim(),
        format!(
            "{{\"seed\":\"{}\"}}",
            String::from_utf8(KEYCONTENTS.to_vec()).unwrap()
        )
    );

    remove_dir_all(get_comprehensive_dir).unwrap();
}

#[test]
fn integration_list_comprehensive() {
    const KEYONE: &str = "listcomprehensive_test_keyone.nk";
    const KEYTWO: &str = "listcomprehensive_test_keytwo.nk";
    const KEYTHREE: &str = "listcomprehensive_test_keythree.nk";
    const KEYONECONTENTS: &[u8] = b"SMAGCRMDVSCKDLSJBAESKJUTWNJKCRRWJK5FQXQZ2POTYWA3JSS63HILFU";
    const KEYTWOCONTENTS: &[u8] = b"SMAGCRMDVSCKDLSJBAESKJUTWNJKCRRWJK5FQXQZ2POTYWA3JSS63HILFU";
    const KEYTHREECONTENTS: &[u8] = b"SMAGCRMDVSCKDLSJBAESKJUTWNJKCRRWJK5FQXQZ2POTYWA3JSS63HILFU";
    const TESTDIR: &str = "integration_list_comprehensive";

    let list_comprehensive_dir = test_dir_with_subfolder(TESTDIR);
    let keyonefile = test_dir_file(TESTDIR, KEYONE);
    let keytwofile = test_dir_file(TESTDIR, KEYTWO);
    let keythreefile = test_dir_file(TESTDIR, KEYTHREE);

    let mut file = File::create(keyonefile).unwrap();
    file.write_all(KEYONECONTENTS).unwrap();
    let mut file = File::create(keytwofile).unwrap();
    file.write_all(KEYTWOCONTENTS).unwrap();
    let mut file = File::create(keythreefile).unwrap();
    file.write_all(KEYTHREECONTENTS).unwrap();

    let list_output = wash()
        .args(&[
            "keys",
            "list",
            "-d",
            list_comprehensive_dir.to_str().unwrap(),
        ])
        .output()
        .expect("failed to list keys with keys list");
    assert!(list_output.status.success());
    let output = output_to_string(list_output);
    assert!(output.contains(&format!(
        "====== Keys found in {} ======",
        list_comprehensive_dir.to_str().unwrap()
    )));
    assert!(output.contains(KEYONE));
    assert!(output.contains(KEYTWO));
    assert!(output.contains(KEYTHREE));

    let list_output_json = wash()
        .args(&[
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
    let output_json = output_to_string(list_output_json);
    assert!(output_json.contains(KEYONE));
    assert!(output_json.contains(KEYTWO));
    assert!(output_json.contains(KEYTHREE));

    remove_dir_all(list_comprehensive_dir).unwrap();
}
