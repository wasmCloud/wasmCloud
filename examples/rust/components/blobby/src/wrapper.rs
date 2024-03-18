// This is adapted from `wasmcloud-actor` but instead is impl'd directly on the type. This is
// something we should probably make part of wit-bindgen or something else since this will be a
// common thing people will need in Rust

use std::io;
use std::io::Read;

use crate::wasi::io::streams::StreamError;

impl Read for crate::wasi::io::streams::InputStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = buf
            .len()
            .try_into()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        match self.blocking_read(n) {
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

// NOTE(thomastaylor312): This is soooo weird. So for some reason if I make sure to explicitly call
// write (crate::wasi::io::streams::OutputStream::write), it never writes the bytes successfully,
// but if we use the wrapper below (copied directly from `wasmcloud-actor`), it works. To make it
// even worse, when we were using it to write to the http response, it worked, but not for the
// blobstore. There is probably some sort of simple thing that could fix this but I don't know what.

// impl Write for crate::wasi::io::streams::OutputStream {
//     fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
//         let n = match self.check_write().map(std::num::NonZeroU64::new) {
//             Ok(Some(n)) => n,
//             Ok(None) | Err(StreamError::Closed) => return Ok(0),
//             Err(StreamError::LastOperationFailed(e)) => {
//                 return Err(io::Error::new(io::ErrorKind::Other, e.to_debug_string()))
//             }
//         };
//         let n = n
//             .get()
//             .try_into()
//             .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
//         let n = buf.len().min(n);
//         log(Level::Info, "write", &format!("n: {}", n));
//         crate::wasi::io::streams::OutputStream::write(self.borrow(), &buf[..n]).map_err(
//             |e| match e {
//                 StreamError::Closed => io::ErrorKind::UnexpectedEof.into(),
//                 StreamError::LastOperationFailed(e) => {
//                     io::Error::new(io::ErrorKind::Other, e.to_debug_string())
//                 }
//             },
//         )?;
//         Ok(n)
//     }

//     fn flush(&mut self) -> std::io::Result<()> {
//         self.blocking_flush()
//             .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
//     }
// }

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
