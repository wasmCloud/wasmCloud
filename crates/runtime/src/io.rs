use core::pin::Pin;
use core::task::{Context, Poll};

use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::{Arc, MutexGuard};

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
