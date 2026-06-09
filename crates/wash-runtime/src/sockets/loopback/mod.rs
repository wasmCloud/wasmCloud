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

    pub fn bind_udp(
        &mut self,
        mut addr: SocketAddr,
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
        };
        let port = bind(net, addr.port(), ep)?;
        addr.set_port(port.into());
        Ok((addr, rx))
    }

    /// Returns true if a loopback TCP listener is registered for `addr`.
    /// Unlike `connect_tcp`, this does not require `&mut self`.
    pub fn has_tcp_listener(&self, addr: &SocketAddr) -> bool {
        let net = self.get_tcp_net(addr.ip());
        let Some(port) = NonZeroU16::new(addr.port()) else {
            return false;
        };
        matches!(net.get(&port), Some(TcpEndpoint::Listening(_)))
    }

    /// Returns true if a loopback UDP socket is bound for `addr` — an in-process
    /// peer able to receive datagrams. Unlike `connect_udp`, needs only `&self`.
    pub fn has_udp_listener(&self, addr: &SocketAddr) -> bool {
        let net = self.get_udp_net(addr.ip());
        let Some(port) = NonZeroU16::new(addr.port()) else {
            return false;
        };
        net.contains_key(&port)
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
    ) -> Result<Option<&mpsc::UnboundedSender<(UdpDatagram, OwnedSemaphorePermit)>>, ErrorCode>
    {
        let net = self.get_udp_net(remote_address.ip());
        let Some(port) = NonZeroU16::new(remote_address.port()) else {
            return Err(ErrorCode::InvalidArgument);
        };
        let Some(UdpEndpoint {
            tx,
            connected_address,
        }) = net.get(&port)
        else {
            return Ok(None);
        };
        if let Some(addr) = connected_address
            && local_address != addr
        {
            return Ok(None);
        }
        Ok(Some(tx))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn has_tcp_listener_only_true_for_listening_endpoints() {
        let (tx, _rx) = mpsc::channel::<TcpConn>(1);
        let mut net = Network::default();
        let port = NonZeroU16::new(8080).unwrap();

        // A merely-bound endpoint is not a listener.
        net.tcp_ipv4.insert(port, TcpEndpoint::Bound);
        assert!(!net.has_tcp_listener(&SocketAddr::from(([127, 0, 0, 1], 8080))));

        // Promote it to Listening → now it is.
        net.tcp_ipv4.insert(port, TcpEndpoint::Listening(tx));
        assert!(net.has_tcp_listener(&SocketAddr::from(([127, 0, 0, 1], 8080))));

        // Unregistered port: false.
        assert!(!net.has_tcp_listener(&SocketAddr::from(([127, 0, 0, 1], 9090))));

        // Port 0 is never a valid listener key.
        assert!(!net.has_tcp_listener(&SocketAddr::from(([127, 0, 0, 1], 0))));

        // The IPv6 table is independent of the IPv4 one.
        assert!(!net.has_tcp_listener(&"[::1]:8080".parse().unwrap()));
    }

    #[test]
    fn has_udp_listener_true_only_for_bound_ports() {
        let mut net = Network::default();

        // Unbound port: false.
        assert!(!net.has_udp_listener(&SocketAddr::from(([127, 0, 0, 1], 5353))));

        // Bind it → now an in-process peer exists.
        let (bound, _rx) = net
            .bind_udp(SocketAddr::from(([127, 0, 0, 1], 5353)))
            .unwrap();
        assert!(net.has_udp_listener(&bound));
        assert!(net.has_udp_listener(&SocketAddr::from(([127, 0, 0, 1], 5353))));

        // Other port / port 0 / other family: false.
        assert!(!net.has_udp_listener(&SocketAddr::from(([127, 0, 0, 1], 6000))));
        assert!(!net.has_udp_listener(&SocketAddr::from(([127, 0, 0, 1], 0))));
        assert!(!net.has_udp_listener(&"[::1]:5353".parse().unwrap()));
    }
}
