use core::future::Future;
use core::ops::Deref;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use wasmtime::component::{HasData, ResourceTable};

pub(crate) mod host_instance_network;
pub(crate) mod host_ip_name_lookup;
pub(crate) mod host_network;
pub(crate) mod host_tcp;
pub(crate) mod host_tcp_create_socket;
pub(crate) mod host_udp;
pub(crate) mod host_udp_create_socket;
pub mod loopback;
pub(crate) mod network;
pub(crate) mod p2_tcp;
pub(crate) mod p2_udp;
pub(crate) mod tcp;
pub(crate) mod udp;
pub(crate) mod util;

#[cfg(feature = "wasip3")]
pub(crate) mod host_ip_name_lookup_p3;
#[cfg(feature = "wasip3")]
pub(crate) mod host_tcp_p3;
#[cfg(feature = "wasip3")]
pub(crate) mod host_udp_p3;

pub use tcp::TcpSocket;
pub use udp::UdpSocket;

/// A helper struct which implements [`HasData`] for the `wasi:sockets` APIs.
pub struct WasiSockets;

impl HasData for WasiSockets {
    type Data<'a> = WasiSocketsCtxView<'a>;
}

/// Value taken from rust std library.
pub(crate) const DEFAULT_TCP_BACKLOG: u32 = 128;

/// Theoretical maximum byte size of a UDP datagram, the real limit is lower,
/// but we do not account for e.g. the transport layer here for simplicity.
/// In practice, datagrams are typically less than 1500 bytes.
pub(crate) const MAX_UDP_DATAGRAM_SIZE: usize = u16::MAX as usize;

#[derive(Default)]
pub struct WasiSocketsCtx {
    pub(crate) socket_addr_check: SocketAddrCheck,
    pub(crate) allowed_network_uses: AllowedNetworkUses,
    pub(crate) loopback: Arc<std::sync::Mutex<loopback::Network>>,
}

pub struct WasiSocketsCtxView<'a> {
    pub ctx: &'a mut WasiSocketsCtx,
    pub table: &'a mut ResourceTable,
}

pub trait WasiSocketsView: Send {
    fn sockets(&mut self) -> WasiSocketsCtxView<'_>;
}

#[derive(Copy, Clone)]
pub(crate) struct AllowedNetworkUses {
    pub(crate) ip_name_lookup: bool,
    pub(crate) udp: bool,
    pub(crate) tcp: bool,
}

impl Default for AllowedNetworkUses {
    fn default() -> Self {
        Self {
            ip_name_lookup: false,
            udp: true,
            tcp: true,
        }
    }
}

impl AllowedNetworkUses {
    pub(crate) fn check_allowed_udp(self) -> std::io::Result<()> {
        if !self.udp {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "UDP is not allowed",
            ));
        }

        Ok(())
    }

    pub(crate) fn check_allowed_tcp(self) -> std::io::Result<()> {
        if !self.tcp {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "TCP is not allowed",
            ));
        }

        Ok(())
    }
}

type SocketAddrCheckFn = dyn Fn(SocketAddr, SocketAddrUse) -> Pin<Box<dyn Future<Output = bool> + Send + Sync>>
    + Send
    + Sync;

/// A check that will be called for each socket address that is used of whether the address is permitted.
#[derive(Clone)]
pub(crate) struct SocketAddrCheck(Arc<SocketAddrCheckFn>);

impl SocketAddrCheck {
    pub(crate) fn new(
        f: impl Fn(SocketAddr, SocketAddrUse) -> Pin<Box<dyn Future<Output = bool> + Send + Sync>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self(Arc::new(f))
    }

    pub(crate) async fn check(
        &self,
        addr: SocketAddr,
        reason: SocketAddrUse,
    ) -> std::io::Result<()> {
        if (self.0)(addr, reason).await {
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "An address was not permitted by the socket address check.",
            ))
        }
    }
}

impl Deref for SocketAddrCheck {
    type Target = SocketAddrCheckFn;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl Default for SocketAddrCheck {
    fn default() -> Self {
        Self(Arc::new(|_, _| Box::pin(async { false })))
    }
}

/// The reason what a socket address is being used for.
#[derive(Clone, Copy, Debug)]
pub enum SocketAddrUse {
    /// Binding TCP socket
    TcpBind,
    /// Connecting TCP socket
    TcpConnect,
    /// Binding UDP socket
    UdpBind,
    /// Connecting UDP socket
    UdpConnect,
    /// Sending datagram on non-connected UDP socket
    UdpOutgoingDatagram,
}

/// Convert our custom `util::ErrorCode` to the P3 bindings `ErrorCode`.
#[cfg(feature = "wasip3")]
pub(crate) fn p3_error_code_from_util(
    error: util::ErrorCode,
) -> wasmtime_wasi::p3::bindings::sockets::types::ErrorCode {
    use wasmtime_wasi::p3::bindings::sockets::types::ErrorCode as P3ErrorCode;
    match error {
        util::ErrorCode::Unknown => P3ErrorCode::Other(None),
        util::ErrorCode::AccessDenied => P3ErrorCode::AccessDenied,
        util::ErrorCode::NotSupported => P3ErrorCode::NotSupported,
        util::ErrorCode::InvalidArgument => P3ErrorCode::InvalidArgument,
        util::ErrorCode::OutOfMemory => P3ErrorCode::OutOfMemory,
        util::ErrorCode::Timeout => P3ErrorCode::Timeout,
        util::ErrorCode::InvalidState => P3ErrorCode::InvalidState,
        util::ErrorCode::AddressNotBindable => P3ErrorCode::AddressNotBindable,
        util::ErrorCode::AddressInUse => P3ErrorCode::AddressInUse,
        util::ErrorCode::RemoteUnreachable => P3ErrorCode::RemoteUnreachable,
        util::ErrorCode::ConnectionRefused => P3ErrorCode::ConnectionRefused,
        util::ErrorCode::ConnectionReset => P3ErrorCode::ConnectionReset,
        util::ErrorCode::ConnectionAborted => P3ErrorCode::ConnectionAborted,
        util::ErrorCode::DatagramTooLarge => P3ErrorCode::DatagramTooLarge,
        util::ErrorCode::NotInProgress => P3ErrorCode::InvalidState,
        util::ErrorCode::ConcurrencyConflict => P3ErrorCode::InvalidState,
    }
}

/// Convert our `util::ErrorCode` to a P3 `SocketError` (TrappableError).
#[cfg(feature = "wasip3")]
pub(crate) fn p3_socket_error_from_util(
    error: util::ErrorCode,
) -> wasmtime_wasi::p3::sockets::SocketError {
    p3_error_code_from_util(error).into()
}

/// Register P3 socket interfaces with the linker using our custom socket implementation.
#[cfg(feature = "wasip3")]
pub fn add_p3_to_linker(
    linker: &mut wasmtime::component::Linker<crate::engine::ctx::SharedCtx>,
) -> anyhow::Result<()> {
    use wasmtime_wasi::p3::bindings::sockets::{ip_name_lookup, types};
    ip_name_lookup::add_to_linker::<_, WasiSockets>(linker, crate::engine::ctx::extract_sockets)?;
    types::add_to_linker::<_, WasiSockets>(linker, crate::engine::ctx::extract_sockets)?;
    Ok(())
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum SocketAddressFamily {
    Ipv4,
    Ipv6,
}

#[cfg(feature = "wasip3")]
impl From<SocketAddressFamily> for wasmtime_wasi::p3::bindings::sockets::types::IpAddressFamily {
    fn from(family: SocketAddressFamily) -> Self {
        match family {
            SocketAddressFamily::Ipv4 => Self::Ipv4,
            SocketAddressFamily::Ipv6 => Self::Ipv6,
        }
    }
}

#[cfg(all(test, feature = "wasip3"))]
mod tests_p3 {
    use super::*;

    #[test]
    fn test_p3_error_code_maps_all_variants() {
        use wasmtime_wasi::p3::bindings::sockets::types::ErrorCode as P3;

        assert!(matches!(
            p3_error_code_from_util(util::ErrorCode::AccessDenied),
            P3::AccessDenied
        ));
        assert!(matches!(
            p3_error_code_from_util(util::ErrorCode::NotSupported),
            P3::NotSupported
        ));
        assert!(matches!(
            p3_error_code_from_util(util::ErrorCode::InvalidArgument),
            P3::InvalidArgument
        ));
        assert!(matches!(
            p3_error_code_from_util(util::ErrorCode::OutOfMemory),
            P3::OutOfMemory
        ));
        assert!(matches!(
            p3_error_code_from_util(util::ErrorCode::Timeout),
            P3::Timeout
        ));
        assert!(matches!(
            p3_error_code_from_util(util::ErrorCode::InvalidState),
            P3::InvalidState
        ));
        assert!(matches!(
            p3_error_code_from_util(util::ErrorCode::AddressNotBindable),
            P3::AddressNotBindable
        ));
        assert!(matches!(
            p3_error_code_from_util(util::ErrorCode::AddressInUse),
            P3::AddressInUse
        ));
        assert!(matches!(
            p3_error_code_from_util(util::ErrorCode::RemoteUnreachable),
            P3::RemoteUnreachable
        ));
        assert!(matches!(
            p3_error_code_from_util(util::ErrorCode::ConnectionRefused),
            P3::ConnectionRefused
        ));
        assert!(matches!(
            p3_error_code_from_util(util::ErrorCode::ConnectionReset),
            P3::ConnectionReset
        ));
        assert!(matches!(
            p3_error_code_from_util(util::ErrorCode::ConnectionAborted),
            P3::ConnectionAborted
        ));
        assert!(matches!(
            p3_error_code_from_util(util::ErrorCode::DatagramTooLarge),
            P3::DatagramTooLarge
        ));
    }

    #[test]
    fn test_p3_error_code_maps_p2_only_variants_to_invalidstate() {
        use wasmtime_wasi::p3::bindings::sockets::types::ErrorCode as P3;

        // P3 collapsed NotInProgress and ConcurrencyConflict into InvalidState
        assert!(matches!(
            p3_error_code_from_util(util::ErrorCode::NotInProgress),
            P3::InvalidState
        ));
        assert!(matches!(
            p3_error_code_from_util(util::ErrorCode::ConcurrencyConflict),
            P3::InvalidState
        ));
    }

    #[test]
    fn test_p3_error_code_maps_unknown_to_other() {
        use wasmtime_wasi::p3::bindings::sockets::types::ErrorCode as P3;

        assert!(matches!(
            p3_error_code_from_util(util::ErrorCode::Unknown),
            P3::Other(None)
        ));
    }

    #[test]
    fn test_p3_socket_error_from_util_converts() {
        // Just verify it doesn't panic and produces a SocketError
        let err = p3_socket_error_from_util(util::ErrorCode::ConnectionRefused);
        let code = err.downcast().expect("should downcast to ErrorCode");
        assert!(matches!(
            code,
            wasmtime_wasi::p3::bindings::sockets::types::ErrorCode::ConnectionRefused
        ));
    }
}
