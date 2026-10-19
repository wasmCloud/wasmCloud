//! Bounded, cancellable setup commands; their output must not block Tokio.
use anyhow::{Context as _, Result};
use std::{
    process::{Child, Command, Output},
    time::Duration,
};

#[cfg(windows)]
mod windows;

/// Own the descendants as well as the immediate child, including after a
/// wrapper exits. Windows children cannot run until assigned to their job.
pub struct ProcessTree {
    #[cfg(unix)]
    group: Option<u32>,
    #[cfg(windows)]
    job: Option<windows::Job>,
}

impl ProcessTree {
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
