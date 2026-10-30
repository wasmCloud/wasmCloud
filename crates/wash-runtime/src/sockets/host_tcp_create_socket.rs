use super::network::SocketResult;
use super::{SocketAddrUse, SocketAddressFamily, TcpSocket, WasiSocketsCtxView};
use wasmtime::component::Resource;
use wasmtime_wasi::p2::bindings::sockets::{network::IpAddressFamily, tcp_create_socket};

type UpstreamTcpSocket = wasmtime_wasi::p2::TcpSocket;

impl tcp_create_socket::Host for WasiSocketsCtxView<'_> {
    fn create_tcp_socket(
        &mut self,
        address_family: IpAddressFamily,
    ) -> SocketResult<Resource<UpstreamTcpSocket>> {
        let family = address_family.into();
        let permit = self
            .ctx
            .socket_addr_check
            .check(
                super::util::implicit_bind_addr(family),
                SocketAddrUse::TcpCreate,
            )
            .map_err(super::network::socket_error_from_util)?
            .permit;
        let mut socket =
            TcpSocket::new(self.ctx, family).map_err(super::network::socket_error_from_util)?;
        socket.hold_quota_slot(permit);
        let socket = self.table.push(socket)?;
        Ok(Resource::new_own(socket.rep()))
    }
}

impl From<IpAddressFamily> for SocketAddressFamily {
    fn from(family: IpAddressFamily) -> SocketAddressFamily {
        match family {
            IpAddressFamily::Ipv4 => Self::Ipv4,
            IpAddressFamily::Ipv6 => Self::Ipv6,
        }
    }
}
