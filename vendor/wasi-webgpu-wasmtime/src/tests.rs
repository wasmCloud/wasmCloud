use std::{
    future::{poll_fn, Future},
    sync::{mpsc, Arc, Mutex},
    task::{Context, Poll, Wake, Waker},
    time::{Duration, Instant},
};

use wasmtime::{component::Resource, Engine, Store};

use crate::{
    poller::Poller,
    wasi::webgpu::webgpu::{self, HostGpuBuffer, HostGpuBufferWithStore},
    Buffer, HasWasiWebGpuCtx, WasiWebGpuCtx, WasiWebGpuCtxView, WasiWebGpuOptions,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

struct State {
    instance: Arc<wgpu_core::global::Global>,
    table: wasmtime_wasi::ResourceTable,
    options: WasiWebGpuOptions,
}

impl State {
    fn new(mapped: bool) -> (Self, u32, wgpu_core::id::QueueId) {
        let instance = Arc::new(wgpu_core::global::Global::new(
            "map-tests",
            wgpu_types::InstanceDescriptor {
                backends: wgpu_types::Backends::NOOP,
                backend_options: wgpu_types::BackendOptions {
                    noop: wgpu_types::NoopBackendOptions { enable: true },
                    ..Default::default()
                },
                flags: wgpu_types::InstanceFlags::from_build_config(),
                memory_budget_thresholds: Default::default(),
                display: None,
            },
            None,
        ));
        let adapter = instance
            .request_adapter(&Default::default(), wgpu_types::Backends::NOOP, None)
            .unwrap();
        let (device, queue) = instance
            .adapter_request_device(adapter, &Default::default(), None, None)
            .unwrap();
        let (buffer_id, error) = instance.device_create_buffer(
            device,
            &wgpu_core::resource::BufferDescriptor {
                label: None,
                size: 16,
                usage: wgpu_types::BufferUsages::MAP_READ,
                mapped_at_creation: mapped,
            },
            None,
        );
        assert!(error.is_none());
        let mut state = Self {
            instance,
            table: wasmtime_wasi::ResourceTable::new(),
            options: Default::default(),
        };
        let buffer = state
            .table
            .push(Buffer {
                buffer_id,
                size: 16,
                usage: wgpu_types::BufferUsages::MAP_READ,
                map_state: Arc::new(Mutex::new(if mapped {
                    webgpu::GpuBufferMapState::Mapped
                } else {
                    webgpu::GpuBufferMapState::Unmapped
                })),
                device: Arc::new(device),
                poller: Arc::new(Poller::new(Some(TEST_TIMEOUT))),
            })
            .unwrap();
        (state, buffer.rep(), queue)
    }

    // Holds the first poll after it has collected its callbacks.
    fn hold_poll(&self, queue: wgpu_core::id::QueueId) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        self.instance.queue_on_submitted_work_done(
            queue,
            Box::new(move || {
                entered_tx.send(()).unwrap();
                let _ = release_rx.recv_timeout(TEST_TIMEOUT);
            }),
        );
        (entered_rx, release_tx)
    }
}

impl WasiWebGpuCtxView for State {
    fn webgpu_ctx(&mut self) -> WasiWebGpuCtx<'_> {
        WasiWebGpuCtx {
            instance: &self.instance,
            table: &mut self.table,
            options: &self.options,
        }
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    struct Thread(std::thread::Thread);
    impl Wake for Thread {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Thread(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                assert!(!remaining.is_zero(), "test future timed out");
                std::thread::park_timeout(remaining);
            }
        }
    }
}

#[test]
fn rejected_remap_preserves_existing_mapping() {
    let (state, rep, _) = State::new(true);
    let mut store = Store::new(&Engine::default(), state);
    block_on(store.run_concurrent(async |access| {
        let access = access.with_getter::<HasWasiWebGpuCtx>(State::webgpu_ctx);
        let result = HasWasiWebGpuCtx::map_async(
            &access,
            Resource::new_borrow(rep),
            webgpu::GpuMapMode::READ,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(matches!(
            result.unwrap_err().kind,
            webgpu::MapAsyncErrorKind::OperationError
        ));
        access.with(|mut access| {
            let mut ctx = access.get();
            assert_eq!(
                ctx.map_state(Resource::new_borrow(rep)).unwrap(),
                webgpu::GpuBufferMapState::Mapped
            );
            assert_eq!(
                ctx.get_mapped_range_get_with_copy(Resource::new_borrow(rep), None, None)
                    .unwrap()
                    .unwrap(),
                vec![0; 16]
            );
        });
    }))
    .unwrap();
}

#[test]
fn rejected_duplicate_preserves_pending_mapping() {
    let (state, rep, queue) = State::new(false);
    let (entered, release) = state.hold_poll(queue);
    // Keep the poller in a callback while both map requests start.
    let buffer = state
        .table
        .get(&Resource::<Buffer>::new_borrow(rep))
        .unwrap();
    let mut waiter = buffer
        .poller
        .waiter(&state.instance, Arc::clone(&buffer.device));
    let done = waiter.on_done();
    entered.recv_timeout(TEST_TIMEOUT).unwrap();
    let mut store = Store::new(&Engine::default(), state);
    block_on(store.run_concurrent(async |access| {
        let access = access.with_getter::<HasWasiWebGpuCtx>(State::webgpu_ctx);
        let mut first = std::pin::pin!(HasWasiWebGpuCtx::map_async(
            &access,
            Resource::new_borrow(rep),
            webgpu::GpuMapMode::READ,
            None,
            None,
        ));
        assert!(poll_fn(|cx| Poll::Ready(first.as_mut().poll(cx)))
            .await
            .is_pending());
        let second = HasWasiWebGpuCtx::map_async(
            &access,
            Resource::new_borrow(rep),
            webgpu::GpuMapMode::READ,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(matches!(
            second.unwrap_err().kind,
            webgpu::MapAsyncErrorKind::OperationError
        ));
        access.with(|mut access| {
            assert_eq!(
                access.get().map_state(Resource::new_borrow(rep)).unwrap(),
                webgpu::GpuBufferMapState::Pending
            );
        });
        done();
        release.send(()).unwrap();
        first.await.unwrap().unwrap();
    }))
    .unwrap();
}

#[test]
fn cancelled_waiter_still_drives_pending_map_callback() {
    let (state, rep, queue) = State::new(false);
    let (entered, release) = state.hold_poll(queue);
    let buffer = state
        .table
        .get(&Resource::<Buffer>::new_borrow(rep))
        .unwrap();
    let mut waiter = buffer
        .poller
        .waiter(&state.instance, Arc::clone(&buffer.device));
    entered.recv_timeout(TEST_TIMEOUT).unwrap();
    let (mapped_tx, mapped_rx) = mpsc::channel();
    let done = waiter.on_done();
    state
        .instance
        .buffer_map_async(
            buffer.buffer_id,
            0,
            None,
            wgpu_core::resource::BufferMapOperation {
                host: wgpu_core::device::HostMap::Read,
                callback: Some(Box::new(move |mapped| {
                    done();
                    let _ = mapped_tx.send(mapped);
                })),
            },
        )
        .unwrap();
    drop(waiter);
    release.send(()).unwrap();
    mapped_rx
        .recv_timeout(TEST_TIMEOUT)
        .expect("cancelled waiter left callback undriven")
        .unwrap();
}
