// Device loss, shared by a device and everything that waits on it.
// https://www.w3.org/TR/webgpu/#lose-the-device

use std::{
    collections::BTreeMap,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
};

use wasmtime::{
    component::{FutureProducer, Resource},
    StoreContextMut,
};

use crate::{wasi::webgpu::webgpu, WasiWebGpuCtxView};

#[derive(Clone, Debug)]
pub struct DeviceLostInfo {
    pub(crate) reason: webgpu::GpuDeviceLostReason,
    pub(crate) message: String,
}

/// Resolves `GpuDevice.lost` once. The first loss wins: wgpu also reports a
/// device it was told to destroy, and that must not overwrite an earlier
/// reason.
#[derive(Debug, Default)]
pub(crate) struct DeviceLost(Mutex<LostState>);

#[derive(Debug, Default)]
struct LostState {
    info: Option<DeviceLostInfo>,
    /// Keyed by `LostWait`, which takes its waker back when it stops waiting.
    wakers: BTreeMap<u64, Waker>,
    next_waker: u64,
}

impl DeviceLost {
    pub(crate) fn lose(&self, reason: webgpu::GpuDeviceLostReason, message: String) {
        let wakers = {
            let mut state = self.0.lock().unwrap();
            if state.info.is_some() {
                return;
            }
            state.info = Some(DeviceLostInfo { reason, message });
            std::mem::take(&mut state.wakers)
        };
        for waker in wakers.into_values() {
            waker.wake();
        }
    }

    pub(crate) fn info(&self) -> Option<DeviceLostInfo> {
        self.0.lock().unwrap().info.clone()
    }

    pub(crate) fn is_lost(&self) -> bool {
        self.0.lock().unwrap().info.is_some()
    }
}

impl From<wgpu_types::DeviceLostReason> for webgpu::GpuDeviceLostReason {
    fn from(reason: wgpu_types::DeviceLostReason) -> Self {
        match reason {
            wgpu_types::DeviceLostReason::Unknown => webgpu::GpuDeviceLostReason::Unknown,
            wgpu_types::DeviceLostReason::Destroyed => webgpu::GpuDeviceLostReason::Destroyed,
        }
    }
}

/// One reader waiting for a device's loss.
pub(crate) struct LostWait {
    lost: Arc<DeviceLost>,
    waker: Option<u64>,
}

impl LostWait {
    pub(crate) fn new(lost: Arc<DeviceLost>) -> Self {
        Self { lost, waker: None }
    }

    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<DeviceLostInfo> {
        let mut state = self.lost.0.lock().unwrap();
        if let Some(info) = &state.info {
            return Poll::Ready(info.clone());
        }
        let id = match self.waker {
            Some(id) => id,
            None => {
                let id = state.next_waker;
                state.next_waker += 1;
                self.waker = Some(id);
                id
            }
        };
        state.wakers.insert(id, cx.waker().clone());
        Poll::Pending
    }
}

impl Drop for LostWait {
    fn drop(&mut self) {
        if let Some(id) = self.waker {
            self.lost.0.lock().unwrap().wakers.remove(&id);
        }
    }
}

/// The guest's `GpuDevice.lost` future.
pub(crate) struct DeviceLostFuture(pub(crate) LostWait);

impl<D: WasiWebGpuCtxView + 'static> FutureProducer<D> for DeviceLostFuture {
    type Item = Resource<webgpu::GpuDeviceLostInfo>;

    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<D>,
        finish: bool,
    ) -> Poll<wasmtime::Result<Option<Self::Item>>> {
        match self.get_mut().0.poll(cx) {
            Poll::Ready(info) => {
                Poll::Ready(Ok(Some(store.data_mut().webgpu_ctx().table.push(info)?)))
            }
            Poll::Pending if finish => Poll::Ready(Ok(None)),
            Poll::Pending => Poll::Pending,
        }
    }
}
