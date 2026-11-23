use crate::sockets::util::ErrorCode;
use core::net::{IpAddr, SocketAddr};
use core::num::NonZeroU16;
use std::collections::{HashMap, HashSet, hash_map};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{OwnedSemaphorePermit, mpsc};

mod tcp;
mod udp;

pub use tcp::*;
pub use udp::*;

/// The store a virtual port was bound from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Family {
    V4,
    V6,
}

impl Family {
    fn of(ip: IpAddr) -> Self {
        match ip {
            IpAddr::V4(..) => Self::V4,
            IpAddr::V6(..) => Self::V6,
        }
    }
}

/// A virtual port: the network keys endpoints by protocol, family and number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Port {
    Tcp(Family, NonZeroU16),
    Udp(Family, NonZeroU16),
}

#[derive(Default)]
pub struct Network {
    pub tcp_ipv4: HashMap<NonZeroU16, TcpEndpoint>,
    pub tcp_ipv6: HashMap<NonZeroU16, TcpEndpoint>,
    pub udp_ipv4: HashMap<NonZeroU16, UdpEndpoint>,
    pub udp_ipv6: HashMap<NonZeroU16, UdpEndpoint>,
    /// The store that last bound each port.
    owners: HashMap<Port, Owner>,
    /// The ports each store holds, so releasing a store visits only its own.
    held: HashMap<Owner, HashSet<Port>>,
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

    fn get_tcp_net_mut(&mut self, ip: IpAddr) -> &mut HashMap<NonZeroU16, TcpEndpoint> {
        self.tcp_net_mut(Family::of(ip))
    }

    fn tcp_net_mut(&mut self, family: Family) -> &mut HashMap<NonZeroU16, TcpEndpoint> {
        match family {
            Family::V4 => &mut self.tcp_ipv4,
            Family::V6 => &mut self.tcp_ipv6,
        }
    }

    fn get_udp_net(&self, ip: IpAddr) -> &HashMap<NonZeroU16, UdpEndpoint> {
        match ip {
            IpAddr::V4(..) => &self.udp_ipv4,
            IpAddr::V6(..) => &self.udp_ipv6,
        }
    }

    fn get_udp_net_mut(&mut self, ip: IpAddr) -> &mut HashMap<NonZeroU16, UdpEndpoint> {
        self.udp_net_mut(Family::of(ip))
    }

    fn udp_net_mut(&mut self, family: Family) -> &mut HashMap<NonZeroU16, UdpEndpoint> {
        match family {
            Family::V4 => &mut self.udp_ipv4,
            Family::V6 => &mut self.udp_ipv6,
        }
    }

    fn record(&mut self, port: Port, owner: Owner) {
        // The port maps are public, so a removal made outside `forget` may have left a record.
        self.forget(port);
        self.owners.insert(port, owner);
        self.held.entry(owner).or_default().insert(port);
    }

    /// Drop `port`'s owner record. Every endpoint removal calls this.
    fn forget(&mut self, port: Port) {
        let Some(owner) = self.owners.remove(&port) else {
            return;
        };
        if let Some(ports) = self.held.get_mut(&owner) {
            ports.remove(&port);
            if ports.is_empty() {
                self.held.remove(&owner);
            }
        }
    }

    fn take_tcp(&mut self, family: Family, port: NonZeroU16) -> Option<TcpEndpoint> {
        let endpoint = self.tcp_net_mut(family).remove(&port)?;
        self.forget(Port::Tcp(family, port));
        Some(endpoint)
    }

    fn take_udp(&mut self, family: Family, port: NonZeroU16) -> Option<UdpEndpoint> {
        let endpoint = self.udp_net_mut(family).remove(&port)?;
        self.forget(Port::Udp(family, port));
        Some(endpoint)
    }

    pub fn bind_tcp(
        &mut self,
        mut addr: SocketAddr,
        owner: Owner,
    ) -> Result<SocketAddr, ErrorCode> {
        let family = Family::of(addr.ip());
        let port = bind(self.tcp_net_mut(family), addr.port(), TcpEndpoint::Bound)?;
        self.record(Port::Tcp(family, port), owner);
        addr.set_port(port.into());
        Ok(addr)
    }

    /// Free the ports `owner` still holds. Its store is gone, so its sockets
    /// were dropped without releasing them.
    pub(crate) fn release(&mut self, owner: Owner) {
        let Some(held) = self.held.remove(&owner) else {
            return;
        };
        for port in held {
            match port {
                Port::Tcp(family, port) => drop(self.take_tcp(family, port)),
                Port::Udp(family, port) => drop(self.take_udp(family, port)),
            }
        }
    }

    /// Release the TCP endpoint bound at `addr`, returning it if one was bound.
    pub fn unbind_tcp(&mut self, addr: SocketAddr) -> Option<TcpEndpoint> {
        self.take_tcp(Family::of(addr.ip()), NonZeroU16::new(addr.port())?)
    }

    /// Release the UDP endpoint bound at `addr`, returning it if one was bound.
    pub fn unbind_udp(&mut self, addr: SocketAddr) -> Option<UdpEndpoint> {
        self.take_udp(Family::of(addr.ip()), NonZeroU16::new(addr.port())?)
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
        let family = Family::of(addr.ip());
        let (tx, rx) = mpsc::unbounded_channel();
        let ep = UdpEndpoint {
            tx,
            connected_address: None,
            receive_permits,
        };
        let port = bind(self.udp_net_mut(family), addr.port(), ep)?;
        self.record(Port::Udp(family, port), owner);
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

    /// How many ports have an owner on record, checked against the index.
    #[cfg(test)]
    pub(crate) fn owner_records(&self) -> usize {
        let indexed: usize = self.held.values().map(HashSet::len).sum();
        assert_eq!(indexed, self.owners.len(), "the index matches the records");
        self.owners.len()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    /// Release frees an owner's ports in each family and protocol, and spares
    /// the same port number held by another owner.
    #[test]
    fn release_frees_each_family_and_protocol_and_spares_other_owners() {
        let mut net = Network::default();
        let (a, b) = (Owner::new(), Owner::new());
        let v4: SocketAddr = "127.0.0.1:7000".parse().unwrap();
        let v6: SocketAddr = "[::1]:7001".parse().unwrap();
        net.bind_tcp(v4, a).unwrap();
        net.bind_tcp(v6, a).unwrap();
        let _a_udp = net.bind_udp(v6, Arc::new(Semaphore::new(0)), a).unwrap();
        let _b_udp = net.bind_udp(v4, Arc::new(Semaphore::new(0)), b).unwrap();
        net.release(a);
        assert!(net.tcp_ipv4.is_empty() && net.tcp_ipv6.is_empty() && net.udp_ipv6.is_empty());
        assert_eq!(net.udp_ipv4.len(), 1, "the other owner's UDP port survives");
    }

    /// Unbinding a port drops its owner record while the owner lives on.
    #[test]
    fn unbinding_a_port_drops_its_owner_record() {
        let mut net = Network::default();
        let owner = Owner::new();
        let v4: SocketAddr = "127.0.0.1:7000".parse().unwrap();
        let v6: SocketAddr = "[::1]:7000".parse().unwrap();
        net.bind_tcp(v4, owner).unwrap();
        net.bind_tcp(v6, owner).unwrap();
        net.unbind_tcp(v4).unwrap();
        net.unbind_tcp(v6).unwrap();
        assert_eq!(net.owner_records(), 0);
    }

    /// The index lists the ports each owner holds and drops an owner once its
    /// last port closes.
    #[test]
    fn the_index_tracks_each_owners_ports() {
        let mut net = Network::default();
        let (a, b) = (Owner::new(), Owner::new());
        let any_v4: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let tcp = net.bind_tcp(any_v4, a).unwrap();
        let (udp, _rx) = net
            .bind_udp("[::1]:0".parse().unwrap(), Arc::new(Semaphore::new(0)), a)
            .unwrap();
        let other = net.bind_tcp(any_v4, b).unwrap();
        let port = |addr: SocketAddr| NonZeroU16::new(addr.port()).unwrap();
        assert_eq!(
            net.held[&a],
            HashSet::from([
                Port::Tcp(Family::V4, port(tcp)),
                Port::Udp(Family::V6, port(udp))
            ])
        );
        assert_eq!(
            net.held[&b],
            HashSet::from([Port::Tcp(Family::V4, port(other))])
        );
        net.unbind_tcp(tcp).unwrap();
        net.unbind_udp(udp).unwrap();
        assert!(
            !net.held.contains_key(&a),
            "an owner with no ports has no entry"
        );
        assert_eq!(net.owner_records(), 1);
    }
}
