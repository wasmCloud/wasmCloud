mod common;
use common::wash;

// Unfortunately, launching the REPL will corrupt a terminal session without being able to properly
// clean up the interactive mode. Until this can be fixed, we'll run these in certain situations
//TODO: Investigate possibility of "detatching" terminal _or_ starting a new session just for these tests.

#[test]
#[ignore]
fn integration_up_basic() {
    let up = wash()
        .args(&["up"])
        .output()
        .expect("failed to launch repl");

    assert!(up.status.success());
}

#[test]
#[ignore]
fn integration_up_all_flags() {
    const LOG_LEVEL: &str = "info";
    const RPC_HOST: &str = "0.0.0.0";
    const RPC_PORT: &str = "4222";

    let up = wash()
        .args(&[
            "up",
            "--log-level",
            LOG_LEVEL,
            "--host",
            RPC_HOST,
            "--port",
            RPC_PORT,
        ])
        .output()
        .expect("failed to launch repl");

    assert!(up.status.success());
}
