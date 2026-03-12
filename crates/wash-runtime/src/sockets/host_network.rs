use super::WasiSocketsCtxView;
use super::network::{self as our_network, SocketError};
use super::util::{from_ipv4_addr, from_ipv6_addr, to_ipv4_addr, to_ipv6_addr};
use wasmtime::component::Resource;
use wasmtime_wasi::p2::bindings::sockets::network::{
    self, ErrorCode, IpAddress, IpSocketAddress, Ipv4SocketAddress, Ipv6SocketAddress,
};

impl network::Host for WasiSocketsCtxView<'_> {
    fn convert_error_code(&mut self, error: SocketError) -> wasmtime::Result<ErrorCode> {
        error.downcast()
    }

    fn network_error_code(
        &mut self,
        err: Resource<wasmtime::Error>,
    ) -> wasmtime::Result<Option<ErrorCode>> {
        let err = self.table.get(&err)?;

        if let Some(err) = err.downcast_ref::<std::io::Error>() {
            return Ok(Some(our_network::error_code_from_io(err.kind())));
        }

        Ok(None)
    }
}

impl network::HostNetwork for WasiSocketsCtxView<'_> {
    fn drop(&mut self, this: Resource<network::Network>) -> wasmtime::Result<()> {
        let this = Resource::<our_network::Network>::new_own(this.rep());
        self.table.delete(this)?;
        Ok(())
    }
}

// Conversion functions — can't use From impls due to orphan rule.

pub(crate) fn ip_addr_to_ip_address(addr: std::net::IpAddr) -> IpAddress {
    match addr {
        std::net::IpAddr::V4(v4) => IpAddress::Ipv4(from_ipv4_addr(v4)),
        std::net::IpAddr::V6(v6) => IpAddress::Ipv6(from_ipv6_addr(v6)),
    }
}

pub(crate) fn ip_socket_address_to_socket_addr(addr: IpSocketAddress) -> std::net::SocketAddr {
    match addr {
        IpSocketAddress::Ipv4(ipv4) => {
            std::net::SocketAddr::V4(ipv4_socket_address_to_socket_addr_v4(ipv4))
        }
        IpSocketAddress::Ipv6(ipv6) => {
            std::net::SocketAddr::V6(ipv6_socket_address_to_socket_addr_v6(ipv6))
        }
    }
}

pub(crate) fn socket_addr_to_ip_socket_address(addr: std::net::SocketAddr) -> IpSocketAddress {
    match addr {
        std::net::SocketAddr::V4(v4) => {
            IpSocketAddress::Ipv4(socket_addr_v4_to_ipv4_socket_address(v4))
        }
        std::net::SocketAddr::V6(v6) => {
            IpSocketAddress::Ipv6(socket_addr_v6_to_ipv6_socket_address(v6))
        }
    }
}

pub(crate) fn ipv4_socket_address_to_socket_addr_v4(
    addr: Ipv4SocketAddress,
) -> std::net::SocketAddrV4 {
    std::net::SocketAddrV4::new(to_ipv4_addr(addr.address), addr.port)
}

pub(crate) fn socket_addr_v4_to_ipv4_socket_address(
    addr: std::net::SocketAddrV4,
) -> Ipv4SocketAddress {
    Ipv4SocketAddress {
        address: from_ipv4_addr(*addr.ip()),
        port: addr.port(),
    }
}

pub(crate) fn ipv6_socket_address_to_socket_addr_v6(
    addr: Ipv6SocketAddress,
) -> std::net::SocketAddrV6 {
    std::net::SocketAddrV6::new(
        to_ipv6_addr(addr.address),
        addr.port,
        addr.flow_info,
        addr.scope_id,
    )
}

pub(crate) fn socket_addr_v6_to_ipv6_socket_address(
    addr: std::net::SocketAddrV6,
) -> Ipv6SocketAddress {
    Ipv6SocketAddress {
        address: from_ipv6_addr(*addr.ip()),
        port: addr.port(),
        flow_info: addr.flowinfo(),
        scope_id: addr.scope_id(),
    }
}
