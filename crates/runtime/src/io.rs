use core::pin::Pin;
use core::task::{Context, Poll};

use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::{Arc, MutexGuard};

use futures::{Stream, StreamExt as _};
use tokio::io::{AsyncRead, AsyncSeek, AsyncWrite};

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

/// Incoming value [`Stream`] wrapper, which buffers the chunks and flattens them
pub struct BufferedIncomingStream<T> {
    stream: Pin<Box<dyn Stream<Item = Vec<T>> + Send>>,
    buffer: Vec<T>,
}

impl<T> BufferedIncomingStream<T> {
    /// Create a new [`BufferedIncomingStream`]
    #[must_use]
    pub fn new(stream: Pin<Box<dyn Stream<Item = Vec<T>> + Send>>) -> Self {
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
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.buffer.is_empty() {
            match self.stream.poll_next_unpin(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Ready(Some(mut values)) => match values.len() {
                    0 => Poll::Ready(None),
                    1 => {
                        let item = values.pop().expect("element missing");
                        Poll::Ready(Some(item))
                    }
                    _ => {
                        self.buffer = values.split_off(1);
                        let item = values.pop().expect("element missing");
                        assert!(values.is_empty());
                        Poll::Ready(Some(item))
                    }
                },
            }
        } else {
            let tail = self.buffer.split_off(1);
            let item = self.buffer.pop().expect("element missing");
            assert!(self.buffer.is_empty());
            self.buffer = tail;
            Poll::Ready(Some(item))
        }
    }
}
