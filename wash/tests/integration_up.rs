mod common;
use common::{output_to_string, test_dir_with_subfolder, wash};
use std::{
    fs::{read_to_string, remove_dir_all},
    process::Command,
};

#[test]
fn integration_up_can_start_wasmcloud() {
    let dir = test_dir_with_subfolder("can_start_wasmcloud");
    let path = dir.join("washup.log");
    let stdout = std::fs::File::create(&path).expect("could not create log file for wash up test");

    let mut up_cmd = wash()
        .args(&["up", "--nats-port", "5893", "-o", "json", "--detached"])
        .stdout(stdout)
        .spawn()
        .expect("Could not spawn wash up process");

    let status = up_cmd.wait().expect("up command failed to complete");

    assert!(status.success());
    let out = read_to_string(&path).expect("could not read output of wash up");

    let (kill_cmd, wasmcloud_log) = match serde_json::from_str::<serde_json::Value>(&out) {
        Ok(v) => (v["kill_cmd"].to_owned(), v["wasmcloud_log"].to_owned()),
        Err(_e) => panic!("Unable to parse kill cmd from wash up output"),
    };

    // Wait until the host starts
    let mut tries = 30;
    while !read_to_string(wasmcloud_log.to_string().trim_matches('"'))
        .expect("could not read output")
        .contains("Started wasmCloud OTP Host Runtime")
    {
        tries -= 1;
        assert!(tries >= 0);
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    let start_echo = wash()
        .args(&[
            "ctl",
            "start",
            "actor",
            "wasmcloud.azurecr.io/echo:0.3.4",
            "--ctl-port",
            "5893",
        ])
        .output()
        .expect("could not start echo actor on new host");

    assert!(output_to_string(start_echo)
        .expect("could not retrieve output from echo start")
        .contains("Actor wasmcloud.azurecr.io/echo:0.3.4 started on host N"));

    let kill_cmd = kill_cmd.to_string();
    let (wasmcloud_stop, nats_kill) = kill_cmd.trim_matches('"').split_once(';').unwrap();
    let (cmd, arg) = wasmcloud_stop.trim().split_once(' ').unwrap();
    Command::new(cmd).arg(arg).output().unwrap();
    let (cmd, arg) = nats_kill.trim().split_once(' ').unwrap();
    Command::new(cmd).arg(arg).output().unwrap();

    remove_dir_all(dir).unwrap();
}
