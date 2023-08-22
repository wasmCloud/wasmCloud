use std::io::{Read, Write};

#[cfg(all(not(feature = "module"), feature = "component"))]
pub struct InputStreamReader {
    stream: crate::wasi::io::streams::InputStream,
    status: Option<crate::wasi::io::streams::StreamStatus>,
}

#[cfg(all(not(feature = "module"), feature = "component"))]
impl From<crate::wasi::io::streams::InputStream> for InputStreamReader {
    fn from(stream: crate::wasi::io::streams::InputStream) -> Self {
        Self {
            stream,
            status: None,
        }
    }
}

#[cfg(all(not(feature = "module"), feature = "component"))]
impl std::io::Read for InputStreamReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(crate::wasi::io::streams::StreamStatus::Ended) = self.status {
            return Ok(0);
        }
        let n = buf
            .len()
            .try_into()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        let (chunk, status) = crate::wasi::io::streams::blocking_read(self.stream, n)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        self.status = Some(status);

        let n = chunk.len();
        if n > buf.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "more bytes read than requested",
            ));
        }
        buf[..n].copy_from_slice(&chunk);
        Ok(n)
    }
}

#[cfg(all(not(feature = "module"), feature = "component"))]
pub struct OutputStreamWriter {
    stream: crate::wasi::io::streams::OutputStream,
    status: Option<crate::wasi::io::streams::StreamStatus>,
}

#[cfg(all(not(feature = "module"), feature = "component"))]
impl From<crate::wasi::io::streams::OutputStream> for OutputStreamWriter {
    fn from(stream: crate::wasi::io::streams::OutputStream) -> Self {
        Self {
            stream,
            status: None,
        }
    }
}

#[cfg(all(not(feature = "module"), feature = "component"))]
impl std::io::Write for OutputStreamWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(crate::wasi::io::streams::StreamStatus::Ended) = self.status {
            return Ok(0);
        }
        let (n, status) = crate::wasi::io::streams::blocking_write(self.stream, buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        self.status = Some(status);
        let n = n
            .try_into()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        if n > buf.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "more bytes written than requested",
            ));
        }
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // not supported
        Ok(())
    }
}

pub struct StdioStream<'a> {
    stdin: std::io::StdinLock<'a>,
    stdout: std::io::StdoutLock<'a>,
}

impl StdioStream<'_> {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Read for StdioStream<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.stdin.read(buf)
    }
}

impl Write for StdioStream<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stdout.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stdout.flush()
    }
}

impl Default for StdioStream<'_> {
    fn default() -> Self {
        Self {
            stdin: std::io::stdin().lock(),
            stdout: std::io::stdout().lock(),
        }
    }
}

#[cfg(feature = "futures")]
impl futures::AsyncRead for StdioStream<'_> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::task::Poll::Ready(self.stdin.read(buf))
    }
}

#[cfg(feature = "futures")]
impl futures::AsyncWrite for StdioStream<'_> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::task::Poll::Ready(self.stdout.write(buf))
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(self.stdout.flush())
    }

    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.poll_flush(cx)
    }
}

#[cfg(feature = "tokio")]
impl tokio::io::AsyncRead for StdioStream<'_> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let mut fill = vec![0; buf.capacity()];
        std::task::Poll::Ready({
            let n = self.stdin.read(&mut fill)?;
            buf.put_slice(&fill[..n]);
            Ok(())
        })
    }
}

#[cfg(feature = "tokio")]
impl tokio::io::AsyncWrite for StdioStream<'_> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        std::task::Poll::Ready(self.stdout.write(buf))
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::task::Poll::Ready(self.stdout.flush())
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        self.poll_flush(cx)
    }
}
