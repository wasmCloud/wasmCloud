use std::io::{Read, Write};

pub struct InputStreamReader<'a> {
    stream: &'a mut crate::wasi::io::streams::InputStream,
}

impl<'a> From<&'a mut crate::wasi::io::streams::InputStream> for InputStreamReader<'a> {
    fn from(stream: &'a mut crate::wasi::io::streams::InputStream) -> Self {
        Self { stream }
    }
}

impl std::io::Read for InputStreamReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use crate::wasi::io::streams::StreamError;
        use std::io;

        let n = buf
            .len()
            .try_into()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        match self.stream.blocking_read(n) {
            Ok(chunk) => {
                let n = chunk.len();
                if n > buf.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        "more bytes read than requested",
                    ));
                }
                buf[..n].copy_from_slice(&chunk);
                Ok(n)
            }
            Err(StreamError::Closed) => Ok(0),
            Err(StreamError::LastOperationFailed(e)) => {
                Err(io::Error::new(io::ErrorKind::Other, e.to_debug_string()))
            }
        }
    }
}

pub struct OutputStreamWriter<'a> {
    stream: &'a mut crate::wasi::io::streams::OutputStream,
}

impl<'a> From<&'a mut crate::wasi::io::streams::OutputStream> for OutputStreamWriter<'a> {
    fn from(stream: &'a mut crate::wasi::io::streams::OutputStream) -> Self {
        Self { stream }
    }
}

impl std::io::Write for OutputStreamWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        use crate::wasi::io::streams::StreamError;
        use std::io;

        let n = match self.stream.check_write().map(std::num::NonZeroU64::new) {
            Ok(Some(n)) => n,
            Ok(None) | Err(StreamError::Closed) => return Ok(0),
            Err(StreamError::LastOperationFailed(e)) => {
                return Err(io::Error::new(io::ErrorKind::Other, e.to_debug_string()))
            }
        };
        let n = n
            .get()
            .try_into()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        let n = buf.len().min(n);
        self.stream.write(&buf[..n]).map_err(|e| match e {
            StreamError::Closed => io::ErrorKind::UnexpectedEof.into(),
            StreamError::LastOperationFailed(e) => {
                io::Error::new(io::ErrorKind::Other, e.to_debug_string())
            }
        })?;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream
            .blocking_flush()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
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
