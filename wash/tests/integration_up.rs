mod common;
use cmd_lib::{run_fun, spawn_with_output};
use common::test_dir_with_subfolder;
use std::{
    fs::{read_to_string, remove_dir_all},
    process::Command,
};

#[test]
fn integration_up_can_start_wasmcloud_and_actor() -> Result<(), anyhow::Error> {
    let dir = test_dir_with_subfolder("can_start_wasmcloud");
    let wash = env!("CARGO_BIN_EXE_wash");

    let mut proc = spawn_with_output!( $wash up --nats-port 5893 -o json --detached )?;
    let out = proc.wait_with_output()?;
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

    let out = run_fun!( $wash ctl start actor wasmcloud.azurecr.io/echo:0.3.4 --ctl-port 5893 )
        .expect("start echo failed");
    assert!(out.contains("Actor wasmcloud.azurecr.io/echo:0.3.4 started on host N"));

    let kill_cmd = kill_cmd.to_string();
    let (wasmcloud_stop, nats_kill) = kill_cmd.trim_matches('"').split_once(';').unwrap();

    // run_cmd doesn't work as well for commands as strings, so leave these as process::command for now
    let (cmd, arg) = wasmcloud_stop.trim().split_once(' ').unwrap();
    Command::new(cmd).arg(arg).output().unwrap();
    let (cmd, arg) = nats_kill.trim().split_once(' ').unwrap();
    Command::new(cmd).arg(arg).output().unwrap();

    remove_dir_all(dir).unwrap();
    Ok(())
}
