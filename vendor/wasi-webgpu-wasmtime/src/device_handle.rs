//! Shared ownership of a device and its queue, and the thread that polls them.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use wgpu_core::{
    device::{queue::SubmittedWorkDoneClosure, WaitIdleError},
    global::Global,
    id::{BufferId, DeviceId, QueueId},
    resource::{BufferAccessError, BufferMapOperation},
    SubmissionIndex,
};

use crate::{
    lost::{DeviceLost, DeviceLostInfo},
    wasi::webgpu::webgpu,
};

/// Longest one poll waits. A waiting poll holds wgpu-core's fence lock, which
/// `queue_submit` needs, so a long wait would stall the guest's submits.
const WAIT_SLICE: Duration = Duration::from_millis(10);

/// How long a lost device's work may keep running after its last holder
/// drops. wgpu panics when it drops a busy queue, so the queue and the
/// instance are leaked instead once this passes.
const ABANDON_AFTER: Duration = Duration::from_secs(60);

/// A device and its queue. They leave the hub when the last holder drops.
///
/// The guest's device, its queues and buffers, and a running poll each hold
/// one, so ids stay valid for a call that is still waiting.
pub(crate) struct DeviceHandle {
    instance: Arc<Global>,
    device: DeviceId,
    queue: QueueId,
    /// Longest submitted work may run before the device is lost.
    timeout: Option<Duration>,
    lost: Arc<DeviceLost>,
    poller: Mutex<Poller>,
}

#[derive(Default)]
struct Poller {
    running: bool,
    /// Counts poll starts, so a poll only retires waiters registered before it.
    generation: u64,
    waiting: Vec<Waiter>,
}

impl Poller {
    fn start_poll(&mut self) -> (SubmissionIndex, u64) {
        let started = self.generation;
        self.generation += 1;
        let target = self
            .waiting
            .iter()
            .map(|waiter| waiter.index)
            .min()
            .unwrap_or(0);
        (target, started)
    }

    fn finish_poll(&mut self, target: SubmissionIndex, started: u64, finished: bool) -> bool {
        self.waiting.retain(|waiter| {
            let retired = finished && waiter.index <= target && waiter.generation <= started;
            !retired && !waiter.pending.is_settled()
        });
        let now = Instant::now();
        // Later submissions and newly registered calls need their own poll.
        self.waiting.iter().any(|waiter| {
            waiter.index <= target
                && waiter.generation <= started
                && waiter.deadline.is_some_and(|deadline| deadline <= now)
        })
    }
}

struct Waiter {
    index: SubmissionIndex,
    generation: u64,
    deadline: Option<Instant>,
    pending: Box<dyn Pending>,
}

/// Something the poller waits on: a call's wgpu callback, or submitted work.
trait Pending: Send {
    /// Whether wgpu has run the callback.
    fn is_settled(&self) -> bool;

    /// Settles the call with the device's loss instead.
    fn abort(self: Box<Self>, info: &DeviceLostInfo);
}

impl DeviceHandle {
    /// `timeout` bounds how long submitted work may run; `None` or zero
    /// lets it run for as long as it takes.
    pub(crate) fn new(
        instance: Arc<Global>,
        device: DeviceId,
        queue: QueueId,
        timeout: Option<Duration>,
    ) -> Self {
        let lost = Arc::<DeviceLost>::default();
        let reported = Arc::clone(&lost);
        instance.device_set_device_lost_closure(
            device,
            Box::new(move |reason, message| reported.lose(reason.into(), message)),
        );
        Self {
            instance,
            device,
            queue,
            timeout: timeout.filter(|timeout| !timeout.is_zero()),
            lost,
            poller: Mutex::default(),
        }
    }

    pub(crate) fn device_id(&self) -> &DeviceId {
        &self.device
    }

    pub(crate) fn queue_id(&self) -> &QueueId {
        &self.queue
    }

    pub(crate) fn lost(&self) -> &Arc<DeviceLost> {
        &self.lost
    }

    /// wgpu reports a destroyed device lost only once its queue drains, and
    /// only to a poll; the spec loses it at once.
    /// https://www.w3.org/TR/webgpu/#dom-gpudevice-destroy
    pub(crate) fn destroy(&self) {
        self.lose(
            webgpu::GpuDeviceLostReason::Destroyed,
            "device destroyed".into(),
        );
    }

    /// Loses the device and settles every call waiting on it. The device
    /// ignores later submissions, and its work keeps the GPU busy until it
    /// ends or the driver resets it.
    fn lose(&self, reason: webgpu::GpuDeviceLostReason, message: String) {
        lose(&self.instance, self.device, &self.lost, reason, message);
        self.abort_waiting();
    }

    /// Bounds the work up to `index` by the timeout, even if no call waits
    /// on it. Without a timeout there is nothing to enforce.
    pub(crate) fn watch(self: &Arc<Self>, index: SubmissionIndex) {
        if self.timeout.is_some() {
            self.poll_until(index, Box::new(Watch));
        }
    }

    /// Resolves once all work submitted so far, including queued writes, is
    /// done, or once the device is lost.
    /// https://www.w3.org/TR/webgpu/#lose-the-device
    pub(crate) fn on_submitted_work_done(self: &Arc<Self>) -> oneshot::Receiver<()> {
        // wgpu-core leaves queued writes out of work-done callbacks, so submit them.
        if let Err((_, err)) = self.instance.queue_submit(self.queue, &[]) {
            if !self.lost.is_lost() {
                log::warn!("failed to flush queued writes: {err}");
            }
        }
        let (settle, done) = Settle::new();
        let on_done = settle.clone();
        let callback: SubmittedWorkDoneClosure = Box::new(move || on_done.send(()));
        let index = self
            .instance
            .queue_on_submitted_work_done(self.queue, callback);
        self.poll_until(index, Box::new(settle));
        done
    }

    /// Maps `buffer`, resolving with the result once the mapping is done.
    /// `state` follows the map even if the caller stops waiting for it.
    pub(crate) fn map_async(
        self: &Arc<Self>,
        buffer: BufferId,
        host: wgpu_core::device::HostMap,
        offset: u64,
        size: Option<u64>,
        state: Arc<Mutex<webgpu::GpuBufferMapState>>,
    ) -> oneshot::Receiver<Result<(), webgpu::MapAsyncError>> {
        let (settle, mapped) = Settle::new();
        let settle = MapSettle { settle, state };
        let on_mapped = settle.clone();
        let op = BufferMapOperation {
            host,
            callback: Some(Box::new(move |result: Result<(), BufferAccessError>| {
                on_mapped.send(result.map_err(|err| map_error(&err)));
            })),
        };
        // On error, wgpu-core has already called the callback with it.
        if let Ok(index) = self.instance.buffer_map_async(buffer, offset, size, op) {
            self.poll_until(index, Box::new(settle));
        }
        mapped
    }

    /// Polls this device on a background thread until `index` completes,
    /// which runs the wgpu callbacks registered on it. `pending` is aborted
    /// instead if the device is lost, which happens if `index` outlives the
    /// timeout. One thread per device serves every waiting call.
    fn poll_until(self: &Arc<Self>, index: SubmissionIndex, pending: Box<dyn Pending>) {
        if let Some(info) = self.lost.info() {
            pending.abort(&info);
            return;
        }
        {
            let mut poller = self.lock_poller();
            let generation = poller.generation;
            poller.waiting.push(Waiter {
                index,
                generation,
                deadline: self.timeout.map(|timeout| Instant::now() + timeout),
                pending,
            });
            if poller.running {
                return;
            }
            poller.running = true;
        }
        let this = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("wasi-webgpu-poll".into())
            .spawn(move || this.run_poller());
        if let Err(err) = spawned {
            self.lose(
                webgpu::GpuDeviceLostReason::Unknown,
                format!("failed to spawn the device poll thread: {err}"),
            );
        }
    }

    fn run_poller(self: Arc<Self>) {
        loop {
            let (target, started) = self.lock_poller().start_poll();
            // Index 0 means nothing is in flight, and any poll fires the callback.
            let poll_type = match target {
                0 => wgpu_types::PollType::Poll,
                index => wgpu_types::PollType::Wait {
                    submission_index: Some(index),
                    timeout: Some(WAIT_SLICE),
                },
            };
            let finished = match self.instance.device_poll(self.device, poll_type) {
                Ok(_) => true,
                Err(WaitIdleError::Timeout) => false,
                Err(err) => {
                    self.lose(
                        webgpu::GpuDeviceLostReason::Unknown,
                        format!("device poll failed: {err}"),
                    );
                    return;
                }
            };
            // A loss wgpu reported during the poll, or a `destroy` meanwhile.
            if self.lost.is_lost() {
                self.abort_waiting();
                return;
            }
            let mut poller = self.lock_poller();
            let late = poller.finish_poll(target, started, finished);
            if poller.waiting.is_empty() {
                poller.running = false;
                return;
            }
            drop(poller);
            if late {
                self.lose(
                    webgpu::GpuDeviceLostReason::Unknown,
                    timed_out(self.timeout.unwrap_or_default()),
                );
                return;
            }
        }
    }

    /// Settles every waiting call with the device's loss.
    fn abort_waiting(&self) {
        let Some(info) = self.lost.info() else {
            return;
        };
        let waiting = {
            let mut poller = self.lock_poller();
            poller.running = false;
            std::mem::take(&mut poller.waiting)
        };
        for waiter in waiting {
            waiter.pending.abort(&info);
        }
    }

    fn lock_poller(&self) -> std::sync::MutexGuard<'_, Poller> {
        self.poller.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Loses `device` once; a later loss keeps the first reason.
fn lose(
    instance: &Global,
    device: DeviceId,
    lost: &DeviceLost,
    reason: webgpu::GpuDeviceLostReason,
    message: String,
) {
    if lost.is_lost() {
        return;
    }
    lost.lose(reason, message);
    instance.device_destroy(device);
}

fn timed_out(timeout: Duration) -> String {
    format!("GPU work did not finish within {timeout:?}")
}

impl Drop for DeviceHandle {
    /// wgpu blocks while it drops a queue with work in flight and panics if
    /// the work outlasts about 6s, so a busy queue is dropped from a thread
    /// once its device is idle.
    fn drop(&mut self) {
        let idle = self
            .instance
            .device_poll(self.device, wgpu_types::PollType::Poll);
        if let Ok(wgpu_types::PollStatus::QueueEmpty) = idle {
            self.instance.queue_drop(self.queue);
            self.instance.device_drop(self.device);
            return;
        }
        let release = Release {
            instance: Arc::clone(&self.instance),
            device: self.device,
            queue: self.queue,
            timeout: self.timeout,
            lost: Arc::clone(&self.lost),
        };
        let spawned = std::thread::Builder::new()
            .name("wasi-webgpu-release".into())
            .spawn(move || release.run());
        if let Err(err) = spawned {
            log::error!("failed to spawn the queue release thread, leaking the queue: {err}");
            // Dropping the instance would drop the busy queue.
            std::mem::forget(Arc::clone(&self.instance));
        }
    }
}

/// A busy queue and its device, dropped once the device is idle. It holds the
/// instance, which drops every queue it still has when it drops.
struct Release {
    instance: Arc<Global>,
    device: DeviceId,
    queue: QueueId,
    timeout: Option<Duration>,
    lost: Arc<DeviceLost>,
}

impl Release {
    /// Returns whether the queue was dropped; `false` means it was leaked.
    fn run(self) -> bool {
        let started = Instant::now();
        let mut lost_at = None;
        loop {
            let polled = self.instance.device_poll(
                self.device,
                wgpu_types::PollType::Wait {
                    submission_index: None,
                    timeout: Some(WAIT_SLICE),
                },
            );
            match polled {
                Ok(wgpu_types::PollStatus::QueueEmpty) => {
                    self.instance.queue_drop(self.queue);
                    self.instance.device_drop(self.device);
                    return true;
                }
                Ok(_) | Err(WaitIdleError::Timeout) => {}
                Err(err) => {
                    self.lose(format!("device poll failed: {err}"));
                    std::thread::sleep(WAIT_SLICE);
                }
            }
            let now = Instant::now();
            if self.lost.is_lost() {
                let lost_at = *lost_at.get_or_insert(now);
                if now - lost_at >= ABANDON_AFTER {
                    log::error!(
                        "GPU work still running {ABANDON_AFTER:?} after its device was lost, leaking the queue"
                    );
                    std::mem::forget(self.instance);
                    return false;
                }
            } else if let Some(timeout) = self.timeout.filter(|timeout| now - started >= *timeout) {
                self.lose(timed_out(timeout));
            }
        }
    }

    fn lose(&self, message: String) {
        lose(
            &self.instance,
            self.device,
            &self.lost,
            webgpu::GpuDeviceLostReason::Unknown,
            message,
        );
    }
}

/// Submitted work the poller bounds by the timeout, with no call waiting on
/// it. It settles when its index completes.
struct Watch;

impl Pending for Watch {
    fn is_settled(&self) -> bool {
        false
    }

    fn abort(self: Box<Self>, _info: &DeviceLostInfo) {}
}

/// Sends one value from whichever clone calls `send` first.
struct Settle<T>(Arc<Mutex<Option<oneshot::Sender<T>>>>);

impl<T> Clone for Settle<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Settle<T> {
    fn new() -> (Self, oneshot::Receiver<T>) {
        let (send, recv) = oneshot::channel();
        (Self(Arc::new(Mutex::new(Some(send)))), recv)
    }

    fn send(&self, value: T) {
        self.send_with(value, |_| {});
    }

    fn is_settled(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none()
    }

    /// Like `send`, but runs `first` before sending, and only if this is the
    /// first value.
    fn send_with(&self, mut value: T, first: impl FnOnce(&mut T)) {
        let send = self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(send) = send {
            first(&mut value);
            let _ = send.send(value);
        }
    }
}

/// Losing the device completes outstanding work-done waits.
impl Pending for Settle<()> {
    fn is_settled(&self) -> bool {
        Settle::is_settled(self)
    }

    fn abort(self: Box<Self>, _info: &DeviceLostInfo) {
        self.send(());
    }
}

/// Settles a map and sets the buffer's map state to match, before the waiting
/// call can see the result.
#[derive(Clone)]
struct MapSettle {
    settle: Settle<Result<(), webgpu::MapAsyncError>>,
    state: Arc<Mutex<webgpu::GpuBufferMapState>>,
}

impl MapSettle {
    fn send(&self, mapped: Result<(), webgpu::MapAsyncError>) {
        self.settle.send_with(mapped, |mapped| {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if *state != webgpu::GpuBufferMapState::Pending {
                *mapped = Err(aborted("buffer was unmapped before its map completed"));
                return;
            }
            *state = match mapped {
                Ok(()) => webgpu::GpuBufferMapState::Mapped,
                Err(_) => webgpu::GpuBufferMapState::Unmapped,
            };
        });
    }
}

impl Pending for MapSettle {
    fn is_settled(&self) -> bool {
        self.settle.is_settled()
    }

    fn abort(self: Box<Self>, info: &DeviceLostInfo) {
        self.send(Err(lost_error(info)));
    }
}

fn map_error(err: &BufferAccessError) -> webgpu::MapAsyncError {
    // Unmapping or destroying the buffer first aborts the map.
    // https://www.w3.org/TR/webgpu/#abort-a-pending-map
    let kind = match err {
        BufferAccessError::MapAborted => webgpu::MapAsyncErrorKind::AbortError,
        _ => webgpu::MapAsyncErrorKind::OperationError,
    };
    webgpu::MapAsyncError {
        kind,
        message: err.to_string(),
    }
}

/// Destroying the device unmaps its buffers, which aborts their maps.
/// https://www.w3.org/TR/webgpu/#dom-gpudevice-destroy
fn lost_error(info: &DeviceLostInfo) -> webgpu::MapAsyncError {
    let kind = match info.reason {
        webgpu::GpuDeviceLostReason::Destroyed => webgpu::MapAsyncErrorKind::AbortError,
        webgpu::GpuDeviceLostReason::Unknown => webgpu::MapAsyncErrorKind::OperationError,
    };
    webgpu::MapAsyncError {
        kind,
        message: info.message.clone(),
    }
}

pub(crate) fn aborted(message: &str) -> webgpu::MapAsyncError {
    webgpu::MapAsyncError {
        kind: webgpu::MapAsyncErrorKind::AbortError,
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_secs(10);

    fn noop_device() -> Arc<DeviceHandle> {
        noop_device_with(None)
    }

    fn noop_device_with(timeout: Option<Duration>) -> Arc<DeviceHandle> {
        let instance = Arc::new(Global::new(
            "test",
            wgpu_types::InstanceDescriptor {
                backends: wgpu_types::Backends::NOOP,
                backend_options: wgpu_types::BackendOptions {
                    noop: wgpu_types::NoopBackendOptions { enable: true },
                    ..Default::default()
                },
                ..wgpu_types::InstanceDescriptor::new_without_display_handle()
            },
            None,
        ));
        let adapter = instance
            .request_adapter(
                &wgpu_types::RequestAdapterOptions::default(),
                wgpu_types::Backends::NOOP,
                None,
            )
            .unwrap();
        let (device, queue) = instance
            .adapter_request_device(
                adapter,
                &wgpu_types::DeviceDescriptor::default(),
                None,
                None,
            )
            .unwrap();
        instance.adapter_drop(adapter);
        Arc::new(DeviceHandle::new(instance, device, queue, timeout))
    }

    /// Holds the device's poller inside a callback until the returned sender
    /// sends. A map starts the poller, since a submit would run the callback
    /// on this thread.
    fn hold_poll(device: &Arc<DeviceHandle>) -> std::sync::mpsc::Sender<()> {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        device.instance.queue_on_submitted_work_done(
            device.queue,
            Box::new(move || {
                entered_tx.send(()).unwrap();
                let _ = release_rx.recv_timeout(TIMEOUT);
            }),
        );
        let other = buffer(device, wgpu_types::BufferUsages::MAP_READ);
        drop(device.map_async(other, wgpu_core::device::HostMap::Read, 0, None, unmapped()));
        entered_rx.recv_timeout(TIMEOUT).unwrap();
        release_tx
    }

    fn poller_idle(device: &DeviceHandle) -> bool {
        let deadline = Instant::now() + TIMEOUT;
        while device.lock_poller().running {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        true
    }

    fn buffer(device: &DeviceHandle, usage: wgpu_types::BufferUsages) -> BufferId {
        let (buffer, err) = device.instance.device_create_buffer(
            device.device,
            &wgpu_types::BufferDescriptor {
                label: None,
                size: 16,
                usage,
                mapped_at_creation: false,
            },
            None,
        );
        assert!(err.is_none(), "{err:?}");
        buffer
    }

    #[test]
    fn work_done_resolves_with_nothing_submitted() {
        let device = noop_device();
        device
            .on_submitted_work_done()
            .recv_timeout(TIMEOUT)
            .unwrap();
    }

    #[test]
    fn work_done_resolves_after_a_queued_write() {
        let device = noop_device();
        let buffer = buffer(&device, wgpu_types::BufferUsages::COPY_DST);
        device
            .instance
            .queue_write_buffer(device.queue, buffer, 0, &[1; 16])
            .unwrap();
        device
            .on_submitted_work_done()
            .recv_timeout(TIMEOUT)
            .unwrap();
        device.instance.buffer_drop(buffer);
    }

    #[test]
    fn work_done_resolves_after_its_holders_drop() {
        let device = noop_device();
        let done = device.on_submitted_work_done();
        drop(device);
        done.recv_timeout(TIMEOUT).unwrap();
    }

    #[test]
    fn concurrent_waits_all_resolve() {
        let device = noop_device();
        let waits: Vec<_> = (0..8).map(|_| device.on_submitted_work_done()).collect();
        for done in waits {
            done.recv_timeout(TIMEOUT).unwrap();
        }
    }

    fn unmapped() -> Arc<Mutex<webgpu::GpuBufferMapState>> {
        Arc::new(Mutex::new(webgpu::GpuBufferMapState::Pending))
    }

    fn state(state: &Mutex<webgpu::GpuBufferMapState>) -> webgpu::GpuBufferMapState {
        *state.lock().unwrap()
    }

    #[test]
    fn map_async_resolves() {
        let device = noop_device();
        let buffer = buffer(&device, wgpu_types::BufferUsages::MAP_READ);
        let map_state = unmapped();
        let mapped = device.map_async(
            buffer,
            wgpu_core::device::HostMap::Read,
            0,
            None,
            Arc::clone(&map_state),
        );
        assert!(mapped.recv_timeout(TIMEOUT).unwrap().is_ok());
        assert_eq!(state(&map_state), webgpu::GpuBufferMapState::Mapped);
        device.instance.buffer_drop(buffer);
    }

    #[test]
    fn map_async_reports_a_validation_error() {
        let device = noop_device();
        let buffer = buffer(&device, wgpu_types::BufferUsages::MAP_READ);
        let map_state = unmapped();
        let mapped = device.map_async(
            buffer,
            wgpu_core::device::HostMap::Write,
            0,
            None,
            Arc::clone(&map_state),
        );
        let err = mapped.recv_timeout(TIMEOUT).unwrap().unwrap_err();
        assert!(matches!(
            err.kind,
            webgpu::MapAsyncErrorKind::OperationError
        ));
        assert_eq!(state(&map_state), webgpu::GpuBufferMapState::Unmapped);
        device.instance.buffer_drop(buffer);
    }

    #[test]
    fn map_state_follows_a_map_nobody_waits_for() {
        let device = noop_device();
        let buffer = buffer(&device, wgpu_types::BufferUsages::MAP_READ);
        let map_state = unmapped();
        drop(device.map_async(
            buffer,
            wgpu_core::device::HostMap::Read,
            0,
            None,
            Arc::clone(&map_state),
        ));
        let deadline = std::time::Instant::now() + TIMEOUT;
        while state(&map_state) == webgpu::GpuBufferMapState::Pending {
            assert!(std::time::Instant::now() < deadline, "map never settled");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(state(&map_state), webgpu::GpuBufferMapState::Mapped);
        device.instance.buffer_drop(buffer);
    }

    #[test]
    fn a_wait_started_during_a_long_poll_is_not_late() {
        let device = noop_device_with(Some(Duration::from_millis(20)));
        let release = hold_poll(&device);
        let buffer = buffer(&device, wgpu_types::BufferUsages::MAP_READ);
        let map_state = unmapped();
        let mapped = device.map_async(
            buffer,
            wgpu_core::device::HostMap::Read,
            0,
            None,
            Arc::clone(&map_state),
        );
        // Outlast the timeout before any poll has run the map's callback.
        std::thread::sleep(Duration::from_millis(60));
        release.send(()).unwrap();
        assert!(mapped.recv_timeout(TIMEOUT).unwrap().is_ok());
        assert!(!device.lost().is_lost());
        device.instance.buffer_drop(buffer);
    }

    #[test]
    fn watched_work_that_finishes_keeps_the_device() {
        let device = noop_device_with(Some(Duration::from_secs(5)));
        let index = device.instance.queue_submit(device.queue, &[]).unwrap();
        device.watch(index);
        assert!(poller_idle(&device), "poller never went idle");
        assert!(!device.lost().is_lost());
    }

    #[test]
    fn release_drops_an_idle_queue() {
        let device = noop_device_with(Some(Duration::from_secs(5)));
        let instance = Arc::clone(&device.instance);
        let adapter = instance
            .request_adapter(
                &wgpu_types::RequestAdapterOptions::default(),
                wgpu_types::Backends::NOOP,
                None,
            )
            .unwrap();
        let (other, queue) = instance
            .adapter_request_device(
                adapter,
                &wgpu_types::DeviceDescriptor::default(),
                None,
                None,
            )
            .unwrap();
        instance.adapter_drop(adapter);
        let release = Release {
            instance,
            device: other,
            queue,
            timeout: Some(Duration::from_secs(5)),
            lost: Arc::default(),
        };
        assert!(release.run());
    }

    #[test]
    fn destroy_loses_the_device() {
        let device = noop_device();
        device.destroy();
        let info = device.lost().info().unwrap();
        assert_eq!(info.reason, webgpu::GpuDeviceLostReason::Destroyed);
    }

    #[test]
    fn waits_on_a_destroyed_device_settle() {
        let device = noop_device();
        let buffer = buffer(&device, wgpu_types::BufferUsages::MAP_READ);
        device.destroy();
        device
            .on_submitted_work_done()
            .recv_timeout(TIMEOUT)
            .unwrap();
        let map_state = unmapped();
        let mapped = device.map_async(
            buffer,
            wgpu_core::device::HostMap::Read,
            0,
            None,
            Arc::clone(&map_state),
        );
        assert!(mapped.recv_timeout(TIMEOUT).unwrap().is_err());
        assert_eq!(state(&map_state), webgpu::GpuBufferMapState::Unmapped);
        device.instance.buffer_drop(buffer);
    }

    #[test]
    fn settle_keeps_the_first_value() {
        let (settle, recv) = Settle::new();
        settle.clone().send(1);
        settle.send(2);
        assert_eq!(recv.recv_timeout(TIMEOUT).unwrap(), 1);
    }

    #[test]
    fn completed_watch_does_not_expire_behind_newer_work() {
        let now = Instant::now();
        let mut poller = Poller::default();
        for (index, deadline) in [(1, now - TIMEOUT), (2, now + TIMEOUT)] {
            poller.waiting.push(Waiter {
                index,
                generation: 0,
                deadline: Some(deadline),
                pending: Box::new(Watch),
            });
        }
        let completed = 1;
        let (target, started) = poller.start_poll();
        assert!(!poller.finish_poll(target, started, target <= completed));
        assert_eq!(poller.waiting.len(), 1);
        assert_eq!(poller.waiting[0].index, 2);
    }

    #[test]
    fn each_expired_watch_is_polled_before_loss() {
        let mut poller = Poller::default();
        for index in 1..=2 {
            poller.waiting.push(Waiter {
                index,
                generation: 0,
                deadline: Some(Instant::now() - TIMEOUT),
                pending: Box::new(Watch),
            });
        }
        let (target, started) = poller.start_poll();
        assert!(!poller.finish_poll(target, started, true));
        let (target, started) = poller.start_poll();
        assert!(!poller.finish_poll(target, started, true));
        assert!(poller.waiting.is_empty());
    }

    #[test]
    fn unfinished_watch_expires() {
        let mut poller = Poller::default();
        poller.waiting.push(Waiter {
            index: 1,
            generation: 0,
            deadline: Some(Instant::now() - TIMEOUT),
            pending: Box::new(Watch),
        });
        let (target, started) = poller.start_poll();
        assert!(poller.finish_poll(target, started, false));
    }
}
