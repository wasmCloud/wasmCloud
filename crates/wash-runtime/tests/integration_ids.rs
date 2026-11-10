//! Exercise the public generators in fresh processes, including their first
//! initialization. A seeded embedding application must not seed runtime IDs.

use std::collections::HashSet;
use std::process::Command;

use anyhow::{Context as _, Result};
use sha2::{Digest as _, Sha256};
use wash_runtime::host::HostBuilder;

#[test]
fn generators_do_not_use_the_applications_prng() {
    fastrand::seed(5924);
    let seed = fastrand::get_seed();
    // Exercise initialization and multiple counter reservations.
    for _ in 0..130 {
        wash_runtime::ids::now_v7();
        assert_eq!(
            fastrand::get_seed(),
            seed,
            "runtime IDs consumed application RNG state"
        );
        HostBuilder::default();
        assert_eq!(
            fastrand::get_seed(),
            seed,
            "host IDs consumed application RNG state"
        );
    }
}

#[test]
fn equally_seeded_processes_have_independent_ids() -> Result<()> {
    let executable = std::env::current_exe()?;
    let mut runtime_ids = HashSet::new();
    let mut host_ids = HashSet::new();
    for _ in 0..4 {
        let output = Command::new(&executable)
            .args([
                "--exact",
                "seeded_process_worker",
                "--ignored",
                "--nocapture",
                "--quiet",
            ])
            .env("WASH_ID_TEST_CHILD", "1")
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "ID worker failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout)?;
        let line = stdout
            .lines()
            .find_map(|line| line.strip_prefix("ID_FINGERPRINTS "))
            .context("worker did not return ID fingerprints")?;
        let (runtime, host) = line
            .split_once(' ')
            .context("worker returned malformed ID fingerprints")?;
        assert!(runtime_ids.insert(runtime.to_owned()));
        assert!(host_ids.insert(host.to_owned()));
    }
    Ok(())
}

#[test]
#[ignore = "subprocess worker for equally_seeded_processes_have_independent_ids"]
fn seeded_process_worker() -> Result<()> {
    if std::env::var_os("WASH_ID_TEST_CHILD").is_none() {
        return Ok(());
    }
    fastrand::seed(5924);
    let seed = fastrand::get_seed();
    let runtime = wash_runtime::ids::now_v7();
    let host = HostBuilder::default();
    assert_eq!(
        fastrand::get_seed(),
        seed,
        "ID initialization used application RNG state"
    );
    println!(
        "ID_FINGERPRINTS {} {}",
        id_fingerprint(runtime),
        id_fingerprint(uuid::Uuid::parse_str(host.id())?)
    );
    Ok(())
}

fn id_fingerprint(id: uuid::Uuid) -> String {
    assert_eq!(id.get_version_num(), 7);
    // Ignore the timestamp so wall-clock scheduling cannot conceal replay
    // of a process's first counter and random tail. Hash the remaining bytes
    // so the subprocess protocol never writes raw IDs to test logs.
    format!("{:x}", Sha256::digest(&id.as_bytes()[6..]))
}
