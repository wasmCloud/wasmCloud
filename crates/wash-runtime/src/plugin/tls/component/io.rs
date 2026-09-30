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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Wake, Waker};

use bytes::BytesMut;
use tokio::io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf};
use tokio::sync::oneshot;
use wasmtime::StoreContextMut;
use wasmtime::component::{Destination, Source, StreamConsumer, StreamProducer, StreamResult};

use super::TlsError;

/// Bytes buffered in each direction of a transport pipe, and the most one
/// stream read hands the guest at once.
pub(super) const CAPACITY: usize = 16 * 1024;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Keep `waker` in `slot`, cloning only when it would wake a different task.
fn register(slot: &Mutex<Option<Waker>>, waker: &Waker) {
    let mut slot = lock(slot);
    if !slot.as_ref().is_some_and(|kept| kept.will_wake(waker)) {
        *slot = Some(waker.clone());
    }
}

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

/// One session shared by its send and receive halves.
///
/// Every poll runs under a waker that wakes both halves. The transport keeps
/// one waker per direction, and a TLS stream sometimes waits on the direction
/// it was not polled for — a read that must first send an alert — which would
/// overwrite the other half's registration and strand it. A spurious wake
/// costs a re-poll.
pub(super) struct Shared<IO> {
    io: Arc<Mutex<IO>>,
    wakers: Arc<BothHalves>,
}

#[derive(Default)]
struct BothHalves {
    read: Mutex<Option<Waker>>,
    write: Mutex<Option<Waker>>,
}

impl Wake for BothHalves {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        for slot in [&self.read, &self.write] {
            if let Some(waker) = lock(slot).take() {
                waker.wake();
            }
        }
    }
}

impl<IO> Shared<IO> {
    pub(super) fn new(io: IO) -> Self {
        Self {
            io: Arc::new(Mutex::new(io)),
            wakers: Arc::default(),
        }
    }

    pub(super) fn lock(&self) -> MutexGuard<'_, IO> {
        lock(&self.io)
    }

    fn poll_half<R>(
        &self,
        half: &Mutex<Option<Waker>>,
        cx: &Context<'_>,
        f: impl FnOnce(Pin<&mut IO>, &mut Context<'_>) -> Poll<R>,
    ) -> Poll<R>
    where
        IO: Unpin,
    {
        register(half, cx.waker());
        let waker = Waker::from(Arc::clone(&self.wakers));
        f(
            Pin::new(&mut *self.lock()),
            &mut Context::from_waker(&waker),
        )
    }
}

impl<IO> Clone for Shared<IO> {
    fn clone(&self) -> Self {
        Self {
            io: Arc::clone(&self.io),
            wakers: Arc::clone(&self.wakers),
        }
    }
}

impl<IO: AsyncRead + Unpin> AsyncRead for Shared<IO> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.poll_half(&self.wakers.read, cx, |io, cx| io.poll_read(cx, buf))
    }
}

impl<IO: AsyncWrite + Unpin> AsyncWrite for Shared<IO> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.poll_half(&self.wakers.write, cx, |io, cx| io.poll_write(cx, buf))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.poll_half(&self.wakers.write, cx, |io, cx| io.poll_flush(cx))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.poll_half(&self.wakers.write, cx, |io, cx| io.poll_shutdown(cx))
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

/// Set when a guest stops reading a connection's cleartext, so the writer
/// feeding that connection's ciphertext in stops too instead of filling its
/// pipe and holding the network side open.
#[derive(Default)]
pub(super) struct Hangup {
    set: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

impl Hangup {
    fn fire(&self) {
        self.set.store(true, Ordering::Release);
        if let Some(waker) = lock(&self.waker).take() {
            waker.wake();
        }
    }

    pub(super) fn is_set(&self) -> bool {
        self.set.load(Ordering::Acquire)
    }
}

/// A writer that fails with a broken pipe once its [`Hangup`] fires.
pub(super) struct HangupWriter<W> {
    inner: W,
    hangup: Arc<Hangup>,
}

impl<W> HangupWriter<W> {
    pub(super) fn new(inner: W, hangup: Arc<Hangup>) -> Self {
        Self { inner, hangup }
    }

    /// Registers before checking, so a hangup between the two still wakes.
    fn hung_up(&self, cx: &Context<'_>) -> bool {
        register(&self.hangup.waker, cx.waker());
        self.hangup.is_set()
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for HangupWriter<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.hung_up(cx) {
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        if self.hung_up(cx) {
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Serves a guest `stream<u8>` read from an [`AsyncRead`].
///
/// The IO is released the moment the stream ends, however it ends: a pipe end
/// kept alive past that would hide a dead transport from its other side.
pub(super) struct AsyncReadProducer<IO> {
    io: Option<IO>,
    ended: Option<oneshot::Sender<std::io::Result<()>>>,
    hangup: Option<Arc<Hangup>>,
}

impl<IO> AsyncReadProducer<IO> {
    pub(super) fn new(io: IO, ended: oneshot::Sender<std::io::Result<()>>) -> Self {
        Self {
            io: Some(io),
            ended: Some(ended),
            hangup: None,
        }
    }

    /// Fire `hangup` when this stream ends.
    pub(super) fn with_hangup(mut self, hangup: Arc<Hangup>) -> Self {
        self.hangup = Some(hangup);
        self
    }

    fn end(&mut self, result: std::io::Result<()>) {
        self.io = None;
        if let Some(hangup) = self.hangup.take() {
            hangup.fire();
        }
        if let Some(ended) = self.ended.take() {
            let _ = ended.send(result);
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
        let Some(io) = self.io.as_mut() else {
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

/// Where a consumer reports how its IO ended: the IO back on a clean end, so
/// the caller can shut it down, or the error that ended it.
type Ended<IO> = oneshot::Sender<std::io::Result<IO>>;

/// Drains a guest `stream<u8>` into an [`AsyncWrite`], flushing each write so
/// accepted bytes reach the transport while the guest keeps its stream open.
pub(super) struct AsyncWriteConsumer<IO> {
    io: Option<(IO, Ended<IO>)>,
    phase: WritePhase,
}

#[derive(Clone, Copy)]
enum WritePhase {
    Writing,
    /// Bytes were accepted and are not yet flushed.
    Flushing,
}

impl<IO> AsyncWriteConsumer<IO> {
    pub(super) fn new(io: IO, ended: Ended<IO>) -> Self {
        Self {
            io: Some((io, ended)),
            phase: WritePhase::Writing,
        }
    }

    fn end(&mut self, result: std::io::Result<()>) {
        if let Some((io, ended)) = self.io.take() {
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
        let this = &mut *self;
        let Some((io, _)) = this.io.as_mut() else {
            return Poll::Ready(Ok(StreamResult::Dropped));
        };
        let mut src = src.as_direct(store);
        if let WritePhase::Writing = this.phase {
            let remaining = src.remaining();
            // The write-side readiness check; see `AsyncReadProducer`.
            if remaining.is_empty() {
                return Poll::Ready(Ok(if finish {
                    StreamResult::Cancelled
                } else {
                    StreamResult::Completed
                }));
            }
            match Pin::new(&mut *io).poll_write(cx, remaining) {
                Poll::Ready(Ok(0)) => {
                    this.end(Err(std::io::ErrorKind::WriteZero.into()));
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
                Poll::Ready(Ok(n)) => {
                    src.mark_read(n);
                    this.phase = WritePhase::Flushing;
                }
                Poll::Ready(Err(e)) => {
                    this.end(Err(e));
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
                Poll::Pending if finish => return Poll::Ready(Ok(StreamResult::Cancelled)),
                Poll::Pending => return Poll::Pending,
            }
        }
        match Pin::new(io).poll_flush(cx) {
            Poll::Ready(Ok(())) => {
                this.phase = WritePhase::Writing;
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Poll::Ready(Err(e)) => {
                this.end(Err(e));
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            // A cancel must not wait on a peer that may never read: the
            // accepted bytes stay buffered, and the next write — or the
            // shutdown when the stream closes — flushes them first.
            Poll::Pending if finish => Poll::Ready(Ok(StreamResult::Cancelled)),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::task::Wake;

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    #[derive(Default)]
    struct Flag(AtomicBool);

    impl Wake for Flag {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// A transport keeping one waker per direction, the shape of tokio's own.
    #[derive(Default)]
    struct OneWakerPerDirection {
        write_waker: Option<Waker>,
    }

    impl AsyncRead for OneWakerPerDirection {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            // A read that must first write — a TLS alert — waits on the write
            // direction with the reader's context.
            self.write_waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }

    impl AsyncWrite for OneWakerPerDirection {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            _buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.write_waker = Some(cx.waker().clone());
            Poll::Pending
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    /// The reader overwriting the transport's single write waker must still
    /// wake the writer parked on it.
    #[test]
    fn a_read_registering_for_write_still_wakes_the_writer() {
        let mut shared = Shared::new(OneWakerPerDirection::default());
        let writer = Arc::new(Flag::default());
        let writer_waker = Waker::from(Arc::clone(&writer));
        let reader_waker = Waker::from(Arc::new(Flag::default()));

        assert!(
            Pin::new(&mut shared)
                .poll_write(&mut Context::from_waker(&writer_waker), b"x")
                .is_pending()
        );
        let mut buf = [0u8; 1];
        assert!(
            Pin::new(&mut shared)
                .poll_read(
                    &mut Context::from_waker(&reader_waker),
                    &mut ReadBuf::new(&mut buf)
                )
                .is_pending()
        );
        let stored = shared.lock().write_waker.take().unwrap();
        stored.wake();
        assert!(writer.0.load(Ordering::SeqCst), "the writer was stranded");
    }

    /// A dead reader must reach the writer as a broken pipe, not a full one.
    #[tokio::test]
    async fn a_producer_that_ends_releases_its_io() {
        let (reader, mut writer) = pipe();
        let (ended, ended_rx) = oneshot::channel();
        drop(AsyncReadProducer::new(reader, ended));
        ended_rx.await.unwrap().unwrap();
        let err = writer.write_all(b"x").await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
    }

    #[tokio::test]
    async fn a_hangup_breaks_a_writer_parked_on_a_full_pipe() {
        let (mut reader, writer) = pipe();
        let hangup = Arc::new(Hangup::default());
        let mut writer = HangupWriter::new(writer, Arc::clone(&hangup));
        let parked = tokio::spawn(async move { writer.write_all(&[0; CAPACITY * 2]).await });
        tokio::task::yield_now().await;
        let (ended, _ended_rx) = oneshot::channel();
        let (unread, _) = pipe();
        drop(AsyncReadProducer::new(unread, ended).with_hangup(hangup));
        let err = parked.await.unwrap().unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
        let mut buf = vec![0; CAPACITY];
        reader.read_exact(&mut buf).await.unwrap();
    }
}
