//! Bounded, cancellable setup commands; their output must not block Tokio.
use anyhow::{Context as _, Result};
use std::{
    process::{Command, Output},
    time::Duration,
};

#[cfg(unix)]
struct ProcessGroup(u32);
#[cfg(unix)]
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        // SAFETY: negative PID addresses only the group created for this child.
        // Keep the guard until wait completes, so cancellation also stops rustc
        // children of cargo rather than leaving an orphan build running.
        #[allow(unsafe_code)]
        unsafe {
            libc::kill(-(self.0 as i32), libc::SIGKILL);
        }
    }
}

pub async fn output(command: Command, budget: Duration) -> Result<Output> {
    let mut command = tokio::process::Command::from(command);
    command
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    let child = command.spawn().context("spawning setup command")?;
    #[cfg(unix)]
    let group = child.id().map(ProcessGroup);
    let result = tokio::time::timeout(budget, child.wait_with_output())
        .await
        .context("setup command timed out")?
        .context("waiting for setup command");
    #[cfg(unix)]
    drop(group);
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[tokio::test]
    async fn a_hung_setup_command_does_not_block_the_runtime() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 60"]);
        let began = std::time::Instant::now();
        assert!(output(command, Duration::from_millis(50)).await.is_err());
        assert!(began.elapsed() < Duration::from_secs(5));
    }
}
