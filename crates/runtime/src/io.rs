use core::pin::Pin;
use core::task::{Context, Poll};

use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::{Arc, MutexGuard};

use bytes::Bytes;
use futures::{Stream, StreamExt as _};
use tokio::io::{AsyncRead, AsyncSeek, AsyncWrite, ReadBuf};
use wrpc_transport::IncomingInputStream;

/// wasmCloud I/O functionality

#[derive(Clone, Default)]
pub struct AsyncVec(Arc<std::sync::Mutex<Cursor<Vec<u8>>>>);

impl AsyncVec {
    fn inner(&self) -> std::io::Result<MutexGuard<Cursor<Vec<u8>>>> {
        self.0
            .lock()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    }
}

impl From<Vec<u8>> for AsyncVec {
    fn from(buf: Vec<u8>) -> Self {
        Self(Arc::new(std::sync::Mutex::new(Cursor::new(buf))))
    }
}

impl Read for AsyncVec {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut inner = self.inner()?;
        Read::read(&mut *inner, buf)
    }
}

impl Seek for AsyncVec {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let mut inner = self.inner()?;
        inner.seek(pos)
    }
}

impl AsyncWrite for AsyncVec {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let mut inner = self.inner()?;
        Pin::new(&mut *inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let mut inner = self.inner()?;
        Pin::new(&mut *inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let mut inner = self.inner()?;
        Pin::new(&mut *inner).poll_shutdown(cx)
    }
}

impl AsyncRead for AsyncVec {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut inner = self.inner()?;
        Pin::new(&mut *inner).poll_read(cx, buf)
    }
}

impl AsyncSeek for AsyncVec {
    fn start_seek(self: Pin<&mut Self>, position: std::io::SeekFrom) -> std::io::Result<()> {
        let mut inner = self.inner()?;
        Pin::new(&mut *inner).start_seek(position)
    }

    fn poll_complete(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<u64>> {
        let mut inner = self.inner()?;
        Pin::new(&mut *inner).poll_complete(cx)
    }
}

/// [`AsyncRead`] adapter for [`IncomingInputStream`]
pub struct IncomingInputStreamReader {
    stream: IncomingInputStream,
    buffer: Bytes,
}

impl IncomingInputStreamReader {
    /// Create a new [`IncomingInputStreamReader`]
    #[must_use]
    pub fn new(stream: IncomingInputStream) -> Self {
        Self {
            stream,
            buffer: Bytes::default(),
        }
    }
}

impl AsyncRead for IncomingInputStreamReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.buffer.len() {
            0 => match self.stream.poll_next_unpin(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(None) => Poll::Ready(Ok(())),
                Poll::Ready(Some(Ok(mut data))) => {
                    let cap = buf.remaining();
                    if data.len() > cap {
                        self.buffer = data.split_off(cap);
                    }
                    buf.put_slice(&data);
                    Poll::Ready(Ok(()))
                }
                Poll::Ready(Some(Err(err))) => {
                    Poll::Ready(Err(std::io::Error::new(std::io::ErrorKind::Other, err)))
                }
            },
            buffered => {
                let cap = buf.remaining();
                if buffered > cap {
                    let tail = self.buffer.split_off(cap);
                    buf.put_slice(&self.buffer);
                    self.buffer = tail;
                } else {
                    buf.put_slice(&self.buffer);
                    self.buffer.clear();
                }
                Poll::Ready(Ok(()))
            }
        }
    }
}

/// Incoming value [`Stream`] wrapper, which buffers the chunks and flattens them
pub struct BufferedIncomingStream<T> {
    stream: Box<dyn Stream<Item = anyhow::Result<Vec<T>>> + Send + Sync + Unpin>,
    buffer: Vec<T>,
}

impl<T> BufferedIncomingStream<T> {
    /// Create a new [`BufferedIncomingStream`]
    #[must_use]
    pub fn new(
        stream: Box<dyn Stream<Item = anyhow::Result<Vec<T>>> + Send + Sync + Unpin>,
    ) -> Self {
        Self {
            stream,
            buffer: Vec::default(),
        }
    }
}

impl<T> Stream for BufferedIncomingStream<T>
where
    T: Unpin,
{
    type Item = anyhow::Result<T>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.buffer.is_empty() {
            match self.stream.poll_next_unpin(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Ready(Some(Ok(mut values))) => match values.len() {
                    0 => Poll::Ready(None),
                    1 => {
                        let item = values.pop().expect("element missing");
                        Poll::Ready(Some(Ok(item)))
                    }
                    _ => {
                        self.buffer = values.split_off(1);
                        let item = values.pop().expect("element missing");
                        assert!(values.is_empty());
                        Poll::Ready(Some(Ok(item)))
                    }
                },
                Poll::Ready(Some(Err(err))) => Poll::Ready(Some(Err(err))),
            }
        } else {
            let tail = self.buffer.split_off(1);
            let item = self.buffer.pop().expect("element missing");
            assert!(self.buffer.is_empty());
            self.buffer = tail;
            Poll::Ready(Some(Ok(item)))
        }
    }
}
