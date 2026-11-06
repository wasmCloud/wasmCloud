// One thread per device polls wgpu for every call waiting on that device.

use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Condvar, Mutex},
    task::Poll,
    time::{Duration, Instant},
};

use tokio::sync::oneshot;
use wgpu_core::{device::WaitIdleError, global::Global, id::DeviceId};

use crate::{
    lost::{DeviceLost, DeviceLostInfo, LostWait},
    types::release_device,
    wasi::webgpu::webgpu,
};

/// Longest single wait on a device. A wait holds the device's fence, which
/// `GpuQueue.submit` needs, and is the longest a deadline goes unchecked.
const WAIT_SLICE: Duration = Duration::from_millis(100);

/// How long a lost device's work may keep running before its poller stops
/// waiting for it. wgpu panics when it drops a busy queue, so the poller then
/// leaks the queue and the instance instead.
const ABANDON_AFTER: Duration = Duration::from_secs(60);

type Release = Box<dyn FnOnce(&Global) + Send>;

/// Polls one device while calls wait on it or a release waits for it to go
/// idle, and loses the device when either outlives its timeout.
pub(crate) struct Poller {
    pub(crate) lost: Arc<DeviceLost>,
    timeout: Option<Duration>,
    state: Mutex<PollerState>,
    changed: Condvar,
}

#[derive(Default)]
struct PollerState {
    running: bool,
    abandoned: bool,
    next_waiter: u64,
    waiters: BTreeMap<u64, Option<Instant>>,
    releases: Vec<(Option<Instant>, Release)>,
}

impl Poller {
    /// `None` or zero waits for as long as the work runs.
    pub(crate) fn new(timeout: Option<Duration>) -> Self {
        Self {
            lost: Arc::default(),
            timeout: timeout.filter(|timeout| !timeout.is_zero()),
            state: Mutex::default(),
            changed: Condvar::new(),
        }
    }

    /// Registers a call waiting on `device`. The waiter holds the device so
    /// its id stays valid if the guest drops the device meanwhile.
    pub(crate) fn waiter(
        self: &Arc<Self>,
        instance: &Arc<Global>,
        device: Arc<DeviceId>,
    ) -> Waiter {
        let mut state = self.state.lock().unwrap();
        let id = state.next_waiter;
        state.next_waiter += 1;
        state.waiters.insert(id, self.deadline());
        self.start(&mut state, instance, &device);
        Waiter {
            poller: Arc::clone(self),
            id,
            instance: Arc::clone(instance),
            device: Some(device),
        }
    }

    /// Runs `release` once `device` has no work in flight.
    pub(crate) fn when_idle(
        self: &Arc<Self>,
        instance: &Arc<Global>,
        device: &Arc<DeviceId>,
        release: impl FnOnce(&Global) + Send + 'static,
    ) {
        let mut state = self.state.lock().unwrap();
        if state.abandoned {
            std::mem::forget(Arc::clone(instance));
            return;
        }
        state.releases.push((self.deadline(), Box::new(release)));
        self.start(&mut state, instance, device);
    }

    fn deadline(&self) -> Option<Instant> {
        self.timeout.map(|timeout| Instant::now() + timeout)
    }

    fn start(
        self: &Arc<Self>,
        state: &mut PollerState,
        instance: &Arc<Global>,
        device: &Arc<DeviceId>,
    ) {
        if state.running {
            self.changed.notify_all();
            return;
        }
        state.running = true;
        let poller = Arc::clone(self);
        let instance = Arc::clone(instance);
        let device = Arc::clone(device);
        // Not the tokio blocking pool: a runtime waits for those on shutdown,
        // and this thread lasts as long as the GPU work does.
        std::thread::spawn(move || poller.run(instance, device));
    }

    fn finish(&self, id: u64) {
        if self.state.lock().unwrap().waiters.remove(&id).is_some() {
            self.changed.notify_all();
        }
    }

    fn lose(&self, instance: &Global, device: DeviceId, message: String) {
        self.lost
            .lose(webgpu::GpuDeviceLostReason::Unknown, message);
        instance.device_destroy(device);
    }

    fn run(&self, instance: Arc<Global>, device: Arc<DeviceId>) {
        let mut lost_at = None;
        loop {
            // Fires the callbacks of every finished submission and map.
            let polled = instance.device_poll(
                *device,
                wgpu_types::PollType::Wait {
                    submission_index: None,
                    timeout: Some(WAIT_SLICE),
                },
            );
            let idle = matches!(polled, Ok(wgpu_types::PollStatus::QueueEmpty));
            let failed = match polled {
                Ok(_) | Err(WaitIdleError::Timeout) => false,
                Err(err) => {
                    self.lose(&instance, *device, err.to_string());
                    true
                }
            };
            if idle {
                let releases = std::mem::take(&mut self.state.lock().unwrap().releases);
                for (_, release) in releases {
                    release(&instance);
                }
            }

            let now = Instant::now();
            if self.lost.is_lost() {
                lost_at.get_or_insert(now);
            }
            let mut state = self.state.lock().unwrap();
            if !idle && lost_at.is_some_and(|at| now - at >= ABANDON_AFTER) {
                state.abandoned = true;
                state.releases.clear();
                std::mem::forget(Arc::clone(&instance));
            }
            if state.waiters.is_empty() && state.releases.is_empty() {
                state.running = false;
                break;
            }
            let deadlines = state
                .waiters
                .values()
                .chain(state.releases.iter().map(|(d, _)| d));
            if lost_at.is_none() && deadlines.flatten().any(|deadline| *deadline <= now) {
                drop(state);
                let timeout = self.timeout.unwrap_or_default();
                self.lose(
                    &instance,
                    *device,
                    format!("GPU work did not finish within {timeout:?}"),
                );
                continue;
            }
            // An idle or failing poll returns at once; wait for a change instead.
            if idle || failed {
                drop(self.changed.wait_timeout(state, WAIT_SLICE).unwrap());
            }
        }
        release_device(&instance, device);
    }
}

/// A call waiting on a device. Dropping it stops the wait.
pub(crate) struct Waiter {
    poller: Arc<Poller>,
    id: u64,
    instance: Arc<Global>,
    device: Option<Arc<DeviceId>>,
}

impl Waiter {
    /// For the wgpu callback that ends this wait: it stops the deadline before
    /// the callback's result can be seen as late.
    pub(crate) fn on_done(&self) -> impl FnOnce() + Send + 'static {
        let poller = Arc::clone(&self.poller);
        let id = self.id;
        move || poller.finish(id)
    }

    /// Waits for `result`, or for the device's loss. `Ok(None)` means the
    /// callback was dropped unanswered.
    pub(crate) async fn wait<R>(
        &self,
        mut result: oneshot::Receiver<R>,
    ) -> Result<Option<R>, DeviceLostInfo> {
        let mut lost = LostWait::new(Arc::clone(&self.poller.lost));
        std::future::poll_fn(|cx| {
            if let Poll::Ready(result) = Pin::new(&mut result).poll(cx) {
                return Poll::Ready(Ok(result.ok()));
            }
            lost.poll(cx).map(Err)
        })
        .await
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        self.poller.finish(self.id);
        if let Some(device) = self.device.take() {
            release_device(&self.instance, device);
        }
    }
}
