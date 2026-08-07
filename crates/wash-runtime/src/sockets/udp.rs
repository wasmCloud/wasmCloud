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
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
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

    /// The guest's quota slot, held for as long as this socket exists.
    ///
    /// A datagram socket is one descriptor however many peers it addresses, so
    /// the slot is taken once — when the socket binds for egress or connects —
    /// rather than per datagram. `Arc` because an unspecified-bound socket is
    /// one socket with two halves: they share the slot and release it when
    /// both are gone.
    quota_slot: Option<Arc<crate::host::quota::ConnectionSlot>>,

    /// Which plane this socket's connected peer was resolved onto.
    ///
    /// Recorded at connect because it cannot be recovered afterwards: the
    /// address stored on the socket is the *rewritten* one, so a peer reached
    /// through the host-loopback sentinel is indistinguishable from the
    /// guest's own virtual loopback by address alone.
    connected_plane: Option<super::Plane>,

    /// Peers this socket has sent to, when it is bound to the unspecified
    /// address and therefore reachable on every interface.
    ///
    /// Such a socket must still receive the *replies* to what it sent — that is
    /// what makes it a UDP client — while not becoming an unsolicited inbound
    /// server on a real interface. Recording the destinations lets the receive
    /// path tell those apart, the way a NAT keeps a mapping per peer.
    ///
    /// `None` when no filtering applies: a loopback bind reaches nothing real,
    /// and a concrete address is an operator-declared listener that is
    /// *supposed* to accept unsolicited traffic.
    egress_peers: Option<Arc<Mutex<BTreeSet<SocketAddr>>>>,
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
            quota_slot: None,
            connected_plane: None,
            egress_peers: None,
        })
    }

    fn bind(&mut self, addr: SocketAddr) -> Result<(), ErrorCode> {
        udp_bind(&self.socket, addr)?;
        self.udp_state = UdpState::BindStarted;
        Ok(())
    }

    /// The peer set this socket filters inbound datagrams against, if any.
    pub(crate) fn egress_peers(&self) -> Option<Arc<Mutex<BTreeSet<SocketAddr>>>> {
        self.egress_peers.clone()
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

    /// Whether this socket is still unbound (in its default post-create state),
    /// the precondition for an implicit bind.
    fn is_unbound(&self) -> bool {
        matches!(self.udp_state, UdpState::Default)
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
            // An unspecified bind stays unspecified at the OS. Pinning it to
            // loopback would confine the socket to loopback for *sending* too —
            // the kernel refuses `send_to` an off-box address from a
            // loopback-bound socket with `EADDRNOTAVAIL` — which takes away
            // outbound UDP entirely. What must not happen is the guest
            // *receiving* unsolicited datagrams from off-host, and that is
            // enforced on the receive path instead: see `egress_peers`.
            socket.bind(addr)?;
            if !ip.is_unspecified() {
                return Ok(());
            }
            // Only this half is confined: the virtual endpoint the guest also
            // gets is registered on loopback, where its guest-to-guest traffic
            // belongs.
            socket.egress_peers = Some(Arc::new(Mutex::new(BTreeSet::new())));
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

    /// Whether a `send-to` on this socket would perform an implicit bind to a
    /// real network address. Only a freshly created, still-unbound socket
    /// does; loopback and unspecified sockets have already been bound (and
    /// checked) by an explicit `bind`.
    pub(crate) fn needs_implicit_bind(&self) -> bool {
        matches!(self, Self::Network(net) if net.is_unbound())
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

    /// Connect to `addr` on `plane`.
    ///
    /// `plane` comes from the socket policy rather than from the address; see
    /// [`TcpSocket::start_connect`](super::tcp::TcpSocket::start_connect).
    pub(crate) fn connect(
        &mut self,
        addr: SocketAddr,
        plane: super::Plane,
        loopback: &mut super::loopback::Network,
    ) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => {
                socket.connected_plane = Some(plane);
                socket.connect(addr)
            }
            Self::Loopback(socket) => socket.connect(addr, loopback),
            // An unspecified-bound socket is simultaneously a real socket and a
            // virtual endpoint, so both sides record the peer, and the plane
            // recorded here is what later decides which half carries a datagram
            // sent with no explicit destination.
            Self::Unspecified { net, lo } => {
                net.connected_plane = Some(plane);
                net.connect(addr)?;
                lo.connect(addr, loopback)
            }
        }
    }

    /// Which plane a datagram with no explicit destination travels on.
    ///
    /// `None` when the socket is not connected, in which case the destination
    /// is checked per datagram and that decision is used instead.
    pub(crate) fn connected_plane(&self) -> Option<super::Plane> {
        match self {
            Self::Network(net) | Self::Unspecified { net, .. } => net.connected_plane,
            // A purely virtual endpoint has no other half to choose.
            Self::Loopback(_) => Some(super::Plane::Virtual),
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

    /// Hold a quota slot for this socket's lifetime.
    ///
    /// Only a socket that reaches a real interface takes one; a purely virtual
    /// endpoint costs no descriptor.
    pub(crate) fn hold_quota_slot(&mut self, slot: Option<crate::host::quota::ConnectionSlot>) {
        let Some(slot) = slot else { return };
        let slot = Arc::new(slot);
        match self {
            Self::Network(socket) => socket.quota_slot = Some(slot),
            Self::Unspecified { net, lo: _ } => net.quota_slot = Some(slot),
            // Virtual only: nothing real to bound.
            Self::Loopback(_) => {}
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

    /// Binding the unspecified address must not put a real datagram socket on
    /// every interface. The virtual registration was always rewritten to
    /// loopback; the OS socket was not, so a guest binding `0.0.0.0` was
    /// reachable from off-host. The TCP path has always rewritten both.
    #[tokio::test]
    async fn unspecified_bind_never_serves_unsolicited_traffic() {
        for (family, unspecified) in [
            (AddressFamily::Ipv4, "0.0.0.0:0"),
            (AddressFamily::Ipv6, "[::]:0"),
        ] {
            let ctx = WasiSocketsCtx::default();
            let mut socket = UdpSocket::new(&ctx, family).unwrap();
            let mut loopback = crate::sockets::loopback::Network::default();

            socket
                .bind(unspecified.parse().unwrap(), &mut loopback)
                .unwrap();
            socket.finish_bind().unwrap();

            let UdpSocket::Unspecified { net, .. } = &socket else {
                panic!("binding {unspecified} should produce an Unspecified socket");
            };

            // The socket stays bound where the guest asked, because a
            // loopback-bound socket cannot *send* off-box: the kernel refuses
            // with `EADDRNOTAVAIL`, which would take away outbound UDP.
            let os_addr = net.socket.local_addr().unwrap();
            assert!(
                os_addr.ip().is_unspecified(),
                "an unspecified bind must stay unspecified to route; got {os_addr}"
            );

            // What keeps it from being an unsolicited server is the peer
            // filter, which starts empty: nothing is admitted until the guest
            // sends somewhere.
            let peers = net
                .egress_peers()
                .expect("an unspecified bind filters its inbound datagrams");
            assert!(
                peers.lock().unwrap().is_empty(),
                "a socket that has sent nothing admits nothing"
            );
        }
    }

    /// The property the peer filter exists for, end to end against real
    /// sockets: a guest that sends somewhere gets that peer's reply, and a
    /// stranger who was never addressed does not reach it.
    #[tokio::test]
    async fn a_reply_is_admitted_and_a_stranger_is_not() {
        let ctx = WasiSocketsCtx::default();
        let mut socket = UdpSocket::new(&ctx, AddressFamily::Ipv4).unwrap();
        let mut loopback = crate::sockets::loopback::Network::default();
        socket
            .bind("0.0.0.0:0".parse().unwrap(), &mut loopback)
            .unwrap();
        socket.finish_bind().unwrap();
        let UdpSocket::Unspecified { net, .. } = &socket else {
            panic!("expected an Unspecified socket");
        };
        let guest = net.socket.clone();
        let guest_port = guest.local_addr().unwrap().port();
        let peers = net.egress_peers().unwrap();

        // The peer the guest talks to, and one it never addresses.
        let server = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_addr = server.local_addr().unwrap();
        let stranger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();

        // Sending records the peer, which is what admits its reply.
        peers.lock().unwrap().insert(server_addr);
        guest.send_to(b"question", server_addr).await.unwrap();

        let mut buf = [0u8; 64];
        let (n, from) = server.recv_from(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"question");
        server.send_to(b"answer", from).await.unwrap();
        stranger
            .send_to(b"unsolicited", format!("127.0.0.1:{guest_port}"))
            .await
            .unwrap();

        // Read whatever arrives and apply the filter the receive paths apply.
        let mut admitted = Vec::new();
        for _ in 0..2 {
            let mut buf = [0u8; 64];
            let Ok(Ok((n, from))) =
                tokio::time::timeout(std::time::Duration::from_secs(2), guest.recv_from(&mut buf))
                    .await
            else {
                break;
            };
            if peers.lock().unwrap().contains(&from) {
                admitted.push(buf[..n].to_vec());
            }
        }
        assert_eq!(
            admitted,
            vec![b"answer".to_vec()],
            "only the peer the guest addressed should be delivered"
        );
    }

    /// A loopback bind reaches nothing real, and a concrete bind is an
    /// operator-declared listener that is supposed to hear from strangers —
    /// neither filters.
    #[tokio::test]
    async fn only_an_unspecified_bind_filters_its_peers() {
        let ctx = WasiSocketsCtx::default();
        let mut socket = UdpSocket::new(&ctx, AddressFamily::Ipv4).unwrap();
        let mut loopback = crate::sockets::loopback::Network::default();
        socket
            .bind("127.0.0.1:0".parse().unwrap(), &mut loopback)
            .unwrap();
        socket.finish_bind().unwrap();
        assert!(
            matches!(socket, UdpSocket::Loopback(_)),
            "a loopback bind is virtual only"
        );
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
