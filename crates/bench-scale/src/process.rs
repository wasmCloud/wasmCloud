//! Bounded, cancellable setup commands; their output must not block Tokio.
use anyhow::{Context as _, Result};
use std::{
    process::{Child, Command, Output},
    time::Duration,
};

#[cfg(windows)]
mod windows;

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
const MAX_PROCESS_MEMBERS: usize = 4096;

/// Own the descendants as well as the immediate child, including after a
/// wrapper exits. Windows children cannot run until assigned to their job.
pub struct ProcessTree {
    #[cfg(unix)]
    group: Option<u32>,
    #[cfg(windows)]
    job: Option<windows::Job>,
}

impl ProcessTree {
    pub(crate) fn members(&self) -> Option<Vec<u32>> {
        #[cfg(target_os = "linux")]
        {
            let group = i32::try_from(self.group?).ok()?;
            let mut members = Vec::new();
            // Enumerate IDs directly: a metadata refresh may omit an unreadable
            // live child and turn its missing footprint into an underestimate.
            for entry in std::fs::read_dir("/proc").ok()? {
                let name = entry.ok()?.file_name();
                let Ok(pid) = name.to_str()?.parse::<i32>() else {
                    continue;
                };
                // SAFETY: getpgid only queries metadata; the owned group also
                // identifies grandchildren reparented after a wrapper exits.
                #[allow(unsafe_code)]
                let actual_group = unsafe { libc::getpgid(pid) };
                if actual_group == -1 {
                    if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                        continue; // It exited after directory enumeration.
                    }
                    return None;
                }
                if actual_group == group {
                    if members.len() == MAX_PROCESS_MEMBERS {
                        return None;
                    }
                    members.push(u32::try_from(pid).ok()?);
                }
            }
            Some(members)
        }
        #[cfg(target_os = "macos")]
        {
            // PROC_PGRP_ONLY from Apple's sys/proc_info.h; libc exposes the
            // function but not this selector. An extra slot detects truncation.
            const PROC_PGRP_ONLY: u32 = 2;
            let mut members: [libc::pid_t; MAX_PROCESS_MEMBERS + 1] = [0; MAX_PROCESS_MEMBERS + 1];
            let buffer_bytes = std::mem::size_of_val(&members);
            // SAFETY: members is writable for exactly buffer_bytes and the
            // selected query returns an array of process IDs, measured in bytes.
            #[allow(unsafe_code)]
            let bytes = unsafe {
                libc::proc_listpids(
                    PROC_PGRP_ONLY,
                    self.group?,
                    members.as_mut_ptr().cast(),
                    i32::try_from(buffer_bytes).ok()?,
                )
            };
            let bytes = usize::try_from(bytes).ok()?;
            if bytes == 0
                || bytes >= buffer_bytes
                || !bytes.is_multiple_of(size_of::<libc::pid_t>())
            {
                return None;
            }
            members
                .get(..bytes / size_of::<libc::pid_t>())?
                .iter()
                .map(|pid| u32::try_from(*pid).ok())
                .collect()
        }
        #[cfg(windows)]
        {
            self.job.as_ref()?.members()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            None
        }
    }

    fn prepare(command: &mut Command) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        #[cfg(windows)]
        let job = windows::Job::prepare(command)?;
        Ok(Self {
            #[cfg(unix)]
            group: None,
            #[cfg(windows)]
            job: Some(job),
        })
    }

    fn attach(&mut self, child: &Child) -> Result<()> {
        #[cfg(unix)]
        {
            self.group = Some(child.id());
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle as _;
            self.job
                .as_ref()
                .context("process job was already closed")?
                .assign_and_resume(child.id(), child.as_raw_handle())?;
        }
        Ok(())
    }

    fn attach_async(&mut self, child: &tokio::process::Child) -> Result<()> {
        #[cfg(unix)]
        {
            self.group = child.id();
        }
        #[cfg(windows)]
        self.job
            .as_ref()
            .context("process job was already closed")?
            .assign_and_resume(
                child.id().context("new child has no process id")?,
                child
                    .raw_handle()
                    .context("new child has no process handle")?,
            )?;
        Ok(())
    }

    pub fn kill(&mut self) {
        #[cfg(unix)]
        if let Some(group) = self.group.take() {
            // SAFETY: this group was created for our child before exec. A negative
            // PID targets its descendants even when the immediate child has exited.
            #[allow(unsafe_code)]
            unsafe {
                libc::kill(-(group as i32), libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        drop(self.job.take());
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        self.kill();
    }
}

pub fn spawn(command: &mut Command) -> Result<(Child, ProcessTree)> {
    let mut tree = ProcessTree::prepare(command)?;
    let mut child = command.spawn().context("spawning child process")?;
    if let Err(error) = tree.attach(&child) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    Ok((child, tree))
}

pub async fn output(command: Command, budget: Duration) -> Result<Output> {
    let mut command = command;
    let mut tree = ProcessTree::prepare(&mut command)?;
    let mut command = tokio::process::Command::from(command);
    command
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().context("spawning setup command")?;
    if let Err(error) = tree.attach_async(&child) {
        let _ = child.kill().await;
        return Err(error);
    }
    let result = tokio::time::timeout(budget, child.wait_with_output())
        .await
        .context("setup command timed out")?
        .context("waiting for setup command");
    drop(tree);
    result
}

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt as _;

    fn tree_command(role: &str, addr: std::net::SocketAddr) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "process::tests::process_tree_helper",
                "--nocapture",
            ])
            .env("BENCH_SCALE_TEST_PROCESS_ROLE", role)
            .env("BENCH_SCALE_TEST_PROCESS_ADDR", addr.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        command
    }

    // Run in a subprocess by the tests below, never by the normal test runner.
    #[test]
    #[ignore = "subprocess fixture for process-tree cleanup tests"]
    #[allow(clippy::zombie_processes)] // The wrapper intentionally leaves its child for the guard.
    fn process_tree_helper() {
        use std::io::{Read as _, Write as _};
        let Ok(role) = std::env::var("BENCH_SCALE_TEST_PROCESS_ROLE") else {
            return;
        };
        let addr = std::env::var("BENCH_SCALE_TEST_PROCESS_ADDR")
            .unwrap()
            .parse()
            .unwrap();
        if role == "leaf" {
            let mut socket = std::net::TcpStream::connect(addr).unwrap();
            socket.write_all(b"R").unwrap();
            // EOF also releases the helper if its parent test fails.
            let _ = socket.read_exact(&mut [0]);
        } else {
            let mut child = tree_command("leaf", addr).spawn().unwrap();
            if role == "parent" {
                let _ = child.wait();
            }
            // A wrapper deliberately exits while its child is still serving.
        }
    }

    async fn assert_leaf_closed(mut socket: tokio::net::TcpStream) {
        let result = tokio::time::timeout(Duration::from_secs(10), socket.read_u8())
            .await
            .expect("the grandchild must stop with its process tree");
        assert!(result.is_err(), "expected EOF or connection reset");
    }

    #[tokio::test]
    async fn cancelling_setup_stops_its_grandchild() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let command = tree_command("parent", listener.local_addr().unwrap());
        let mut work = Box::pin(output(command, Duration::from_secs(60)));
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::select! {
                result = &mut work => panic!("setup exited before cancellation: {result:?}"),
                connected = listener.accept() => connected.unwrap(),
            }
        })
        .await
        .unwrap();
        assert_eq!(socket.read_u8().await.unwrap(), b'R');
        drop(work);
        assert_leaf_closed(socket).await;
    }

    #[tokio::test]
    async fn a_host_wrapper_cannot_leave_a_child_after_exiting() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut command = tree_command("wrapper", listener.local_addr().unwrap());
        let mut guard = crate::host::ChildGuard::spawn(&mut command).unwrap();
        let (mut socket, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(socket.read_u8().await.unwrap(), b'R');
        tokio::time::timeout(Duration::from_secs(10), async {
            while guard.exited().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(guard);
        assert_leaf_closed(socket).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_hung_setup_command_does_not_block_the_runtime() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 60"]);
        let began = std::time::Instant::now();
        assert!(output(command, Duration::from_millis(50)).await.is_err());
        assert!(began.elapsed() < Duration::from_secs(5));
    }
}
