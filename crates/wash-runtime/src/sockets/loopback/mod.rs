use crate::sockets::util::ErrorCode;
use core::net::{IpAddr, SocketAddr};
use core::num::NonZeroU16;
use std::collections::{HashMap, hash_map};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{OwnedSemaphorePermit, mpsc};

mod tcp;
mod udp;

pub use tcp::*;
pub use udp::*;

/// The store a virtual port was bound from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owner(u64);

impl Owner {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for Owner {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Port {
    TcpV4(NonZeroU16),
    TcpV6(NonZeroU16),
    UdpV4(NonZeroU16),
    UdpV6(NonZeroU16),
}

#[derive(Default)]
pub struct Network {
    pub tcp_ipv4: HashMap<NonZeroU16, TcpEndpoint>,
    pub tcp_ipv6: HashMap<NonZeroU16, TcpEndpoint>,
    pub udp_ipv4: HashMap<NonZeroU16, UdpEndpoint>,
    pub udp_ipv6: HashMap<NonZeroU16, UdpEndpoint>,
    /// The store that last bound each port.
    owners: HashMap<Port, Owner>,
}

fn bind<T>(net: &mut HashMap<NonZeroU16, T>, port: u16, ep: T) -> Result<NonZeroU16, ErrorCode> {
    if let Some(port) = NonZeroU16::new(port) {
        let hash_map::Entry::Vacant(entry) = net.entry(port) else {
            return Err(ErrorCode::AddressInUse);
        };
        entry.insert(ep);
        Ok(port)
    } else {
        for port in (1..=u16::MAX).rev() {
            let Some(port) = NonZeroU16::new(port) else {
                continue;
            };
            if let hash_map::Entry::Vacant(entry) = net.entry(port) {
                entry.insert(ep);
                return Ok(port);
            };
        }
        Err(ErrorCode::AddressInUse)
    }
}

impl Network {
    fn get_tcp_net(&self, ip: IpAddr) -> &HashMap<NonZeroU16, TcpEndpoint> {
        match ip {
            IpAddr::V4(..) => &self.tcp_ipv4,
            IpAddr::V6(..) => &self.tcp_ipv6,
        }
    }

    pub(crate) fn get_tcp_net_mut(&mut self, ip: IpAddr) -> &mut HashMap<NonZeroU16, TcpEndpoint> {
        match ip {
            IpAddr::V4(..) => &mut self.tcp_ipv4,
            IpAddr::V6(..) => &mut self.tcp_ipv6,
        }
    }

    fn get_udp_net(&self, ip: IpAddr) -> &HashMap<NonZeroU16, UdpEndpoint> {
        match ip {
            IpAddr::V4(..) => &self.udp_ipv4,
            IpAddr::V6(..) => &self.udp_ipv6,
        }
    }

    pub(crate) fn get_udp_net_mut(&mut self, ip: IpAddr) -> &mut HashMap<NonZeroU16, UdpEndpoint> {
        match ip {
            IpAddr::V4(..) => &mut self.udp_ipv4,
            IpAddr::V6(..) => &mut self.udp_ipv6,
        }
    }

    pub fn bind_tcp(
        &mut self,
        mut addr: SocketAddr,
        owner: Owner,
    ) -> Result<SocketAddr, ErrorCode> {
        let net = self.get_tcp_net_mut(addr.ip());
        let port = bind(net, addr.port(), TcpEndpoint::Bound)?;
        let key = match addr.ip() {
            IpAddr::V4(..) => Port::TcpV4(port),
            IpAddr::V6(..) => Port::TcpV6(port),
        };
        self.owners.insert(key, owner);
        addr.set_port(port.into());
        Ok(addr)
    }

    /// Free the ports `owner` still holds. Its store is gone, so its sockets
    /// were dropped without releasing them.
    pub(crate) fn release(&mut self, owner: Owner) {
        let Self {
            tcp_ipv4,
            tcp_ipv6,
            udp_ipv4,
            udp_ipv6,
            owners,
        } = self;
        owners.retain(|port, holder| {
            if *holder != owner {
                return true;
            }
            match *port {
                Port::TcpV4(port) => drop(tcp_ipv4.remove(&port)),
                Port::TcpV6(port) => drop(tcp_ipv6.remove(&port)),
                Port::UdpV4(port) => drop(udp_ipv4.remove(&port)),
                Port::UdpV6(port) => drop(udp_ipv6.remove(&port)),
            }
            false
        });
    }

    /// Release the TCP endpoint bound at `addr`, returning it if one was bound.
    pub fn unbind_tcp(&mut self, addr: SocketAddr) -> Option<TcpEndpoint> {
        let port = NonZeroU16::new(addr.port())?;
        self.get_tcp_net_mut(addr.ip()).remove(&port)
    }

    pub fn bind_udp(
        &mut self,
        mut addr: SocketAddr,
        receive_permits: std::sync::Arc<tokio::sync::Semaphore>,
        owner: Owner,
    ) -> Result<
        (
            SocketAddr,
            mpsc::UnboundedReceiver<(UdpDatagram, OwnedSemaphorePermit)>,
        ),
        ErrorCode,
    > {
        let net = self.get_udp_net_mut(addr.ip());
        let (tx, rx) = mpsc::unbounded_channel();
        let ep = UdpEndpoint {
            tx,
            connected_address: None,
            receive_permits,
        };
        let port = bind(net, addr.port(), ep)?;
        let key = match addr.ip() {
            IpAddr::V4(..) => Port::UdpV4(port),
            IpAddr::V6(..) => Port::UdpV6(port),
        };
        self.owners.insert(key, owner);
        addr.set_port(port.into());
        Ok((addr, rx))
    }

    pub fn connect_tcp(&mut self, addr: &SocketAddr) -> Result<&mpsc::Sender<TcpConn>, ErrorCode> {
        let net = self.get_tcp_net(addr.ip());
        let Some(port) = NonZeroU16::new(addr.port()) else {
            return Err(ErrorCode::InvalidArgument);
        };
        let Some(TcpEndpoint::Listening(tx)) = net.get(&port) else {
            return Err(ErrorCode::ConnectionRefused);
        };
        Ok(tx)
    }

    pub fn connect_udp(
        &mut self,
        local_address: &SocketAddr,
        remote_address: &SocketAddr,
    ) -> Result<Option<&UdpEndpoint>, ErrorCode> {
        let net = self.get_udp_net(remote_address.ip());
        let Some(port) = NonZeroU16::new(remote_address.port()) else {
            return Err(ErrorCode::InvalidArgument);
        };
        let Some(endpoint) = net.get(&port) else {
            return Ok(None);
        };
        if let Some(addr) = endpoint.connected_address
            && !crate::sockets::util::is_same_peer(*local_address, addr)
        {
            return Ok(None);
        }
        Ok(Some(endpoint))
    }
}
