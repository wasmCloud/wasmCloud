use std::{
    future::{poll_fn, Future},
    sync::{mpsc, Arc, Mutex},
    task::{Context, Poll, Wake, Waker},
    time::{Duration, Instant},
};

use wasmtime::{
    component::{Resource, ResourceTable},
    Engine, Store,
};

use crate::{
    device_handle::DeviceHandle,
    wasi::webgpu::webgpu::{self, HostGpuBuffer, HostGpuBufferWithStore},
    Buffer, HasWasiWebGpuCtx, WasiWebGpuCtx, WasiWebGpuCtxView, WasiWebGpuOptions,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

struct State {
    instance: Arc<wgpu_core::global::Global>,
    table: ResourceTable,
    options: WasiWebGpuOptions,
    device: Arc<DeviceHandle>,
}

impl State {
    fn new(mapped: bool) -> (Self, u32) {
        let instance = Arc::new(wgpu_core::global::Global::new(
            "map-tests",
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
            .request_adapter(&Default::default(), wgpu_types::Backends::NOOP, None)
            .unwrap();
        let (device, queue) = instance
            .adapter_request_device(adapter, &Default::default(), None, None)
            .unwrap();
        instance.adapter_drop(adapter);
        let device = Arc::new(DeviceHandle::new(
            Arc::clone(&instance),
            device,
            queue,
            Some(TEST_TIMEOUT),
        ));
        let (buffer_id, error) = instance.device_create_buffer(
            *device.device_id(),
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
            table: ResourceTable::new(),
            options: Default::default(),
            device: Arc::clone(&device),
        };
        let buffer = state
            .table
            .push(Buffer {
                buffer_id,
                device,
                size: 16,
                usage: wgpu_types::BufferUsages::MAP_READ,
                map_state: Arc::new(Mutex::new(if mapped {
                    webgpu::GpuBufferMapState::Mapped
                } else {
                    webgpu::GpuBufferMapState::Unmapped
                })),
            })
            .unwrap();
        (state, buffer.rep())
    }

    /// Holds the device's poller inside a callback until the returned sender
    /// sends, so callbacks registered meanwhile wait for the next poll.
    fn hold_poll(&self) -> mpsc::Sender<()> {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        self.instance.queue_on_submitted_work_done(
            *self.device.queue_id(),
            Box::new(move || {
                entered_tx.send(()).unwrap();
                let _ = release_rx.recv_timeout(TEST_TIMEOUT);
            }),
        );
        // A map starts the poller without a submit, which would maintain the
        // device, and so run the callback, on this thread.
        let (other, error) = self.instance.device_create_buffer(
            *self.device.device_id(),
            &wgpu_core::resource::BufferDescriptor {
                label: None,
                size: 16,
                usage: wgpu_types::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            },
            None,
        );
        assert!(error.is_none());
        let other_state = Arc::new(Mutex::new(webgpu::GpuBufferMapState::Pending));
        drop(self.device.map_async(
            other,
            wgpu_core::device::HostMap::Read,
            0,
            None,
            other_state,
        ));
        entered_rx.recv_timeout(TEST_TIMEOUT).unwrap();
        release_tx
    }

    fn map_state(&self, rep: u32) -> Arc<Mutex<webgpu::GpuBufferMapState>> {
        let buffer = self.table.get(&Resource::<Buffer>::new_borrow(rep));
        Arc::clone(&buffer.unwrap().map_state)
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
    let (state, rep) = State::new(true);
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
    let (state, rep) = State::new(false);
    let release = state.hold_poll();
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
        release.send(()).unwrap();
        first.await.unwrap().unwrap();
    }))
    .unwrap();
}

#[test]
fn cancelled_map_still_settles_its_buffer() {
    let (state, rep) = State::new(false);
    let release = state.hold_poll();
    let mut store = Store::new(&Engine::default(), state);
    block_on(store.run_concurrent(async |access| {
        let access = access.with_getter::<HasWasiWebGpuCtx>(State::webgpu_ctx);
        let mut map = Box::pin(HasWasiWebGpuCtx::map_async(
            &access,
            Resource::new_borrow(rep),
            webgpu::GpuMapMode::READ,
            None,
            None,
        ));
        assert!(poll_fn(|cx| Poll::Ready(map.as_mut().poll(cx)))
            .await
            .is_pending());
        drop(map);
    }))
    .unwrap();
    let map_state = store.data().map_state(rep);
    release.send(()).unwrap();
    let deadline = Instant::now() + TEST_TIMEOUT;
    while *map_state.lock().unwrap() == webgpu::GpuBufferMapState::Pending {
        assert!(Instant::now() < deadline, "cancelled map left undriven");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        *map_state.lock().unwrap(),
        webgpu::GpuBufferMapState::Mapped
    );
}

#[test]
fn reading_an_unmapped_range_is_an_operation_error() {
    let (mut state, rep) = State::new(false);
    let mut ctx = state.webgpu_ctx();
    let read = ctx
        .get_mapped_range_get_with_copy(Resource::new_borrow(rep), None, None)
        .unwrap();
    assert!(matches!(
        read.unwrap_err().kind,
        webgpu::GetMappedRangeErrorKind::OperationError
    ));
    let write = ctx
        .get_mapped_range_set_with_copy(Resource::new_borrow(rep), vec![0; 16], None, None)
        .unwrap();
    assert!(matches!(
        write.unwrap_err().kind,
        webgpu::GetMappedRangeErrorKind::OperationError
    ));
}

fn assert_operation_error<T: std::fmt::Debug>(result: Result<T, webgpu::GetMappedRangeError>) {
    assert!(
        matches!(
            &result,
            Err(webgpu::GetMappedRangeError {
                kind: webgpu::GetMappedRangeErrorKind::OperationError,
                ..
            })
        ),
        "{result:?}"
    );
}

#[test]
fn an_invalid_mapped_range_is_an_operation_error() {
    let (mut state, rep) = State::new(true);
    let mut ctx = state.webgpu_ctx();
    let buffer = || Resource::new_borrow(rep);
    // Past the end, unaligned, and longer than the buffer.
    for (offset, size) in [(Some(24), None), (Some(4), None), (None, Some(24))] {
        assert_operation_error(
            ctx.get_mapped_range_get_with_copy(buffer(), offset, size)
                .unwrap(),
        );
        assert_operation_error(
            ctx.get_mapped_range_set_with_copy(buffer(), vec![0; 16], offset, size)
                .unwrap(),
        );
    }
}

#[test]
fn writing_the_wrong_length_is_an_operation_error() {
    let (mut state, rep) = State::new(true);
    let mut ctx = state.webgpu_ctx();
    let buffer = || Resource::new_borrow(rep);
    for len in [8, 24] {
        assert_operation_error(
            ctx.get_mapped_range_set_with_copy(buffer(), vec![1; len], None, None)
                .unwrap(),
        );
    }
    let data: Vec<u8> = (0..8).collect();
    ctx.get_mapped_range_set_with_copy(buffer(), data.clone(), Some(8), None)
        .unwrap()
        .unwrap();
    let read = ctx
        .get_mapped_range_get_with_copy(buffer(), None, None)
        .unwrap()
        .unwrap();
    assert_eq!(read, [vec![0; 8], data].concat());
}

fn unmap_before_map_callback(remap: bool, destroy: bool) {
    let (state, rep) = State::new(false);
    let (other, error) = state.instance.device_create_buffer(
        *state.device.device_id(),
        &wgpu_core::resource::BufferDescriptor {
            label: None,
            size: 16,
            usage: wgpu_types::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        },
        None,
    );
    assert!(error.is_none());
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    state
        .instance
        .buffer_map_async(
            other,
            0,
            None,
            wgpu_core::resource::BufferMapOperation {
                host: wgpu_core::device::HostMap::Read,
                callback: Some(Box::new(move |result| {
                    assert!(result.is_ok());
                    entered_tx.send(()).unwrap();
                    release_rx.recv_timeout(TEST_TIMEOUT).unwrap();
                })),
            },
        )
        .unwrap();
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
        entered_rx.recv_timeout(TEST_TIMEOUT).unwrap();
        // Native mapping is complete; its callback is blocked behind another map.
        access.with(|mut access| {
            let mut ctx = access.get();
            if destroy {
                ctx.destroy(Resource::new_borrow(rep)).unwrap();
            } else {
                ctx.unmap(Resource::new_borrow(rep)).unwrap().unwrap();
            }
        });
        let mut second = std::pin::pin!(HasWasiWebGpuCtx::map_async(
            &access,
            Resource::new_borrow(rep),
            webgpu::GpuMapMode::READ,
            None,
            None,
        ));
        if remap {
            assert!(poll_fn(|cx| Poll::Ready(second.as_mut().poll(cx)))
                .await
                .is_pending());
        }
        release_tx.send(()).unwrap();
        let err = first.await.unwrap().unwrap_err();
        assert!(matches!(err.kind, webgpu::MapAsyncErrorKind::AbortError));
        if remap {
            second.await.unwrap().unwrap();
        }
        access.with(|mut access| {
            let mut ctx = access.get();
            assert_eq!(
                ctx.map_state(Resource::new_borrow(rep)).unwrap(),
                if remap {
                    webgpu::GpuBufferMapState::Mapped
                } else {
                    webgpu::GpuBufferMapState::Unmapped
                }
            );
            if remap {
                ctx.get_mapped_range_get_with_copy(Resource::new_borrow(rep), None, None)
                    .unwrap()
                    .unwrap();
            }
        });
    }))
    .unwrap();
    store.data().instance.buffer_drop(other);
}

#[test]
fn unmap_rejects_a_delayed_map_callback() {
    unmap_before_map_callback(false, false);
}

#[test]
fn delayed_map_callback_preserves_a_new_mapping() {
    unmap_before_map_callback(true, false);
}

#[test]
fn destroy_rejects_a_delayed_map_callback() {
    unmap_before_map_callback(false, true);
}
