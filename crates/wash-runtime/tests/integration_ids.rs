//! Exercise the public generators in fresh processes, including their first
//! initialization. A seeded embedding application must not seed runtime IDs.

use std::collections::HashSet;
use std::process::Command;

use anyhow::{Context as _, Result};
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
            .find_map(|line| line.strip_prefix("IDS "))
            .context("worker did not return IDs")?;
        let (runtime, host) = line
            .split_once(' ')
            .context("worker returned malformed IDs")?;
        let runtime = uuid::Uuid::parse_str(runtime)?;
        let host = uuid::Uuid::parse_str(host)?;
        // Ignore the timestamp so wall-clock scheduling cannot conceal replay
        // of a process's first counter and random tail.
        assert!(runtime_ids.insert(runtime.as_u128() & ((1_u128 << 80) - 1)));
        assert!(host_ids.insert(host.as_u128() & ((1_u128 << 80) - 1)));
        assert_eq!(runtime.get_version_num(), 7);
        assert_eq!(host.get_version_num(), 7);
    }
    Ok(())
}

#[test]
#[ignore = "subprocess worker for equally_seeded_processes_have_independent_ids"]
fn seeded_process_worker() {
    if std::env::var_os("WASH_ID_TEST_CHILD").is_none() {
        return;
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
    println!("IDS {runtime} {}", host.id());
}
