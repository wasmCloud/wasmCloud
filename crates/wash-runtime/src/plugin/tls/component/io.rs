//! Byte plumbing between a guest's `stream<u8>`s and a host TLS session.
//!
//! The arrangement is the one wasmtime's own `wasi:tls` host uses: the guest's
//! ciphertext streams become an in-memory duplex transport the handshake runs
//! over, and its cleartext streams read and write a TLS session that does not
//! exist until the handshake finishes — [`Deferred`] parks them until then.
//! Guest stream buffers are read and written in place ([`Source::as_direct`],
//! [`Destination::as_direct`]); ciphertext still takes one copy through the
//! 16KiB [`pipe`] in each direction, between rustls and the guest's buffer.

use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf};
use tokio::sync::oneshot;
use wasmtime::StoreContextMut;
use wasmtime::component::{Destination, Source, StreamConsumer, StreamProducer, StreamResult};

use super::TlsError;

/// Bytes buffered in each direction of a transport pipe, and the most one
/// stream read hands the guest at once.
pub(super) const CAPACITY: usize = 16 * 1024;

/// A one-way in-memory pipe that reports EOF when the writer goes away and a
/// broken pipe when the reader does, which `tokio::io::simplex` does not.
pub(super) fn pipe() -> (Reader, Writer) {
    let (r, w) = tokio::io::duplex(CAPACITY);
    (Reader(r), Writer(w))
}

pub(super) struct Reader(DuplexStream);
pub(super) struct Writer(DuplexStream);

impl AsyncRead for Reader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for Writer {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

/// A session's byte stream, whichever way the handshake ended.
pub(super) trait SessionIo: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> SessionIo for T {}

/// An `Arc<Mutex<IO>>`, so the send and receive halves share one session.
pub(super) struct Shared<IO>(Arc<Mutex<IO>>);

impl<IO> Shared<IO> {
    pub(super) fn new(io: IO) -> Self {
        Self(Arc::new(Mutex::new(io)))
    }

    pub(super) fn lock(&self) -> MutexGuard<'_, IO> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl<IO> Clone for Shared<IO> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<IO: AsyncRead + Unpin> AsyncRead for Shared<IO> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.lock()).poll_read(cx, buf)
    }
}

impl<IO: AsyncWrite + Unpin> AsyncWrite for Shared<IO> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut *self.lock()).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.lock()).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut *self.lock()).poll_shutdown(cx)
    }
}

/// A session that exists once the handshake resolves it. Until then every
/// read and write is pending, with its waker kept for [`Deferred::resolve`].
pub(super) enum Deferred<IO> {
    Pending { read: Waker, write: Waker },
    Ready(IO),
}

impl<IO> Deferred<IO> {
    pub(super) fn pending() -> Self {
        Self::Pending {
            read: Waker::noop().clone(),
            write: Waker::noop().clone(),
        }
    }

    /// Install the session and wake whatever was waiting on it. A second
    /// resolve is ignored: the first outcome stands.
    pub(super) fn resolve(&mut self, io: IO) {
        let Self::Pending { read, write } = self else {
            return;
        };
        let (read, write) = (
            std::mem::replace(read, Waker::noop().clone()),
            std::mem::replace(write, Waker::noop().clone()),
        );
        *self = Self::Ready(io);
        read.wake();
        write.wake();
    }
}

impl<IO: AsyncRead + Unpin> AsyncRead for Deferred<IO> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Pending { read, .. } => {
                read.clone_from(cx.waker());
                Poll::Pending
            }
            Self::Ready(io) => Pin::new(io).poll_read(cx, buf),
        }
    }
}

impl<IO: AsyncWrite + Unpin> AsyncWrite for Deferred<IO> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match &mut *self {
            Self::Pending { write, .. } => {
                write.clone_from(cx.waker());
                Poll::Pending
            }
            Self::Ready(io) => Pin::new(io).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Pending { write, .. } => {
                write.clone_from(cx.waker());
                Poll::Pending
            }
            Self::Ready(io) => Pin::new(io).poll_flush(cx),
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match &mut *self {
            Self::Pending { write, .. } => {
                write.clone_from(cx.waker());
                Poll::Pending
            }
            Self::Ready(io) => Pin::new(io).poll_shutdown(cx),
        }
    }
}

/// A session that failed: every operation reports the failure, and as a
/// guest stream it is already closed.
pub(super) struct Closed(pub(super) TlsError);

impl AsyncRead for Closed {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(self.0.clone().into()))
    }
}

impl AsyncWrite for Closed {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Err(self.0.clone().into()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(self.0.clone().into()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(self.0.clone().into()))
    }
}

impl<D> StreamProducer<D> for Closed {
    type Item = u8;
    type Buffer = BytesMut;

    fn poll_produce<'a>(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _store: StoreContextMut<'a, D>,
        _dst: Destination<'a, Self::Item, Self::Buffer>,
        _finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        Poll::Ready(Ok(StreamResult::Dropped))
    }
}

/// Where a producer or consumer reports how its IO ended: the IO back on a
/// clean end, so the caller can shut it down, or the error that ended it.
type Ended<IO> = oneshot::Sender<std::io::Result<IO>>;

/// Serves a guest `stream<u8>` read from an [`AsyncRead`].
pub(super) struct AsyncReadProducer<IO>(Option<(IO, Ended<IO>)>);

impl<IO> AsyncReadProducer<IO> {
    pub(super) fn new(io: IO, ended: Ended<IO>) -> Self {
        Self(Some((io, ended)))
    }

    fn end(&mut self, result: std::io::Result<()>) {
        if let Some((io, ended)) = self.0.take() {
            let _ = ended.send(result.map(|()| io));
        }
    }
}

impl<IO> Drop for AsyncReadProducer<IO> {
    fn drop(&mut self) {
        self.end(Ok(()));
    }
}

impl<D, IO> StreamProducer<D> for AsyncReadProducer<IO>
where
    IO: AsyncRead + Send + Unpin + 'static,
{
    type Item = u8;
    type Buffer = BytesMut;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: StoreContextMut<'a, D>,
        dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let Some((io, _)) = self.0.as_mut() else {
            return Poll::Ready(Ok(StreamResult::Dropped));
        };
        let mut dst = dst.as_direct(store, CAPACITY);
        let remaining = dst.remaining();
        // A zero-length read asks only whether the stream is ready, which an
        // `AsyncRead` cannot answer without reading; say yes and read on the
        // next poll (WebAssembly/component-model#561).
        if remaining.is_empty() {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        let mut buf = ReadBuf::new(remaining);
        match Pin::new(io).poll_read(cx, &mut buf) {
            Poll::Ready(Ok(())) if buf.filled().is_empty() => {
                self.end(Ok(()));
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Poll::Ready(Ok(())) => {
                let n = buf.filled().len();
                dst.mark_written(n);
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Poll::Ready(Err(e)) => {
                self.end(Err(e));
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Poll::Pending if finish => Poll::Ready(Ok(StreamResult::Cancelled)),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Drains a guest `stream<u8>` into an [`AsyncWrite`].
pub(super) struct AsyncWriteConsumer<IO>(Option<(IO, Ended<IO>)>);

impl<IO> AsyncWriteConsumer<IO> {
    pub(super) fn new(io: IO, ended: Ended<IO>) -> Self {
        Self(Some((io, ended)))
    }

    fn end(&mut self, result: std::io::Result<()>) {
        if let Some((io, ended)) = self.0.take() {
            let _ = ended.send(result.map(|()| io));
        }
    }
}

impl<IO> Drop for AsyncWriteConsumer<IO> {
    fn drop(&mut self) {
        self.end(Ok(()));
    }
}

impl<D, IO> StreamConsumer<D> for AsyncWriteConsumer<IO>
where
    IO: AsyncWrite + Send + Unpin + 'static,
{
    type Item = u8;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: StoreContextMut<D>,
        src: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let Some((io, _)) = self.0.as_mut() else {
            return Poll::Ready(Ok(StreamResult::Dropped));
        };
        let mut src = src.as_direct(store);
        let remaining = src.remaining();
        // The write-side readiness check; see `AsyncReadProducer`.
        if remaining.is_empty() {
            return Poll::Ready(Ok(if finish {
                StreamResult::Cancelled
            } else {
                StreamResult::Completed
            }));
        }
        match Pin::new(io).poll_write(cx, remaining) {
            Poll::Ready(Ok(0)) => {
                self.end(Err(std::io::ErrorKind::WriteZero.into()));
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Poll::Ready(Ok(n)) => {
                src.mark_read(n);
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Poll::Ready(Err(e)) => {
                self.end(Err(e));
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Poll::Pending if finish => Poll::Ready(Ok(StreamResult::Cancelled)),
            Poll::Pending => Poll::Pending,
        }
    }
}
