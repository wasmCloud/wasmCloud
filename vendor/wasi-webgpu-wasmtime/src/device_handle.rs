//! Shared ownership of a device and its queue, and the thread that polls them.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use wgpu_core::{
    device::{queue::SubmittedWorkDoneClosure, WaitIdleError},
    global::Global,
    id::{BufferId, DeviceId, QueueId},
    resource::{BufferAccessError, BufferMapOperation},
    SubmissionIndex,
};

use crate::wasi::webgpu::webgpu;

type Abort = Box<dyn FnOnce() + Send>;

/// Longest one poll waits. A waiting poll holds wgpu-core's fence lock, which
/// `queue_submit` needs, so a long wait would stall the guest's submits.
const WAIT_SLICE: Duration = Duration::from_millis(10);

/// A device and its queue. They leave the hub when the last holder drops.
///
/// The guest's device, its queues and buffers, and a running poll each hold
/// one, so ids stay valid for a call that is still waiting.
pub(crate) struct DeviceHandle {
    instance: Arc<Global>,
    device: DeviceId,
    queue: QueueId,
    poller: Mutex<Poller>,
}

#[derive(Default)]
struct Poller {
    running: bool,
    target: SubmissionIndex,
    /// Counts poll starts, so a poll only retires waiters registered before it.
    generation: u64,
    waiting: Vec<Waiter>,
}

struct Waiter {
    index: SubmissionIndex,
    generation: u64,
    abort: Abort,
}

impl DeviceHandle {
    pub(crate) fn new(instance: Arc<Global>, device: DeviceId, queue: QueueId) -> Self {
        Self {
            instance,
            device,
            queue,
            poller: Mutex::default(),
        }
    }

    pub(crate) fn device_id(&self) -> &DeviceId {
        &self.device
    }

    pub(crate) fn queue_id(&self) -> &QueueId {
        &self.queue
    }

    /// Resolves once all work submitted so far, including queued writes, is done.
    pub(crate) fn on_submitted_work_done(self: &Arc<Self>) -> oneshot::Receiver<()> {
        // wgpu-core leaves queued writes out of work-done callbacks, so submit them.
        if let Err((_, err)) = self.instance.queue_submit(self.queue, &[]) {
            log::warn!("failed to flush queued writes: {err}");
        }
        let (settle, done) = Settle::new();
        let on_done = settle.clone();
        let callback: SubmittedWorkDoneClosure = Box::new(move || on_done.send(()));
        let index = self
            .instance
            .queue_on_submitted_work_done(self.queue, callback);
        self.poll_until(index, Box::new(move || settle.send(())));
        done
    }

    /// Maps `buffer`, resolving with the result once the mapping is done.
    pub(crate) fn map_async(
        self: &Arc<Self>,
        buffer: BufferId,
        host: wgpu_core::device::HostMap,
        offset: u64,
        size: Option<u64>,
    ) -> oneshot::Receiver<Result<(), webgpu::MapAsyncError>> {
        let (settle, mapped) = Settle::new();
        let on_mapped = settle.clone();
        let op = BufferMapOperation {
            host,
            callback: Some(Box::new(move |result: Result<(), BufferAccessError>| {
                on_mapped.send(result.map_err(|err| map_error(&err)));
            })),
        };
        // On error, wgpu-core has already called the callback with it.
        if let Ok(index) = self.instance.buffer_map_async(buffer, offset, size, op) {
            self.poll_until(
                index,
                Box::new(move || settle.send(Err(aborted("the device was lost")))),
            );
        }
        mapped
    }

    /// Polls this device on a background thread until `index` completes,
    /// which runs the wgpu callbacks registered on it. `abort` runs instead if
    /// polling fails. One thread per device serves every waiting call.
    fn poll_until(self: &Arc<Self>, index: SubmissionIndex, abort: Abort) {
        {
            let mut poller = self.lock_poller();
            poller.target = poller.target.max(index);
            let generation = poller.generation;
            poller.waiting.push(Waiter {
                index,
                generation,
                abort,
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
            log::error!("failed to spawn the device poll thread: {err}");
            self.abort_waiting();
        }
    }

    fn run_poller(self: Arc<Self>) {
        loop {
            let (target, started) = {
                let mut poller = self.lock_poller();
                let started = poller.generation;
                poller.generation += 1;
                (poller.target, started)
            };
            // Index 0 means nothing is in flight, and any poll fires the callback.
            let poll_type = match target {
                0 => wgpu_types::PollType::Poll,
                index => wgpu_types::PollType::Wait {
                    submission_index: Some(index),
                    timeout: Some(WAIT_SLICE),
                },
            };
            match self.instance.device_poll(self.device, poll_type) {
                Ok(_) => {}
                Err(WaitIdleError::Timeout) => continue,
                Err(err) => {
                    log::warn!("device poll failed: {err}");
                    self.abort_waiting();
                    return;
                }
            }
            let mut poller = self.lock_poller();
            poller
                .waiting
                .retain(|waiter| waiter.index > target || waiter.generation > started);
            if poller.waiting.is_empty() {
                poller.running = false;
                poller.target = 0;
                return;
            }
        }
    }

    fn abort_waiting(&self) {
        let waiting = {
            let mut poller = self.lock_poller();
            poller.running = false;
            poller.target = 0;
            std::mem::take(&mut poller.waiting)
        };
        for waiter in waiting {
            (waiter.abort)();
        }
    }

    fn lock_poller(&self) -> std::sync::MutexGuard<'_, Poller> {
        self.poller.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for DeviceHandle {
    fn drop(&mut self) {
        self.instance.queue_drop(self.queue);
        self.instance.device_drop(self.device);
    }
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
        let send = self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(send) = send {
            let _ = send.send(value);
        }
    }
}

fn map_error(err: &BufferAccessError) -> webgpu::MapAsyncError {
    webgpu::MapAsyncError {
        kind: webgpu::MapAsyncErrorKind::OperationError,
        message: err.to_string(),
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
        Arc::new(DeviceHandle::new(instance, device, queue))
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

    #[test]
    fn map_async_resolves() {
        let device = noop_device();
        let buffer = buffer(&device, wgpu_types::BufferUsages::MAP_READ);
        let mapped = device.map_async(buffer, wgpu_core::device::HostMap::Read, 0, None);
        assert!(mapped.recv_timeout(TIMEOUT).unwrap().is_ok());
        device.instance.buffer_drop(buffer);
    }

    #[test]
    fn map_async_reports_a_validation_error() {
        let device = noop_device();
        let buffer = buffer(&device, wgpu_types::BufferUsages::MAP_READ);
        let mapped = device.map_async(buffer, wgpu_core::device::HostMap::Write, 0, None);
        let err = mapped.recv_timeout(TIMEOUT).unwrap().unwrap_err();
        assert!(matches!(
            err.kind,
            webgpu::MapAsyncErrorKind::OperationError
        ));
        device.instance.buffer_drop(buffer);
    }

    #[test]
    fn settle_keeps_the_first_value() {
        let (settle, recv) = Settle::new();
        settle.clone().send(1);
        settle.send(2);
        assert_eq!(recv.recv_timeout(TIMEOUT).unwrap(), 1);
    }
}
