use super::TcpSocket;
use super::network::{SocketError, SocketResult};
use anyhow::Result;
use io_lifetimes::AsSocketlike;
use rustix::io::Errno;
use std::io;
use std::mem;
use std::net::Shutdown;
use std::sync::Arc;
use tokio::sync::Mutex;
use wasmtime_wasi::runtime::AbortOnDropJoinHandle;
use wasmtime_wasi_io::poll::Pollable;
use wasmtime_wasi_io::streams::{
    DynInputStream, DynOutputStream, InputStream, OutputStream, StreamError,
};

type LoopbackChannel = (bytes::Bytes, tokio::sync::OwnedSemaphorePermit);

impl TcpSocket {
    pub(crate) fn p2_streams(&mut self) -> SocketResult<(DynInputStream, DynOutputStream)> {
        match self {
            Self::Network(socket) => {
                let client = socket
                    .tcp_stream_arc()
                    .map_err(super::network::socket_error_from_util)?;
                let reader = Arc::new(Mutex::new(TcpReader::new(client.clone())));
                let writer = Arc::new(Mutex::new(TcpWriter::new(client.clone())));
                socket
                    .set_p2_streaming_state(P2TcpStreamingState {
                        stream: client.clone(),
                        reader: reader.clone(),
                        writer: writer.clone(),
                    })
                    .map_err(super::network::socket_error_from_util)?;
                let input: DynInputStream = Box::new(TcpReadStream(reader));
                let output: DynOutputStream = Box::new(TcpWriteStream(writer));
                Ok((input, output))
            }
            Self::Loopback(socket) => {
                use super::loopback::{TcpConn, TcpSocket, TcpState};
                let state = mem::replace(&mut socket.state, TcpState::Closed);
                let TcpState::Connected {
                    conn:
                        TcpConn {
                            local_address,
                            remote_address,
                            rx,
                            tx,
                        },
                    accepted,
                } = state
                else {
                    socket.state = state;
                    return Err(super::network::socket_error_from_util(
                        super::util::ErrorCode::InvalidState,
                    ));
                };
                let rx = Arc::new(Mutex::new(rx));
                let tx = Arc::new(std::sync::Mutex::new(tx));
                // Ensure `check-write` allows more than `send_buffer_size` bytes to be written to
                // make this assertion succeed:
                // https://github.com/bytecodealliance/wasmtime/blob/b1c7887c801b62f7fb39e3bd916d8737b3043135/crates/test-programs/src/bin/p2_tcp_streams.rs#L96
                let permits = socket
                    .send_buffer_size
                    .saturating_add(1)
                    .min(TcpSocket::MAX_SEND_BUFFER_SIZE);
                let permits = Arc::new(tokio::sync::Semaphore::new(permits as _));
                socket.state = TcpState::P2Streaming {
                    local_address,
                    remote_address,
                    accepted,
                    permits: Arc::clone(&permits),
                    rx: Arc::clone(&rx),
                    tx: Arc::clone(&tx),
                };
                let input: DynInputStream = Box::new(LoopbackInputStream {
                    rx,
                    buffer: bytes::Bytes::default(),
                });
                let output: DynOutputStream = Box::new(LoopbackOutputStream { tx, permits });
                Ok((input, output))
            }
            Self::Unspecified { .. } => Err(super::network::socket_error_from_util(
                super::util::ErrorCode::InvalidState,
            )),
        }
    }
}

pub(crate) struct P2TcpStreamingState {
    pub(crate) stream: Arc<tokio::net::TcpStream>,
    reader: Arc<Mutex<TcpReader>>,
    writer: Arc<Mutex<TcpWriter>>,
}

impl P2TcpStreamingState {
    pub(crate) fn shutdown(&self, how: Shutdown) -> SocketResult<()> {
        if let Shutdown::Both | Shutdown::Read = how {
            try_lock_for_socket(&self.reader)?.shutdown();
        }

        if let Shutdown::Both | Shutdown::Write = how {
            try_lock_for_socket(&self.writer)?.shutdown();
        }

        Ok(())
    }
}

struct TcpReader {
    stream: Arc<tokio::net::TcpStream>,
    closed: bool,
}

impl TcpReader {
    fn new(stream: Arc<tokio::net::TcpStream>) -> Self {
        Self {
            stream,
            closed: false,
        }
    }
    fn read(&mut self, size: usize) -> Result<bytes::Bytes, StreamError> {
        if self.closed {
            return Err(StreamError::Closed);
        }
        if size == 0 {
            return Ok(bytes::Bytes::new());
        }

        let mut buf = bytes::BytesMut::with_capacity(size);
        let n = match self.stream.try_read_buf(&mut buf) {
            // A 0-byte read indicates that the stream has closed.
            Ok(0) => {
                self.closed = true;
                return Err(StreamError::Closed);
            }
            Ok(n) => n,

            // Failing with `EWOULDBLOCK` is how we differentiate between a closed channel and no
            // data to read right now.
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => 0,

            Err(e) => {
                self.closed = true;
                return Err(StreamError::LastOperationFailed(e.into()));
            }
        };

        buf.truncate(n);
        Ok(buf.freeze())
    }

    fn shutdown(&mut self) {
        native_shutdown(&self.stream, Shutdown::Read);
        self.closed = true;
    }

    async fn ready(&mut self) {
        if self.closed {
            return;
        }

        let _ = self.stream.readable().await;
    }
}

struct TcpReadStream(Arc<Mutex<TcpReader>>);

#[async_trait::async_trait]
impl InputStream for TcpReadStream {
    fn read(&mut self, size: usize) -> Result<bytes::Bytes, StreamError> {
        try_lock_for_stream(&self.0)?.read(size)
    }
}

#[async_trait::async_trait]
impl Pollable for TcpReadStream {
    async fn ready(&mut self) {
        self.0.lock().await.ready().await
    }
}

const SOCKET_READY_SIZE: usize = 1024 * 1024 * 1024;

struct TcpWriter {
    stream: Arc<tokio::net::TcpStream>,
    state: WriteState,
}

enum WriteState {
    Ready,
    Writing(AbortOnDropJoinHandle<io::Result<()>>),
    Closing(AbortOnDropJoinHandle<io::Result<()>>),
    Closed,
    Error(io::Error),
}

impl TcpWriter {
    fn new(stream: Arc<tokio::net::TcpStream>) -> Self {
        Self {
            stream,
            state: WriteState::Ready,
        }
    }

    fn try_write_portable(stream: &tokio::net::TcpStream, buf: &[u8]) -> io::Result<usize> {
        stream.try_write(buf).map_err(|error| {
            match Errno::from_io_error(&error) {
                // Windows returns `WSAESHUTDOWN` when writing to a shut down socket.
                // We normalize this to EPIPE, because that is what the other platforms return.
                // See: https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-send#:~:text=WSAESHUTDOWN
                #[cfg(windows)]
                Some(Errno::SHUTDOWN) => io::Error::new(io::ErrorKind::BrokenPipe, error),

                _ => error,
            }
        })
    }

    /// Write `bytes` in a background task, remembering the task handle for use in a future call to
    /// `write_ready`
    fn background_write(&mut self, mut bytes: bytes::Bytes) {
        assert!(matches!(self.state, WriteState::Ready));

        let stream = self.stream.clone();
        self.state = WriteState::Writing(wasmtime_wasi::runtime::spawn(async move {
            // Note: we are not using the AsyncWrite impl here, and instead using the TcpStream
            // primitive try_write, which goes directly to attempt a write with mio. This has
            // two advantages: 1. this operation takes a &TcpStream instead of a &mut TcpStream
            // required to AsyncWrite, and 2. it eliminates any buffering in tokio we may need
            // to flush.
            while !bytes.is_empty() {
                stream.writable().await?;
                match Self::try_write_portable(&stream, &bytes) {
                    Ok(n) => {
                        let _ = bytes.split_to(n);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                    Err(e) => return Err(e),
                }
            }

            Ok(())
        }));
    }

    fn write(&mut self, mut bytes: bytes::Bytes) -> Result<(), StreamError> {
        match self.state {
            WriteState::Ready => {}
            WriteState::Closed => return Err(StreamError::Closed),
            WriteState::Writing(_) | WriteState::Closing(_) | WriteState::Error(_) => {
                return Err(StreamError::Trap(wasmtime::format_err!(
                    "unpermitted: must call check_write first"
                )));
            }
        }
        while !bytes.is_empty() {
            match Self::try_write_portable(&self.stream, &bytes) {
                Ok(n) => {
                    let _ = bytes.split_to(n);
                }

                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // As `try_write` indicated that it would have blocked, we'll perform the write
                    // in the background to allow us to return immediately.
                    self.background_write(bytes);

                    return Ok(());
                }

                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {
                    self.state = WriteState::Closed;
                    return Err(StreamError::Closed);
                }

                Err(e) => return Err(StreamError::LastOperationFailed(e.into())),
            }
        }

        Ok(())
    }

    fn flush(&mut self) -> Result<(), StreamError> {
        // `flush` is a no-op here, as we're not managing any internal buffer. Additionally,
        // `write_ready` will join the background write task if it's active, so following `flush`
        // with `write_ready` will have the desired effect.
        match self.state {
            WriteState::Ready
            | WriteState::Writing(_)
            | WriteState::Closing(_)
            | WriteState::Error(_) => Ok(()),
            WriteState::Closed => Err(StreamError::Closed),
        }
    }

    fn check_write(&mut self) -> Result<usize, StreamError> {
        match mem::replace(&mut self.state, WriteState::Closed) {
            WriteState::Writing(task) => {
                self.state = WriteState::Writing(task);
                return Ok(0);
            }
            WriteState::Closing(task) => {
                self.state = WriteState::Closing(task);
                return Ok(0);
            }
            WriteState::Ready => {
                self.state = WriteState::Ready;
            }
            WriteState::Closed => return Err(StreamError::Closed),
            WriteState::Error(e) => return Err(StreamError::LastOperationFailed(e.into())),
        }

        let writable = self.stream.writable();
        futures::pin_mut!(writable);
        if wasmtime_wasi::runtime::poll_noop(writable).is_none() {
            return Ok(0);
        }
        Ok(SOCKET_READY_SIZE)
    }

    fn shutdown(&mut self) {
        self.state = match mem::replace(&mut self.state, WriteState::Closed) {
            // No write in progress, immediately shut down:
            WriteState::Ready => {
                native_shutdown(&self.stream, Shutdown::Write);
                WriteState::Closed
            }

            // Schedule the shutdown after the current write has finished:
            WriteState::Writing(write) => {
                let stream = self.stream.clone();
                WriteState::Closing(wasmtime_wasi::runtime::spawn(async move {
                    let result = write.await;
                    native_shutdown(&stream, Shutdown::Write);
                    result
                }))
            }

            s => s,
        };
    }

    async fn cancel(&mut self) {
        match mem::replace(&mut self.state, WriteState::Closed) {
            WriteState::Writing(task) | WriteState::Closing(task) => _ = task.cancel().await,
            _ => {}
        }
    }

    async fn ready(&mut self) {
        match &mut self.state {
            WriteState::Writing(task) => {
                self.state = match task.await {
                    Ok(()) => WriteState::Ready,
                    Err(e) => WriteState::Error(e),
                }
            }
            WriteState::Closing(task) => {
                self.state = match task.await {
                    Ok(()) => WriteState::Closed,
                    Err(e) => WriteState::Error(e),
                }
            }
            _ => {}
        }

        if let WriteState::Ready = self.state {
            let _ = self.stream.writable().await;
        }
    }
}

struct TcpWriteStream(Arc<Mutex<TcpWriter>>);

#[async_trait::async_trait]
impl OutputStream for TcpWriteStream {
    fn write(&mut self, bytes: bytes::Bytes) -> Result<(), StreamError> {
        try_lock_for_stream(&self.0)?.write(bytes)
    }

    fn flush(&mut self) -> Result<(), StreamError> {
        try_lock_for_stream(&self.0)?.flush()
    }

    fn check_write(&mut self) -> Result<usize, StreamError> {
        try_lock_for_stream(&self.0)?.check_write()
    }

    async fn cancel(&mut self) {
        self.0.lock().await.cancel().await
    }
}

#[async_trait::async_trait]
impl Pollable for TcpWriteStream {
    async fn ready(&mut self) {
        self.0.lock().await.ready().await
    }
}

fn native_shutdown(stream: &tokio::net::TcpStream, how: Shutdown) {
    _ = stream
        .as_socketlike_view::<std::net::TcpStream>()
        .shutdown(how);
}

fn try_lock_for_stream<T>(mutex: &Mutex<T>) -> Result<tokio::sync::MutexGuard<'_, T>, StreamError> {
    mutex
        .try_lock()
        .map_err(|_| StreamError::trap("concurrent access to resource not supported"))
}

fn try_lock_for_socket<T>(mutex: &Mutex<T>) -> SocketResult<tokio::sync::MutexGuard<'_, T>> {
    mutex.try_lock().map_err(|_| {
        SocketError::trap(wasmtime::format_err!(
            "concurrent access to resource not supported"
        ))
    })
}

struct LoopbackInputStream {
    rx: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<LoopbackChannel>>>>,
    buffer: bytes::Bytes,
}

#[async_trait::async_trait]
impl Pollable for LoopbackInputStream {
    async fn ready(&mut self) {
        if !self.buffer.is_empty() {
            return;
        }

        let mut rx = self.rx.lock().await;
        let Some(rx) = rx.as_mut() else {
            return;
        };
        if let Some((buf, _permit)) = rx.recv().await {
            self.buffer = buf;
        };
    }
}

impl InputStream for LoopbackInputStream {
    fn read(&mut self, size: usize) -> Result<bytes::Bytes, StreamError> {
        use tokio::sync::mpsc::error::TryRecvError;

        let Ok(mut rx) = self.rx.try_lock() else {
            return Err(StreamError::Closed);
        };
        let Some(rx) = rx.as_mut() else {
            return Err(StreamError::Closed);
        };
        if rx.is_closed() && rx.is_empty() && self.buffer.is_empty() {
            return Err(StreamError::Closed);
        }

        if size == 0 {
            return Ok(bytes::Bytes::default());
        }
        if !self.buffer.is_empty() {
            let n = self.buffer.len();
            return Ok(self.buffer.split_to(size.min(n)));
        }
        match rx.try_recv() {
            Ok((buf, _permit)) => {
                self.buffer = buf;
                let n = self.buffer.len();
                Ok(self.buffer.split_to(size.min(n)))
            }
            Err(TryRecvError::Empty) => Ok(bytes::Bytes::default()),
            Err(TryRecvError::Disconnected) => Err(StreamError::Closed),
        }
    }
}

struct LoopbackOutputStream {
    tx: Arc<std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<LoopbackChannel>>>>,
    permits: Arc<tokio::sync::Semaphore>,
}

impl LoopbackOutputStream {
    fn is_closed(&self) -> bool {
        let Ok(tx) = self.tx.lock() else {
            return true;
        };
        tx.as_ref().map(|tx| tx.is_closed()).unwrap_or(true)
    }
}

#[async_trait::async_trait]
impl Pollable for LoopbackOutputStream {
    async fn ready(&mut self) {
        _ = self.permits.acquire().await
    }
}

impl OutputStream for LoopbackOutputStream {
    fn write(&mut self, bytes: bytes::Bytes) -> Result<(), StreamError> {
        let mut tx = self
            .tx
            .lock()
            .map_err(|e| StreamError::Trap(wasmtime::format_err!("{e}")))?;
        let Some(tx) = tx.as_mut() else {
            return Err(StreamError::Closed);
        };
        let Some(permit) = bytes.len().try_into().ok().and_then(|n| {
            let permits = Arc::clone(&self.permits);
            permits.try_acquire_many_owned(n).ok()
        }) else {
            return Err(StreamError::Trap(wasmtime::format_err!(
                "write beyond capacity of LoopbackOutputStream"
            )));
        };
        if tx.send((bytes, permit)).is_err() {
            Err(StreamError::Closed)
        } else {
            Ok(())
        }
    }

    fn flush(&mut self) -> Result<(), StreamError> {
        if self.is_closed() {
            Err(StreamError::Closed)
        } else {
            Ok(())
        }
    }

    fn check_write(&mut self) -> Result<usize, StreamError> {
        if self.is_closed() {
            Err(StreamError::Closed)
        } else {
            Ok(self.permits.available_permits())
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::timeout;

    async fn pair() -> (Arc<tokio::net::TcpStream>, tokio::net::TcpStream) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        (Arc::new(client), server)
    }

    #[tokio::test]
    async fn native_reader_distinguishes_pending_data_partial_reads_and_eof() {
        let (stream, mut peer) = pair().await;
        let mut reader = TcpReadStream(Arc::new(Mutex::new(TcpReader::new(stream))));
        assert!(reader.read(0).unwrap().is_empty());
        assert!(reader.read(10).unwrap().is_empty());
        assert!(matches!(
            futures::poll!(Box::pin(reader.ready())),
            std::task::Poll::Pending
        ));
        peer.write_all(b"hello").await.unwrap();
        timeout(Duration::from_secs(5), reader.ready())
            .await
            .unwrap();
        assert_eq!(reader.read(2).unwrap(), &b"he"[..]);
        assert_eq!(reader.read(10).unwrap(), &b"llo"[..]);
        peer.shutdown().await.unwrap();
        timeout(Duration::from_secs(5), async {
            loop {
                reader.ready().await;
                match reader.read(10) {
                    Err(StreamError::Closed) => break,
                    Ok(bytes) => assert!(bytes.is_empty()),
                    Err(error) => panic!("unexpected read error: {error:?}"),
                }
            }
        })
        .await
        .unwrap();
        assert!(matches!(reader.read(0), Err(StreamError::Closed)));
        timeout(Duration::from_secs(5), reader.ready())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn background_write_blocks_new_writes_until_ready() {
        let (stream, mut peer) = pair().await;
        let writer = Arc::new(Mutex::new(TcpWriter::new(stream)));
        let mut output = TcpWriteStream(writer.clone());
        timeout(Duration::from_secs(5), output.ready())
            .await
            .unwrap();
        assert!(output.check_write().unwrap() > 0);
        writer
            .lock()
            .await
            .background_write(Bytes::from_static(b"payload"));
        assert_eq!(output.check_write().unwrap(), 0);
        output.flush().unwrap();
        assert!(matches!(
            output.write(Bytes::from_static(b"extra")),
            Err(StreamError::Trap(_))
        ));
        timeout(Duration::from_secs(5), output.ready())
            .await
            .unwrap();
        assert!(output.check_write().unwrap() > 0);
        let mut buf = [0; 7];
        timeout(Duration::from_secs(5), peer.read_exact(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buf, b"payload");
        output.cancel().await;
        assert!(matches!(
            output.write(Bytes::new()),
            Err(StreamError::Closed)
        ));
        assert!(matches!(output.flush(), Err(StreamError::Closed)));
    }

    #[tokio::test]
    async fn shutdown_drains_the_background_write_before_eof() {
        let (stream, mut peer) = pair().await;
        let mut writer = TcpWriter::new(stream);
        writer.background_write(Bytes::from_static(b"last message"));
        writer.shutdown();
        assert_eq!(writer.check_write().unwrap(), 0);
        writer.shutdown();
        timeout(Duration::from_secs(5), writer.ready())
            .await
            .unwrap();
        let mut received = Vec::new();
        timeout(Duration::from_secs(5), peer.read_to_end(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, b"last message");
        assert!(matches!(writer.check_write(), Err(StreamError::Closed)));
    }

    #[tokio::test]
    async fn failed_background_write_reports_its_error_once() {
        let (stream, _peer) = pair().await;
        let mut writer = TcpWriter::new(stream);
        writer.state = WriteState::Writing(wasmtime_wasi::runtime::spawn(async {
            Err(io::Error::from(io::ErrorKind::ConnectionReset))
        }));
        timeout(Duration::from_secs(5), writer.ready())
            .await
            .unwrap();
        writer.flush().unwrap();
        assert!(matches!(
            writer.check_write(),
            Err(StreamError::LastOperationFailed(_))
        ));
        assert!(matches!(writer.check_write(), Err(StreamError::Closed)));
    }

    #[tokio::test]
    async fn cancel_aborts_a_pending_background_write() {
        let (stream, _peer) = pair().await;
        let mut writer = TcpWriter::new(stream);
        writer.state = WriteState::Writing(wasmtime_wasi::runtime::spawn(std::future::pending()));
        timeout(Duration::from_secs(5), writer.cancel())
            .await
            .unwrap();
        assert!(matches!(writer.check_write(), Err(StreamError::Closed)));
        timeout(Duration::from_secs(5), writer.ready())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn concurrent_native_stream_access_traps_instead_of_blocking() {
        let (stream, _peer) = pair().await;
        let reader = Arc::new(Mutex::new(TcpReader::new(stream.clone())));
        let writer = Arc::new(Mutex::new(TcpWriter::new(stream.clone())));
        let state = P2TcpStreamingState {
            stream,
            reader: reader.clone(),
            writer: writer.clone(),
        };
        let mut input = TcpReadStream(reader.clone());
        let mut output = TcpWriteStream(writer.clone());
        let read_guard = reader.lock().await;
        assert!(matches!(input.read(1), Err(StreamError::Trap(_))));
        assert!(
            state
                .shutdown(Shutdown::Read)
                .unwrap_err()
                .downcast()
                .is_err()
        );
        drop(read_guard);
        let write_guard = writer.lock().await;
        assert!(matches!(output.check_write(), Err(StreamError::Trap(_))));
        assert!(matches!(output.flush(), Err(StreamError::Trap(_))));
        assert!(matches!(
            output.write(Bytes::new()),
            Err(StreamError::Trap(_))
        ));
        assert!(
            state
                .shutdown(Shutdown::Write)
                .unwrap_err()
                .downcast()
                .is_err()
        );
        drop(write_guard);
        state.shutdown(Shutdown::Both).unwrap();
        assert!(matches!(input.read(1), Err(StreamError::Closed)));
        assert!(matches!(output.check_write(), Err(StreamError::Closed)));
    }
}
