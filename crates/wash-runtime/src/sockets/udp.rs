#![allow(unsafe_code)] // Socket operations require unsafe

use super::util::{
    ErrorCode, get_unicast_hop_limit, is_valid_address_family, is_valid_remote_address,
    receive_buffer_size, send_buffer_size, set_receive_buffer_size, set_send_buffer_size,
    set_unicast_hop_limit, udp_bind, udp_connect, udp_disconnect, udp_socket,
};
use super::{SocketAddrCheck, SocketAddressFamily, WasiSocketsCtx};

use cap_net_ext::AddressFamily;
use io_lifetimes::AsSocketlike as _;
use io_lifetimes::raw::{FromRawSocketlike as _, IntoRawSocketlike as _};
use rustix::io::Errno;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::debug;

/// Inline replacement for `with_ambient_tokio_runtime` -- we always run inside tokio.
fn with_ambient_tokio_runtime<R>(f: impl FnOnce() -> R) -> R {
    f()
}

/// The state of a UDP socket.
///
/// This represents the various states a socket can be in during the
/// activities of binding, and connecting.
#[derive(Clone)]
enum UdpState {
    /// The initial state for a newly-created socket.
    Default,

    /// A `bind` operation has started but has yet to complete with
    /// `finish_bind`.
    BindStarted,

    /// Binding finished via `finish_bind`. The socket has an address but
    /// is not yet listening for connections.
    Bound,

    /// The socket is "connected" to a peer address.
    #[expect(dead_code, reason = "p2 has its own way of managing sending/receiving")]
    Connected(SocketAddr),
}

/// A host UDP socket, plus associated bookkeeping.
///
/// The inner state is wrapped in an Arc because the same underlying socket is
/// used for implementing the stream types.
#[derive(Clone)]
pub struct NetworkUdpSocket {
    socket: Arc<tokio::net::UdpSocket>,

    /// The current state in the bind/connect progression.
    udp_state: UdpState,

    /// Socket address family.
    family: SocketAddressFamily,

    /// If set, use this custom check for addrs, otherwise use what's in
    /// `WasiSocketsCtx`.
    socket_addr_check: Option<SocketAddrCheck>,
}

impl NetworkUdpSocket {
    /// Create a new socket in the given family.
    fn new(cx: &WasiSocketsCtx, family: AddressFamily) -> Result<Self, ErrorCode> {
        cx.allowed_network_uses.check_allowed_udp()?;

        // Delegate socket creation to cap_net_ext. They handle a couple of things for us:
        // - On Windows: call WSAStartup if not done before.
        // - Set the NONBLOCK and CLOEXEC flags. Either immediately during socket creation,
        //   or afterwards using ioctl or fcntl. Exact method depends on the platform.

        let fd = udp_socket(family)?;

        let socket_address_family = match family {
            AddressFamily::Ipv4 => SocketAddressFamily::Ipv4,
            AddressFamily::Ipv6 => {
                rustix::net::sockopt::set_ipv6_v6only(&fd, true)?;
                SocketAddressFamily::Ipv6
            }
        };

        let socket = with_ambient_tokio_runtime(|| {
            tokio::net::UdpSocket::try_from(unsafe {
                std::net::UdpSocket::from_raw_socketlike(fd.into_raw_socketlike())
            })
        })?;

        Ok(Self {
            socket: Arc::new(socket),
            udp_state: UdpState::Default,
            family: socket_address_family,
            socket_addr_check: None,
        })
    }

    fn bind(&mut self, addr: SocketAddr) -> Result<(), ErrorCode> {
        udp_bind(&self.socket, addr)?;
        self.udp_state = UdpState::BindStarted;
        Ok(())
    }

    fn finish_bind(&mut self) -> Result<(), ErrorCode> {
        match self.udp_state {
            UdpState::BindStarted => {
                self.udp_state = UdpState::Bound;
                Ok(())
            }
            _ => Err(ErrorCode::NotInProgress),
        }
    }

    pub(crate) fn is_connected(&self) -> bool {
        matches!(self.udp_state, UdpState::Connected(..))
    }

    fn is_bound(&self) -> bool {
        matches!(self.udp_state, UdpState::Connected(..) | UdpState::Bound)
    }

    fn disconnect(&mut self) -> Result<(), ErrorCode> {
        if !self.is_connected() {
            return Err(ErrorCode::InvalidState);
        }
        udp_disconnect(&self.socket).map_err(ErrorCode::from)?;
        self.udp_state = UdpState::Bound;
        Ok(())
    }

    fn connect(&mut self, addr: SocketAddr) -> Result<(), ErrorCode> {
        if !is_valid_address_family(addr.ip(), self.family) || !is_valid_remote_address(addr) {
            return Err(ErrorCode::InvalidArgument);
        }

        match self.udp_state {
            UdpState::Bound | UdpState::Connected(_) => {}
            _ => return Err(ErrorCode::InvalidState),
        }

        match udp_connect(&self.socket, addr) {
            Ok(()) => {
                self.udp_state = UdpState::Connected(addr);
                Ok(())
            }
            Err(e) => {
                // Revert to a consistent state:
                _ = udp_disconnect(&self.socket);
                self.udp_state = UdpState::Bound;

                Err(match e {
                    Errno::AFNOSUPPORT => ErrorCode::InvalidArgument, // See `udp_bind` implementation.
                    Errno::INPROGRESS => {
                        debug!("UDP connect returned EINPROGRESS, which should never happen");
                        ErrorCode::Unknown
                    }
                    err => err.into(),
                })
            }
        }
    }

    fn local_address(&self) -> Result<SocketAddr, ErrorCode> {
        if matches!(self.udp_state, UdpState::Default | UdpState::BindStarted) {
            return Err(ErrorCode::InvalidState);
        }
        let addr = self
            .socket
            .as_socketlike_view::<std::net::UdpSocket>()
            .local_addr()?;
        Ok(addr)
    }

    pub(crate) fn remote_address(&self) -> Result<SocketAddr, ErrorCode> {
        if !matches!(self.udp_state, UdpState::Connected(..)) {
            return Err(ErrorCode::InvalidState);
        }
        let addr = self
            .socket
            .as_socketlike_view::<std::net::UdpSocket>()
            .peer_addr()?;
        Ok(addr)
    }

    pub(crate) fn address_family(&self) -> SocketAddressFamily {
        self.family
    }

    fn unicast_hop_limit(&self) -> Result<u8, ErrorCode> {
        let n = get_unicast_hop_limit(&self.socket, self.family)?;
        Ok(n)
    }

    fn set_unicast_hop_limit(&self, value: u8) -> Result<(), ErrorCode> {
        set_unicast_hop_limit(&self.socket, self.family, value)?;
        Ok(())
    }

    fn receive_buffer_size(&self) -> Result<u64, ErrorCode> {
        let n = receive_buffer_size(&self.socket)?;
        Ok(n)
    }

    fn set_receive_buffer_size(&self, value: u64) -> Result<(), ErrorCode> {
        set_receive_buffer_size(&self.socket, value)?;
        Ok(())
    }

    fn send_buffer_size(&self) -> Result<u64, ErrorCode> {
        let n = send_buffer_size(&self.socket)?;
        Ok(n)
    }

    fn set_send_buffer_size(&self, value: u64) -> Result<(), ErrorCode> {
        set_send_buffer_size(&self.socket, value)?;
        Ok(())
    }

    pub(crate) fn socket(&self) -> &Arc<tokio::net::UdpSocket> {
        &self.socket
    }

    pub(crate) fn socket_addr_check(&self) -> Option<&SocketAddrCheck> {
        self.socket_addr_check.as_ref()
    }

    fn set_socket_addr_check(&mut self, check: Option<SocketAddrCheck>) {
        self.socket_addr_check = check;
    }
}

impl super::loopback::UdpSocket {
    pub fn new(
        socket: &NetworkUdpSocket,
        state: super::loopback::UdpState,
    ) -> Result<Self, ErrorCode> {
        let hop_limit = get_unicast_hop_limit(&socket.socket, socket.family)?;

        let receive_buffer_size = receive_buffer_size(&socket.socket)?;

        let send_buffer_size = send_buffer_size(&socket.socket)?;
        let send_buffer_size = send_buffer_size
            .try_into()
            .unwrap_or(Self::MAX_SEND_BUFFER_SIZE);

        Ok(Self {
            state,
            hop_limit,
            receive_buffer_size,
            send_buffer_size,
            family: socket.family,
            socket_addr_check: socket.socket_addr_check.clone(),
        })
    }
}

pub enum UdpSocket {
    Network(NetworkUdpSocket),
    Loopback(super::loopback::UdpSocket),
    Unspecified {
        net: NetworkUdpSocket,
        lo: super::loopback::UdpSocket,
    },
}

impl UdpSocket {
    pub(crate) fn new(cx: &WasiSocketsCtx, family: AddressFamily) -> Result<Self, ErrorCode> {
        NetworkUdpSocket::new(cx, family).map(Self::Network)
    }

    pub(crate) fn bind(
        &mut self,
        mut addr: SocketAddr,
        loopback: &mut super::loopback::Network,
    ) -> Result<(), ErrorCode> {
        use core::net::{Ipv4Addr, Ipv6Addr};

        let Self::Network(socket) = self else {
            return Err(ErrorCode::InvalidState);
        };
        if !matches!(socket.udp_state, UdpState::Default) {
            return Err(ErrorCode::InvalidState);
        }
        if !is_valid_address_family(addr.ip(), socket.family) {
            return Err(ErrorCode::InvalidArgument);
        }
        let ip = addr.ip().to_canonical();
        if !ip.is_loopback() {
            socket.bind(addr)?;
            if !ip.is_unspecified() {
                return Ok(());
            }
            addr = socket.socket.local_addr()?;
            match &mut addr {
                SocketAddr::V4(addr) => addr.set_ip(Ipv4Addr::LOCALHOST),
                SocketAddr::V6(addr) => addr.set_ip(Ipv6Addr::LOCALHOST),
            }
        };

        let (addr, rx) = loopback.bind_udp(addr)?;
        let lo = super::loopback::UdpSocket::new(
            socket,
            super::loopback::UdpState::BindStarted {
                local_address: addr,
                rx,
            },
        )?;

        if ip.is_unspecified() {
            *self = Self::Unspecified {
                net: socket.clone(),
                lo,
            }
        } else {
            *self = Self::Loopback(lo);
        }
        Ok(())
    }

    pub(crate) fn finish_bind(&mut self) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.finish_bind(),
            Self::Loopback(socket) => socket.finish_bind(),
            Self::Unspecified { net, lo } => {
                net.finish_bind()?;
                lo.finish_bind()
            }
        }
    }

    pub(crate) fn is_connected(&self) -> bool {
        match self {
            Self::Network(socket) => socket.is_connected(),
            Self::Loopback(socket) => socket.is_connected(),
            Self::Unspecified { net, lo } => net.is_connected() && lo.is_connected(),
        }
    }

    pub(crate) fn is_bound(&self) -> bool {
        match self {
            Self::Network(socket) => socket.is_bound(),
            Self::Loopback(socket) => socket.is_bound(),
            Self::Unspecified { net, lo } => net.is_bound() && lo.is_bound(),
        }
    }

    pub(crate) fn disconnect(
        &mut self,
        loopback: &mut super::loopback::Network,
    ) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.disconnect(),
            Self::Loopback(socket) => socket.disconnect(loopback),
            Self::Unspecified { net, lo } => {
                net.disconnect()?;
                lo.disconnect(loopback)
            }
        }
    }

    pub(crate) fn connect(
        &mut self,
        addr: SocketAddr,
        loopback: &mut super::loopback::Network,
    ) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.connect(addr),
            Self::Loopback(socket) => socket.connect(addr, loopback),
            Self::Unspecified { net, lo } => {
                net.connect(addr)?;
                lo.connect(addr, loopback)
            }
        }
    }

    pub(crate) fn local_address(&self) -> Result<SocketAddr, ErrorCode> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => socket.local_address(),
            Self::Loopback(socket) => socket.local_address(),
        }
    }

    pub(crate) fn remote_address(&self) -> Result<SocketAddr, ErrorCode> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.remote_address()
            }
            Self::Loopback(socket) => socket.remote_address(),
        }
    }

    pub(crate) fn address_family(&self) -> SocketAddressFamily {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.address_family()
            }
            Self::Loopback(socket) => socket.address_family(),
        }
    }

    pub(crate) fn unicast_hop_limit(&self) -> Result<u8, ErrorCode> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.unicast_hop_limit()
            }
            Self::Loopback(socket) => socket.unicast_hop_limit(),
        }
    }

    pub(crate) fn set_unicast_hop_limit(&mut self, value: u8) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.set_unicast_hop_limit(value),
            Self::Loopback(socket) => socket.set_unicast_hop_limit(value),
            Self::Unspecified { net, lo } => {
                net.set_unicast_hop_limit(value)?;
                lo.set_unicast_hop_limit(value)
            }
        }
    }

    pub(crate) fn receive_buffer_size(&self) -> Result<u64, ErrorCode> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.receive_buffer_size()
            }
            Self::Loopback(socket) => socket.receive_buffer_size(),
        }
    }

    pub(crate) fn set_receive_buffer_size(&mut self, value: u64) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.set_receive_buffer_size(value),
            Self::Loopback(socket) => socket.set_receive_buffer_size(value),
            Self::Unspecified { net, lo } => {
                net.set_receive_buffer_size(value)?;
                lo.set_receive_buffer_size(value)
            }
        }
    }

    pub(crate) fn send_buffer_size(&self) -> Result<u64, ErrorCode> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.send_buffer_size()
            }
            Self::Loopback(socket) => socket.send_buffer_size(),
        }
    }

    pub(crate) fn set_send_buffer_size(&mut self, value: u64) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.set_send_buffer_size(value),
            Self::Loopback(socket) => socket.set_send_buffer_size(value),
            Self::Unspecified { net, lo } => {
                net.set_send_buffer_size(value)?;
                lo.set_send_buffer_size(value)
            }
        }
    }

    pub(crate) fn socket_addr_check(&self) -> Option<&SocketAddrCheck> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.socket_addr_check()
            }
            Self::Loopback(socket) => socket.socket_addr_check(),
        }
    }

    pub(crate) fn set_socket_addr_check(&mut self, check: Option<SocketAddrCheck>) {
        match self {
            Self::Network(socket) => socket.set_socket_addr_check(check),
            Self::Loopback(socket) => socket.set_socket_addr_check(check),
            Self::Unspecified { net, lo } => {
                net.set_socket_addr_check(check.clone());
                lo.set_socket_addr_check(check);
            }
        }
    }

    pub(crate) fn drop(self, loopback: &mut super::loopback::Network) -> wasmtime::Result<()> {
        match self {
            Self::Network(socket) => {
                drop(socket);
                Ok(())
            }
            Self::Loopback(socket) => socket.drop(loopback),
            Self::Unspecified { net, lo } => {
                drop(net);
                lo.drop(loopback)
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::sockets::WasiSocketsCtx;
    use cap_net_ext::AddressFamily;

    fn make_ipv4_socket() -> NetworkUdpSocket {
        let ctx = WasiSocketsCtx::default();
        NetworkUdpSocket::new(&ctx, AddressFamily::Ipv4).unwrap()
    }

    fn bind_socket(socket: &mut NetworkUdpSocket) {
        let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        socket.bind(addr).unwrap();
        socket.finish_bind().unwrap();
    }

    #[tokio::test]
    async fn test_new_socket_default_state() {
        let socket = make_ipv4_socket();
        assert!(!socket.is_bound());
        assert!(!socket.is_connected());
    }

    #[tokio::test]
    async fn test_bind_and_finish_bind() {
        let mut socket = make_ipv4_socket();
        let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();

        socket.bind(addr).unwrap();
        // BindStarted is not yet Bound
        assert!(!socket.is_bound());

        socket.finish_bind().unwrap();
        assert!(socket.is_bound());
        assert!(!socket.is_connected());
    }

    #[tokio::test]
    async fn test_finish_bind_without_bind_errors() {
        let mut socket = make_ipv4_socket();
        let result = socket.finish_bind();
        assert!(matches!(result, Err(ErrorCode::NotInProgress)));
    }

    #[tokio::test]
    async fn test_connect_from_bound() {
        let mut socket = make_ipv4_socket();
        bind_socket(&mut socket);

        let remote: std::net::SocketAddr = "127.0.0.1:9999".parse().unwrap();
        let result = socket.connect(remote);
        assert!(result.is_ok());
        assert!(socket.is_connected());
    }

    #[tokio::test]
    async fn test_connect_from_default_errors() {
        let mut socket = make_ipv4_socket();
        let remote: std::net::SocketAddr = "127.0.0.1:9999".parse().unwrap();
        let result = socket.connect(remote);
        assert!(matches!(result, Err(ErrorCode::InvalidState)));
    }

    #[tokio::test]
    async fn test_connect_rejects_unspecified_addr() {
        let mut socket = make_ipv4_socket();
        bind_socket(&mut socket);

        let remote: std::net::SocketAddr = "0.0.0.0:9999".parse().unwrap();
        let result = socket.connect(remote);
        assert!(matches!(result, Err(ErrorCode::InvalidArgument)));
    }

    #[tokio::test]
    async fn test_connect_rejects_port_zero() {
        let mut socket = make_ipv4_socket();
        bind_socket(&mut socket);

        let remote: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        let result = socket.connect(remote);
        assert!(matches!(result, Err(ErrorCode::InvalidArgument)));
    }

    #[tokio::test]
    async fn test_connect_rejects_wrong_family() {
        let mut socket = make_ipv4_socket();
        bind_socket(&mut socket);

        let remote: std::net::SocketAddr = "[::1]:9999".parse().unwrap();
        let result = socket.connect(remote);
        assert!(matches!(result, Err(ErrorCode::InvalidArgument)));
    }

    #[tokio::test]
    async fn test_reconnect_from_connected() {
        // Key wasmtime 43 change: connect-first, disconnect-on-failure
        let mut socket = make_ipv4_socket();
        bind_socket(&mut socket);

        let remote1: std::net::SocketAddr = "127.0.0.1:9999".parse().unwrap();
        socket.connect(remote1).unwrap();

        let remote2: std::net::SocketAddr = "127.0.0.1:8888".parse().unwrap();
        let result = socket.connect(remote2);
        assert!(result.is_ok());
        assert!(socket.is_connected());
    }

    #[tokio::test]
    async fn test_disconnect_from_connected() {
        let mut socket = make_ipv4_socket();
        bind_socket(&mut socket);

        let remote: std::net::SocketAddr = "127.0.0.1:9999".parse().unwrap();
        socket.connect(remote).unwrap();

        let result = socket.disconnect();
        assert!(result.is_ok());
        assert!(!socket.is_connected());
        assert!(socket.is_bound());
    }

    #[tokio::test]
    async fn test_disconnect_from_bound_errors() {
        let mut socket = make_ipv4_socket();
        bind_socket(&mut socket);

        let result = socket.disconnect();
        assert!(matches!(result, Err(ErrorCode::InvalidState)));
    }

    #[tokio::test]
    async fn test_local_address_after_bind() {
        let mut socket = make_ipv4_socket();
        bind_socket(&mut socket);

        let addr = socket.local_address();
        assert!(addr.is_ok());
        let addr = addr.unwrap();
        assert_ne!(addr.port(), 0);
    }

    #[tokio::test]
    async fn test_local_address_before_bind_errors() {
        let socket = make_ipv4_socket();
        let result = socket.local_address();
        assert!(matches!(result, Err(ErrorCode::InvalidState)));
    }

    #[tokio::test]
    async fn test_remote_address_when_connected() {
        let mut socket = make_ipv4_socket();
        bind_socket(&mut socket);

        let remote: std::net::SocketAddr = "127.0.0.1:9999".parse().unwrap();
        socket.connect(remote).unwrap();

        let addr = socket.remote_address().unwrap();
        assert_eq!(addr, remote);
    }

    #[tokio::test]
    async fn test_remote_address_when_not_connected_errors() {
        let mut socket = make_ipv4_socket();
        bind_socket(&mut socket);

        let result = socket.remote_address();
        assert!(matches!(result, Err(ErrorCode::InvalidState)));
    }

    #[tokio::test]
    async fn test_hop_limit_roundtrip() {
        let socket = make_ipv4_socket();
        socket.set_unicast_hop_limit(64).unwrap();
        let hop = socket.unicast_hop_limit().unwrap();
        assert_eq!(hop, 64);
    }

    #[tokio::test]
    async fn test_hop_limit_zero_errors() {
        let socket = make_ipv4_socket();
        let result = socket.set_unicast_hop_limit(0);
        assert!(matches!(result, Err(ErrorCode::InvalidArgument)));
    }
}
