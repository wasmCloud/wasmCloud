use super::{SocketAddrCheck, SocketAddrUse};
use std::net::SocketAddr;
use std::sync::Arc;
use wasmtime_wasi::TrappableError;
use wasmtime_wasi::p2::bindings::sockets::network::ErrorCode;

pub type SocketResult<T> = Result<T, SocketError>;

pub type SocketError = TrappableError<ErrorCode>;

pub(crate) fn socket_error_from_io(error: std::io::Error) -> SocketError {
    error_code_from_io(error.kind()).into()
}

pub(crate) fn socket_error_from_util(error: super::util::ErrorCode) -> SocketError {
    error_code_from_util(error).into()
}

pub(crate) fn error_code_from_util(error: super::util::ErrorCode) -> ErrorCode {
    match error {
        super::util::ErrorCode::Unknown => ErrorCode::Unknown,
        super::util::ErrorCode::AccessDenied => ErrorCode::AccessDenied,
        super::util::ErrorCode::NotSupported => ErrorCode::NotSupported,
        super::util::ErrorCode::InvalidArgument => ErrorCode::InvalidArgument,
        super::util::ErrorCode::OutOfMemory => ErrorCode::OutOfMemory,
        super::util::ErrorCode::Timeout => ErrorCode::Timeout,
        super::util::ErrorCode::InvalidState => ErrorCode::InvalidState,
        super::util::ErrorCode::AddressNotBindable => ErrorCode::AddressNotBindable,
        super::util::ErrorCode::AddressInUse => ErrorCode::AddressInUse,
        super::util::ErrorCode::RemoteUnreachable => ErrorCode::RemoteUnreachable,
        super::util::ErrorCode::ConnectionRefused => ErrorCode::ConnectionRefused,
        super::util::ErrorCode::ConnectionReset => ErrorCode::ConnectionReset,
        super::util::ErrorCode::ConnectionAborted => ErrorCode::ConnectionAborted,
        super::util::ErrorCode::DatagramTooLarge => ErrorCode::DatagramTooLarge,
        super::util::ErrorCode::NotInProgress => ErrorCode::NotInProgress,
        super::util::ErrorCode::ConcurrencyConflict => ErrorCode::ConcurrencyConflict,
    }
}

pub(crate) fn error_code_from_io(error: std::io::ErrorKind) -> ErrorCode {
    match error {
        std::io::ErrorKind::AddrInUse => ErrorCode::AddressInUse,
        std::io::ErrorKind::AddrNotAvailable => ErrorCode::AddressNotBindable,
        std::io::ErrorKind::ConnectionAborted => ErrorCode::ConnectionAborted,
        std::io::ErrorKind::ConnectionRefused => ErrorCode::ConnectionRefused,
        std::io::ErrorKind::ConnectionReset => ErrorCode::ConnectionReset,
        std::io::ErrorKind::InvalidInput => ErrorCode::InvalidArgument,
        std::io::ErrorKind::OutOfMemory => ErrorCode::OutOfMemory,
        std::io::ErrorKind::PermissionDenied => ErrorCode::AccessDenied,
        std::io::ErrorKind::TimedOut => ErrorCode::Timeout,
        std::io::ErrorKind::Unsupported => ErrorCode::NotSupported,
        _ => ErrorCode::Unknown,
    }
}

pub struct Network {
    pub(crate) socket_addr_check: SocketAddrCheck,
    pub(crate) allowed_names: Arc<[crate::host::allowed_names::AllowedName]>,
}

impl Network {
    pub async fn check_socket_addr(
        &self,
        addr: SocketAddr,
        reason: SocketAddrUse,
    ) -> std::io::Result<()> {
        self.socket_addr_check.check(addr, reason).await
    }
}
