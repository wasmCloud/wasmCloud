//! Custom error types
use std::error::Error as StdError;
use std::fmt;

#[derive(Debug)]
pub struct Error(Box<ErrorKind>);

pub fn new(kind: ErrorKind) -> Box<dyn ::std::error::Error + Sync + Send> {
    Box::new(Error(Box::new(kind)))
}

#[derive(Debug)]
pub enum ErrorKind {
    Wapc(wapc::errors::Error),
    HostCallFailure(Box<dyn StdError + Send + Sync>),
    Wascap(wascap::Error),
    Authorization(String),
    IO(std::io::Error),
    MiscHost(String),
    Plugin(libloading::Error),
}

impl StdError for Error {
    fn description(&self) -> &str {
        match *self.0 {
            ErrorKind::Wapc(_) => "waPC error",
            ErrorKind::IO(_) => "I/O error",
            ErrorKind::HostCallFailure(_) => "Error occurred during host call",
            ErrorKind::Wascap(_) => "Embedded JWT Failure",
            ErrorKind::Authorization(_) => "Module authorization failure",
            ErrorKind::MiscHost(_) => "Wasmcloud Host error",
            ErrorKind::Plugin(_) => "Plugin error",
        }
    }

    fn cause(&self) -> Option<&dyn StdError> {
        match *self.0 {
            ErrorKind::Wapc(ref err) => Some(err),
            ErrorKind::HostCallFailure(_) => None,
            ErrorKind::Wascap(ref err) => Some(err),
            ErrorKind::Authorization(_) => None,
            ErrorKind::IO(ref err) => Some(err),
            ErrorKind::MiscHost(_) => None,
            ErrorKind::Plugin(ref err) => Some(err),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self.0 {
            ErrorKind::Wapc(ref err) => write!(f, "waPC failure: {}", err),
            ErrorKind::HostCallFailure(ref err) => {
                write!(f, "Error occurred during host call: {}", err)
            }
            ErrorKind::Wascap(ref err) => write!(f, "Embedded JWT failure: {}", err),
            ErrorKind::Authorization(ref err) => {
                write!(f, "WebAssembly module authorization failure: {}", err)
            }
            ErrorKind::IO(ref err) => write!(f, "I/O error: {}", err),
            ErrorKind::MiscHost(ref err) => write!(f, "Wasmcloud Host Error: {}", err),
            ErrorKind::Plugin(ref err) => write!(f, "Plugin error: {}", err),
        }
    }
}

impl From<libloading::Error> for Error {
    fn from(source: libloading::Error) -> Error {
        Error(Box::new(ErrorKind::Plugin(source)))
    }
}
impl From<wascap::Error> for Error {
    fn from(source: wascap::Error) -> Error {
        Error(Box::new(ErrorKind::Wascap(source)))
    }
}

impl From<wapc::errors::Error> for Error {
    fn from(source: wapc::errors::Error) -> Error {
        Error(Box::new(ErrorKind::Wapc(source)))
    }
}

impl From<std::io::Error> for Error {
    fn from(source: std::io::Error) -> Error {
        Error(Box::new(ErrorKind::IO(source)))
    }
}

impl From<Box<dyn StdError + Send + Sync>> for Error {
    fn from(source: Box<dyn StdError + Send + Sync>) -> Error {
        Error(Box::new(ErrorKind::HostCallFailure(source)))
    }
}

impl From<String> for Error {
    fn from(source: String) -> Error {
        Error(Box::new(ErrorKind::MiscHost(source)))
    }
}

#[cfg(test)]
mod tests {
    fn assert_sync_send<T: Send + Sync>() {}
    const _: fn() = || assert_sync_send::<super::Error>();
}
