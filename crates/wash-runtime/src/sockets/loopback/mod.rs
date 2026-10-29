use crate::sockets::util::ErrorCode;
use core::net::{IpAddr, SocketAddr};
use core::num::NonZeroU16;
use std::collections::{HashMap, hash_map};
use tokio::sync::{OwnedSemaphorePermit, mpsc};

mod tcp;
mod udp;

pub use tcp::*;
pub use udp::*;

#[derive(Default)]
pub struct Network {
    pub tcp_ipv4: HashMap<NonZeroU16, TcpEndpoint>,
    pub tcp_ipv6: HashMap<NonZeroU16, TcpEndpoint>,
    pub udp_ipv4: HashMap<NonZeroU16, UdpEndpoint>,
    pub udp_ipv6: HashMap<NonZeroU16, UdpEndpoint>,
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

    pub fn bind_tcp(&mut self, mut addr: SocketAddr) -> Result<SocketAddr, ErrorCode> {
        let net = self.get_tcp_net_mut(addr.ip());
        let port = bind(net, addr.port(), TcpEndpoint::Bound)?;
        addr.set_port(port.into());
        Ok(addr)
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
