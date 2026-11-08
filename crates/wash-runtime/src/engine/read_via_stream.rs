//! `wasi:filesystem/types@0.3.0` `descriptor.read-via-stream`, reading what the
//! guest asked for up to [`StoreSettings::max_io_size`].
//!
//! wasmtime-wasi 48 reads a fixed 8 KiB per blocking-pool task whatever the
//! guest's buffer, so a 64 MiB `fread` costs 8,192 round trips. This is
//! bytecodealliance/wasmtime#15095's producer; once wash is on a wasmtime that
//! ships it, delete this module and pass the setting to
//! `WasiCtxBuilder::max_io_size`.
//!
//! Every read goes to the blocking pool: wash never sets
//! `allow_blocking_current_thread`, and an embedder-supplied `WasiCtx` that does
//! is not honored here.
//!
//! [`StoreSettings::max_io_size`]: crate::engine::store_settings::StoreSettings::max_io_size

use std::io;
use std::iter;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};

use bytes::BytesMut;
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, spawn_blocking};
use wasmtime::component::{
    Destination, FutureReader, Linker, Resource, StreamProducer, StreamReader, StreamResult,
};
use wasmtime::error::Context as _;
use wasmtime::{AsContextMut, StoreContextMut};
use wasmtime_wasi::filesystem::{Descriptor, WasiFilesystemView};
use wasmtime_wasi::p3::bindings::filesystem::types::ErrorCode;

use crate::engine::ctx::SharedCtx;

type ReadViaStream = (StreamReader<u8>, FutureReader<Result<(), ErrorCode>>);

/// Replace wasmtime-wasi's `read-via-stream` on a linker that already has its
/// `wasi:filesystem/types@0.3.0`.
pub(crate) fn add_to_linker(linker: &mut Linker<SharedCtx>) -> anyhow::Result<()> {
    let name = format!(
        "wasi:filesystem/types@{}",
        crate::engine::WASI_LINKER_VERSIONS[1]
    );
    linker.allow_shadowing(true);
    let shadowed = linker.instance(&name).and_then(|mut types| {
        types.func_wrap("[method]descriptor.read-via-stream", read_via_stream)
    });
    linker.allow_shadowing(false);
    shadowed?;
    Ok(())
}

fn read_via_stream(
    mut store: StoreContextMut<'_, SharedCtx>,
    (fd, offset): (Resource<Descriptor>, u64),
) -> wasmtime::Result<(ReadViaStream,)> {
    let max_read_size = store.data().store_settings.max_io_size();
    let descriptor = store
        .data_mut()
        .filesystem()
        .table
        .get(&fd)
        .context("failed to get descriptor resource from table")?
        .clone();
    let file = match descriptor {
        Descriptor::File(file) => file.file,
        Descriptor::Dir(_) => {
            return Ok(((
                StreamReader::new(&mut store, iter::empty())?,
                FutureReader::new(&mut store, async {
                    wasmtime::error::Ok(Err(ErrorCode::IsDirectory))
                })?,
            ),));
        }
    };
    let (result_tx, result_rx) = oneshot::channel();
    let producer = ReadStreamProducer {
        file,
        offset,
        result: Some(result_tx),
        task: None,
        max_read_size,
    };
    Ok(((
        StreamReader::new(&mut store, producer)?,
        FutureReader::new(&mut store, result_rx)?,
    ),))
}

struct ReadStreamProducer {
    file: Arc<std::fs::File>,
    offset: u64,
    result: Option<oneshot::Sender<Result<(), ErrorCode>>>,
    task: Option<JoinHandle<io::Result<BytesMut>>>,
    max_read_size: usize,
}

impl Drop for ReadStreamProducer {
    fn drop(&mut self) {
        self.close(Ok(()))
    }
}

impl ReadStreamProducer {
    fn close(&mut self, res: Result<(), ErrorCode>) {
        if let Some(tx) = self.result.take() {
            _ = tx.send(res);
        }
    }

    fn complete_read(&mut self, amt: usize) -> StreamResult {
        let Ok(amt) = u64::try_from(amt) else {
            self.close(Err(ErrorCode::Overflow));
            return StreamResult::Dropped;
        };
        let Some(offset) = self.offset.checked_add(amt) else {
            self.close(Err(ErrorCode::Overflow));
            return StreamResult::Dropped;
        };
        self.offset = offset;
        StreamResult::Completed
    }
}

impl<D> StreamProducer<D> for ReadStreamProducer {
    type Item = u8;
    type Buffer = BytesMut;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let me = &mut *self;
        let task = me.task.get_or_insert_with(|| {
            // A zero-length read asks for readiness, so it gets a full read.
            let size = match dst.remaining(store.as_context_mut()) {
                Some(0) | None => me.max_read_size,
                Some(n) => n.min(me.max_read_size),
            };
            let mut buf = dst.take_buffer();
            buf.resize(size, 0);
            let file = Arc::clone(&me.file);
            let offset = me.offset;
            spawn_blocking(move || {
                read_at(&file, &mut buf, offset).map(|n| {
                    buf.truncate(n);
                    buf
                })
            })
        });

        // The blocking read cannot be interrupted, so `finish` only aborts a
        // task that has not started yet.
        let result = match Pin::new(&mut *task).poll(cx) {
            Poll::Pending if finish => {
                task.abort();
                ready!(Pin::new(task).poll(cx))
            }
            other => ready!(other),
        };
        self.task = None;
        match result {
            Ok(Ok(buf)) if buf.is_empty() => {
                self.close(Ok(()));
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Ok(Ok(buf)) => {
                let n = buf.len();
                dst.set_buffer(buf);
                Poll::Ready(Ok(self.complete_read(n)))
            }
            Ok(Err(err)) => {
                self.close(Err(err.into()));
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Err(err) if err.is_cancelled() => Poll::Ready(Ok(StreamResult::Cancelled)),
            Err(err) => Poll::Ready(Err(wasmtime::format_err!("file read task failed: {err}"))),
        }
    }
}

#[cfg(unix)]
fn read_at(file: &std::fs::File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buf, offset)
}

#[cfg(windows)]
fn read_at(file: &std::fs::File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buf, offset)
}
