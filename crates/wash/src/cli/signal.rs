//! Shutdown signals for the commands that run until they are told to stop.

use std::future::Future;

use anyhow::Context as _;
use tokio::sync::oneshot;
use wash_runtime::observability::{FLUSH_BUDGET, flush_within};

/// What a process leaving on the signal it was given exits with.
const INTERRUPTED: i32 = 130;
const TERMINATED: i32 = 143;

/// The armed shutdown signals.
pub struct Shutdown {
    signalled: oneshot::Receiver<()>,
    ready: oneshot::Sender<()>,
}

/// Ends this process, giving the OTel exporters a bounded chance to hand over
/// what they were still batching — the spans and logs that say what the signal
/// interrupted.
///
/// Ending the process is this binary's own decision, so it stays here; how long
/// the exporters get, and why, belongs to the runtime that configured them.
async fn exit(code: i32) -> ! {
    if !flush_within(FLUSH_BUDGET).await {
        eprintln!("observability did not flush within {FLUSH_BUDGET:?}; exiting without it");
    }
    std::process::exit(code)
}

/// Arms the signals asking this process to shut down, before the work they
/// interrupt begins.
///
/// Kubernetes terminates a pod with `SIGTERM` and escalates to `SIGKILL` only
/// once the grace period runs out, so a command listening for `SIGINT` alone is
/// killed outright in a cluster and never reaches its shutdown.
///
/// Until [`Shutdown::ready`], a signal ends the process: a command still
/// starting up has nothing to shut down yet, and one that sat on the signal
/// instead could not be stopped at all while it pulled an image. After it, the
/// signal goes to the future `ready` returns, and a second signal leaves
/// immediately rather than waiting that shutdown out.
pub fn arm() -> anyhow::Result<Shutdown> {
    let mut signals = Signals::arm()?;
    let (signalled, receiver) = oneshot::channel();
    let (ready, mut is_ready) = oneshot::channel();

    tokio::spawn(async move {
        // A flag could only be read once the signal had already arrived,
        // leaving a window in which a `ready` landing alongside it goes unseen
        // and the process ends instead of draining. `biased` settles the tie
        // the same way every time: once there is something to shut down, shut
        // it down.
        tokio::select! {
            biased;
            // `Err` is the `Shutdown` dropped without `ready`: nothing will
            // drain, so a signal keeps ending the process.
            ready = &mut is_ready => if ready.is_err() {
                exit(signals.next().await).await;
            },
            code = signals.next() => exit(code).await,
        }
        // `Signal::recv` is cancel-safe, so a signal that raced `ready` and
        // lost is still pending here and drives the shutdown it asked for.
        signals.next().await;
        let _ = signalled.send(());
        exit(signals.next().await).await;
    });

    Ok(Shutdown {
        signalled: receiver,
        ready,
    })
}

impl Shutdown {
    /// Hands the next signal to the returned future instead of ending the
    /// process. Call it once there is something to shut down; awaiting the
    /// future waits for that signal.
    pub fn ready(self) -> impl Future<Output = ()> + Send {
        let _ = self.ready.send(());
        async move {
            let _ = self.signalled.await;
        }
    }
}

struct Signals {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(windows)]
    interrupt: tokio::signal::windows::CtrlC,
    #[cfg(windows)]
    shutdown: tokio::signal::windows::CtrlShutdown,
    #[cfg(windows)]
    close: tokio::signal::windows::CtrlClose,
}

#[cfg(unix)]
impl Signals {
    fn arm() -> anyhow::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};

        Ok(Self {
            interrupt: signal(SignalKind::interrupt()).context("failed to listen for SIGINT")?,
            terminate: signal(SignalKind::terminate()).context("failed to listen for SIGTERM")?,
        })
    }

    /// Waits for the next signal, answering with the exit code it asks for.
    async fn next(&mut self) -> i32 {
        tokio::select! {
            _ = self.interrupt.recv() => INTERRUPTED,
            _ = self.terminate.recv() => TERMINATED,
        }
    }
}

#[cfg(windows)]
impl Signals {
    /// Ctrl-C, plus the two events a console sends a process it is about to
    /// end: the window closing or the user logging off, and system shutdown.
    fn arm() -> anyhow::Result<Self> {
        use tokio::signal::windows::{ctrl_c, ctrl_close, ctrl_shutdown};

        Ok(Self {
            interrupt: ctrl_c().context("failed to listen for Ctrl-C")?,
            shutdown: ctrl_shutdown().context("failed to listen for the shutdown event")?,
            close: ctrl_close().context("failed to listen for the console close event")?,
        })
    }

    async fn next(&mut self) -> i32 {
        tokio::select! {
            _ = self.interrupt.recv() => INTERRUPTED,
            _ = self.shutdown.recv() => TERMINATED,
            _ = self.close.recv() => TERMINATED,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use anyhow::Result;
    use std::{future::poll_fn, task::Poll};

    #[tokio::test]
    async fn ready_notifies_the_listener_before_its_future_is_polled() -> Result<()> {
        let (signal, signalled) = oneshot::channel();
        let (ready, mut listener) = oneshot::channel();
        let shutdown = Shutdown { signalled, ready };
        let stopped = shutdown.ready();
        listener.try_recv()?;
        tokio::pin!(stopped);
        poll_fn(|cx| {
            assert!(stopped.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        assert!(signal.send(()).is_ok());
        stopped.await;
        Ok(())
    }

    #[tokio::test]
    async fn ready_completes_if_the_signal_listener_has_already_gone_away() {
        let (signal, signalled) = oneshot::channel();
        let (ready, listener) = oneshot::channel();
        drop(signal);
        drop(listener);
        Shutdown { signalled, ready }.ready().await;
    }

    #[test]
    fn dropping_shutdown_or_its_future_closes_the_expected_channels() {
        let (signal, signalled) = oneshot::channel();
        let (ready, mut listener) = oneshot::channel();
        drop(Shutdown { signalled, ready });
        assert_eq!(
            listener.try_recv(),
            Err(oneshot::error::TryRecvError::Closed)
        );
        assert!(signal.send(()).is_err());

        let (signal, signalled) = oneshot::channel();
        let (ready, mut listener) = oneshot::channel();
        drop(Shutdown { signalled, ready }.ready());
        assert_eq!(listener.try_recv(), Ok(()));
        assert!(signal.send(()).is_err());
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use anyhow::ensure;
        use std::{io::Write as _, process::Stdio, time::Duration};
        use tokio::{
            io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, Lines},
            process::{Child, ChildStdin, ChildStdout, Command},
            time::timeout,
        };

        const WAIT: Duration = Duration::from_secs(10);
        const SCENARIO: &str = "WASH_SHUTDOWN_TEST_SCENARIO";

        struct Process {
            child: Child,
            input: ChildStdin,
            output: Lines<BufReader<ChildStdout>>,
        }

        impl Process {
            async fn spawn(scenario: &str) -> Result<Self> {
                let mut child = Command::new(std::env::current_exe()?)
                    .args([
                        "--exact",
                        "cli::signal::tests::unix::signal_child",
                        "--nocapture",
                        "--test-threads=1",
                    ])
                    .env(SCENARIO, scenario)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true)
                    .spawn()?;
                let input = child.stdin.take().context("child stdin")?;
                let output = BufReader::new(child.stdout.take().context("child stdout")?).lines();
                let mut process = Self {
                    child,
                    input,
                    output,
                };
                process.marker("armed").await?;
                Ok(process)
            }

            async fn marker(&mut self, expected: &str) -> Result<()> {
                timeout(WAIT, async {
                    while let Some(line) = self.output.next_line().await? {
                        if let Some(marker) = line.strip_prefix("shutdown-test:") {
                            assert_eq!(marker, expected);
                            return Ok(());
                        }
                    }
                    anyhow::bail!("child closed stdout before {expected}")
                })
                .await?
            }

            async fn command(&mut self, command: &str) -> Result<()> {
                self.input
                    .write_all(format!("{command}\n").as_bytes())
                    .await?;
                self.input.flush().await?;
                Ok(())
            }

            async fn signal(&self, signal: &str) -> Result<()> {
                let pid = self.child.id().context("child has already exited")?;
                let status = timeout(
                    WAIT,
                    Command::new("kill")
                        .arg(format!("-{signal}"))
                        .arg(pid.to_string())
                        .status(),
                )
                .await??;
                ensure!(status.success(), "kill -{signal} failed: {status}");
                Ok(())
            }

            async fn exited(mut self, expected: i32) -> Result<()> {
                let status = timeout(WAIT, self.child.wait()).await??;
                let mut stderr = String::new();
                if let Some(mut pipe) = self.child.stderr.take() {
                    use tokio::io::AsyncReadExt as _;
                    pipe.read_to_string(&mut stderr).await?;
                }
                assert_eq!(
                    status.code(),
                    Some(expected),
                    "child status {status}: {stderr}"
                );
                Ok(())
            }
        }

        fn marker(value: &str) -> Result<()> {
            println!("\nshutdown-test:{value}");
            std::io::stdout().flush()?;
            Ok(())
        }

        fn command(expected: &str) -> Result<()> {
            let mut line = String::new();
            ensure!(
                std::io::stdin().read_line(&mut line)? != 0,
                "parent closed stdin"
            );
            assert_eq!(line.trim(), expected);
            Ok(())
        }

        #[test]
        fn signal_child() -> Result<()> {
            let Ok(scenario) = std::env::var(SCENARIO) else {
                return Ok(());
            };
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let _entered = runtime.enter();
            let shutdown = arm()?;
            marker("armed")?;
            match scenario.as_str() {
                "startup" => runtime.block_on(std::future::pending::<()>()),
                "dropped" => {
                    drop(shutdown);
                    marker("dropped")?;
                    runtime.block_on(std::future::pending::<()>());
                }
                "ready" | "tie" => {
                    // The runtime stays unpolled until both readiness and any queued signal exist.
                    command("ready")?;
                    let stopped = shutdown.ready();
                    marker("ready")?;
                    runtime.block_on(async {
                        stopped.await;
                        marker("draining")?;
                        let mut line = String::new();
                        tokio::io::BufReader::new(tokio::io::stdin())
                            .read_line(&mut line)
                            .await?;
                        ensure!(
                            line.trim() == "finish",
                            "unexpected drain command: {line:?}"
                        );
                        Ok::<_, anyhow::Error>(())
                    })?;
                }
                _ => anyhow::bail!("unknown child scenario: {scenario}"),
            }
            Ok(())
        }

        #[tokio::test]
        async fn startup_signals_exit_with_the_requested_code() -> Result<()> {
            for (signal, code) in [("INT", 130), ("TERM", 143)] {
                let process = Process::spawn("startup").await?;
                process.signal(signal).await?;
                process.exited(code).await?;
            }
            Ok(())
        }

        #[tokio::test]
        async fn dropping_shutdown_preserves_startup_signal_behavior() -> Result<()> {
            for (signal, code) in [("INT", 130), ("TERM", 143)] {
                let mut process = Process::spawn("dropped").await?;
                process.marker("dropped").await?;
                process.signal(signal).await?;
                process.exited(code).await?;
            }
            Ok(())
        }

        #[tokio::test]
        async fn ready_signals_allow_the_graceful_shutdown_to_finish() -> Result<()> {
            for signal in ["INT", "TERM"] {
                let mut process = Process::spawn("ready").await?;
                process.command("ready").await?;
                process.marker("ready").await?;
                process.signal(signal).await?;
                process.marker("draining").await?;
                process.command("finish").await?;
                process.exited(0).await?;
            }
            Ok(())
        }

        #[tokio::test]
        async fn second_signal_exits_during_graceful_shutdown_using_its_own_code() -> Result<()> {
            for first in ["INT", "TERM"] {
                for (second, code) in [("INT", 130), ("TERM", 143)] {
                    let mut process = Process::spawn("ready").await?;
                    process.command("ready").await?;
                    process.marker("ready").await?;
                    process.signal(first).await?;
                    process.marker("draining").await?;
                    process.signal(second).await?;
                    process.exited(code).await?;
                }
            }
            Ok(())
        }

        #[tokio::test]
        async fn readiness_wins_when_a_signal_is_already_pending() -> Result<()> {
            for signal in ["INT", "TERM"].into_iter().cycle().take(16) {
                let mut process = Process::spawn("tie").await?;
                // kill completes before ready is sent, with the signal task still unpolled.
                process.signal(signal).await?;
                process.command("ready").await?;
                process.marker("ready").await?;
                process.marker("draining").await?;
                process.command("finish").await?;
                process.exited(0).await?;
            }
            Ok(())
        }
    }
}
