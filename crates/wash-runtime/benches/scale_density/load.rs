//! Load for the density experiment.
use super::{Answered, host_header};
use anyhow::Result;
use futures::{Stream, StreamExt as _, stream};
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::task::AbortOnDropHandle;

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
        let (runtime, client) = (self.runtime.clone(), self.client.clone());
        joined(
            AbortOnDropHandle::new(self.runtime.spawn(async move {
                requests(runtime, &client, addr, indices, concurrency).await
            }))
            .await,
        )
    }
}

/// Send one GET to each workload index, from outside the host.
async fn requests(
    runtime: tokio::runtime::Handle,
    client: &reqwest::Client,
    addr: SocketAddr,
    indices: Vec<usize>,
    concurrency: usize,
) -> Answered {
    let url = reqwest::Url::parse(&format!("http://{addr}/"))
        .expect("a socket address forms a valid URL");
    let results: Vec<Option<f64>> = spawn_capped(runtime, indices, concurrency, |index| {
        let (client, url) = (client.clone(), url.clone());
        async move {
            let started = Instant::now();
            let response = client
                .get(url)
                .header("HOST", host_header(index))
                .send()
                .await
                .ok()?;
            if !response.status().is_success() {
                return None;
            }
            response.bytes().await.ok()?;
            Some(started.elapsed().as_secs_f64() * 1000.0)
        }
    })
    .collect()
    .await;
    let latencies: Vec<f64> = results.iter().flatten().copied().collect();
    Answered {
        ok: latencies.len(),
        failed: results.len() - latencies.len(),
        latencies,
    }
}

/// Spawn `run` for each item onto `runtime`, at most `concurrency` at once,
/// yielding each output as it finishes. Spawned items may run on any of its
/// workers; one `buffer_unordered` caps them all together. Dropping the
/// stream aborts the tasks still running.
fn spawn_capped<T, F, Fut>(
    runtime: tokio::runtime::Handle,
    items: impl IntoIterator<Item = T>,
    concurrency: usize,
    mut run: F,
) -> impl Stream<Item = Fut::Output>
where
    F: FnMut(T) -> Fut,
    Fut: Future + Send + 'static,
    Fut::Output: Send + 'static,
{
    stream::iter(items)
        .map(move |item| AbortOnDropHandle::new(runtime.spawn(run(item))))
        .buffer_unordered(concurrency.max(1))
        .map(joined)
}

/// A finished task's output, re-raising its own panic rather than a new one.
fn joined<T>(done: Result<T, tokio::task::JoinError>) -> T {
    match done {
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        done => done.expect("a load task should not be cancelled"),
    }
}

#[cfg(test)]
mod tests {
    // Each test holds its own imports and helpers: the harnessless bench's
    // own `cfg(test)` build drops `#[test]` functions, leaving them unused.

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_slow_item_does_not_hold_back_the_others_or_the_limit() {
        use futures::StreamExt as _;
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        use std::time::Duration;

        let (items, concurrency) = (40, 4);
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (done, _) = tokio::sync::watch::channel(0);
        let done = Arc::new(done);
        // Item 0 starts first and waits for every other item to finish.
        let answered: Vec<bool> = super::spawn_capped(
            tokio::runtime::Handle::current(),
            0..items,
            concurrency,
            |item| {
                let (in_flight, peak, done) = (in_flight.clone(), peak.clone(), done.clone());
                async move {
                    let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    let ok = if item == 0 {
                        let mut finished = done.subscribe();
                        let others = finished.wait_for(|n| *n >= items - 1);
                        matches!(
                            tokio::time::timeout(Duration::from_secs(10), others).await,
                            Ok(Ok(_))
                        )
                    } else {
                        true
                    };
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    done.send_modify(|n| *n += 1);
                    ok
                }
            },
        )
        .collect()
        .await;

        assert_eq!(answered.len(), items);
        assert!(
            answered.iter().all(|ok| *ok),
            "item 0 waited out the others"
        );
        let peak = peak.load(Ordering::SeqCst);
        assert!(peak <= concurrency, "{peak} items in flight at once");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn items_run_on_several_workers_at_once() {
        use futures::StreamExt as _;
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        };
        use std::time::{Duration, Instant};

        // Each item blocks its thread until all have started, which only
        // items on separate workers can do.
        let items = 3;
        let arrived = Arc::new(AtomicUsize::new(0));
        let threads = Arc::new(Mutex::new(std::collections::HashSet::new()));
        let met: Vec<bool> =
            super::spawn_capped(tokio::runtime::Handle::current(), 0..items, items, |_| {
                let (arrived, threads) = (arrived.clone(), threads.clone());
                async move {
                    threads.lock().unwrap().insert(std::thread::current().id());
                    arrived.fetch_add(1, Ordering::SeqCst);
                    let deadline = Instant::now() + Duration::from_secs(10);
                    while arrived.load(Ordering::SeqCst) < items && Instant::now() < deadline {
                        std::thread::yield_now();
                    }
                    arrived.load(Ordering::SeqCst) == items
                }
            })
            .collect()
            .await;

        assert!(met.iter().all(|met| *met), "items ran one at a time");
        assert_eq!(threads.lock().unwrap().len(), items);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn dropping_the_stream_aborts_items_in_flight() {
        use futures::StreamExt as _;
        use std::sync::Arc;
        use std::time::Duration;

        struct Dropped(Arc<tokio::sync::watch::Sender<usize>>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.send_modify(|n| *n += 1);
            }
        }

        let concurrency = 4;
        let (started, _) = tokio::sync::watch::channel(0);
        let (dropped, _) = tokio::sync::watch::channel(0);
        let (started, dropped) = (Arc::new(started), Arc::new(dropped));
        let mut stream = Box::pin(super::spawn_capped(
            tokio::runtime::Handle::current(),
            0..10,
            concurrency,
            |_| {
                let (started, guard) = (started.clone(), Dropped(dropped.clone()));
                async move {
                    let _guard = guard;
                    started.send_modify(|n| *n += 1);
                    std::future::pending::<()>().await;
                }
            },
        ));
        let mut all_started = started.subscribe();
        tokio::select! {
            _ = stream.next() => panic!("an item that never finishes finished"),
            _ = all_started.wait_for(|n| *n >= concurrency) => {}
        }
        let mut gone = dropped.subscribe();
        drop(stream);

        let aborted = gone.wait_for(|n| *n >= concurrency);
        tokio::time::timeout(Duration::from_secs(10), aborted)
            .await
            .expect("the in-flight items should be aborted")
            .expect("the sender is held here");
    }

    #[tokio::test]
    async fn items_run_on_the_runtime_they_are_given() {
        use futures::StreamExt as _;

        let given = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("given-runtime")
            .build()
            .unwrap();
        let names: Vec<Option<String>> =
            super::spawn_capped(given.handle().clone(), 0..4, 2, |_| async {
                std::thread::current().name().map(str::to_owned)
            })
            .collect()
            .await;
        given.shutdown_background();

        assert!(
            names
                .iter()
                .all(|name| name.as_deref() == Some("given-runtime")),
            "{names:?}"
        );
    }

    #[tokio::test]
    async fn a_panicking_item_raises_its_own_panic() {
        use futures::{FutureExt as _, StreamExt as _};

        let items = super::spawn_capped(
            tokio::runtime::Handle::current(),
            0..3,
            3,
            |item| async move {
                if item == 1 {
                    panic!("item {item} failed");
                }
            },
        );
        let payload = std::panic::AssertUnwindSafe(items.collect::<Vec<()>>())
            .catch_unwind()
            .await
            .expect_err("the item's panic should reach the caller");

        assert_eq!(
            payload.downcast_ref::<String>().map(String::as_str),
            Some("item 1 failed")
        );
    }
}
