//! Shutdown signals for the commands that run until they are told to stop.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context as _;
use tokio::sync::oneshot;

/// What a process leaving on the signal it was given exits with.
const INTERRUPTED: i32 = 130;
const TERMINATED: i32 = 143;

/// The armed shutdown signals.
pub struct Shutdown {
    signalled: oneshot::Receiver<()>,
    ready: Arc<AtomicBool>,
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
    let ready = Arc::new(AtomicBool::new(false));

    tokio::spawn({
        let ready = Arc::clone(&ready);
        async move {
            let code = signals.next().await;
            if !ready.load(Ordering::SeqCst) {
                std::process::exit(code);
            }
            let _ = signalled.send(());
            std::process::exit(signals.next().await);
        }
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
        self.ready.store(true, Ordering::SeqCst);
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
