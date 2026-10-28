//! Load for the density experiment.
use super::{Answered, requests};
use anyhow::Result;
use std::{net::SocketAddr, sync::Arc, time::Duration};

/// The CPU a child runs with.
#[derive(Debug, Clone, Copy)]
pub(super) struct CpuLimit {
    pub(super) limit: Option<usize>,
    /// Whether the limit is held by affinity, not just the worker count.
    pub(super) pinned: bool,
    /// What the host may use: the limit, or every core this process may.
    pub(super) cores: usize,
}

/// Pin this thread, and every thread it starts from now on, to the first
/// `cpus` cores it may use. Returns whether it could.
#[cfg(target_os = "linux")]
pub(super) fn pin_to_cpus(cpus: usize) -> Result<bool> {
    use anyhow::{Context as _, bail};
    use rustix::thread::{CpuSet, sched_getaffinity, sched_setaffinity};
    let allowed = sched_getaffinity(None).context("reading this process's CPU affinity")?;
    let mut chosen = CpuSet::new();
    let mut count = 0;
    for cpu in (0..CpuSet::MAX_CPU).filter(|cpu| allowed.is_set(*cpu)) {
        if count == cpus {
            break;
        }
        chosen.set(cpu);
        count += 1;
    }
    if count < cpus {
        bail!("WASH_SCALE_CPUS asks for {cpus} cores, but this process may only use {count}");
    }
    sched_setaffinity(None, &chosen).context("pinning to the CPU limit")?;
    Ok(true)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn pin_to_cpus(_cpus: usize) -> Result<bool> {
    Ok(false)
}

/// The load generator's threads, so their CPU can be told apart from the
/// host's.
#[derive(Default)]
pub(super) struct LoadThreads {
    pub(super) ids: std::sync::Mutex<Vec<u32>>,
    pub(super) started: std::sync::Condvar,
}

impl LoadThreads {
    pub(super) fn register_current(&self) {
        let mut ids = self.ids.lock().unwrap();
        ids.push(current_thread_id());
        self.started.notify_all();
    }

    pub(super) fn wait_for(&self, workers: usize) {
        let ids = self.ids.lock().unwrap();
        drop(
            self.started
                .wait_while(ids, |ids| ids.len() < workers)
                .unwrap(),
        );
    }

    /// CPU the load generator's threads have used, where it can be read.
    #[cfg(target_os = "linux")]
    pub(super) fn cpu(&self) -> Option<Duration> {
        let ids = self.ids.lock().unwrap().clone();
        ids.iter()
            .map(|tid| {
                let raw =
                    std::fs::read_to_string(format!("/proc/self/task/{tid}/schedstat")).ok()?;
                raw.split_whitespace().next()?.parse::<u64>().ok()
            })
            .sum::<Option<u64>>()
            .map(Duration::from_nanos)
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn cpu(&self) -> Option<Duration> {
        None
    }
}

#[cfg(target_os = "linux")]
pub(super) fn current_thread_id() -> u32 {
    rustix::thread::gettid().as_raw_nonzero().get() as u32
}

#[cfg(not(target_os = "linux"))]
pub(super) fn current_thread_id() -> u32 {
    0
}

/// CPU this whole process has used.
#[cfg(unix)]
pub(super) fn process_cpu() -> Option<Duration> {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::ProcessCPUTime);
    Some(Duration::new(
        u64::try_from(now.tv_sec).ok()?,
        u32::try_from(now.tv_nsec).ok()?,
    ))
}

#[cfg(not(unix))]
pub(super) fn process_cpu() -> Option<Duration> {
    None
}

/// CPU the host has used: the process's, less the load generator's where
/// that can be read. The flag says whether it was.
pub(super) fn host_cpu(load: &LoadThreads) -> (Option<Duration>, bool) {
    let process = process_cpu();
    match (process, load.cpu()) {
        (Some(process), Some(load)) => (process.checked_sub(load), true),
        (process, _) => (process, false),
    }
}

/// Sends requests from the load generator's runtime.
#[derive(Clone)]
pub(super) struct Load {
    pub(super) runtime: tokio::runtime::Handle,
    pub(super) threads: Arc<LoadThreads>,
    pub(super) client: reqwest::Client,
}

impl Load {
    pub(super) fn new(runtime: tokio::runtime::Handle, threads: Arc<LoadThreads>) -> Result<Self> {
        Ok(Self {
            runtime,
            threads,
            client: reqwest::Client::builder()
                .pool_max_idle_per_host(256)
                .timeout(Duration::from_secs(30))
                .build()?,
        })
    }

    pub(super) async fn send(
        &self,
        addr: SocketAddr,
        indices: Vec<usize>,
        concurrency: usize,
    ) -> Answered {
        let client = self.client.clone();
        self.runtime
            .spawn(async move { requests(&client, addr, indices.into_iter(), concurrency).await })
            .await
            .expect("the load generator should not panic")
    }
}
