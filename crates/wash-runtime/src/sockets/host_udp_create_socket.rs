use super::network::SocketResult;
use super::{SocketAddrUse, SocketAddressFamily, UdpSocket, WasiSocketsCtxView};
use wasmtime::component::Resource;
use wasmtime_wasi::p2::bindings::sockets::{network::IpAddressFamily, udp_create_socket};

type UpstreamUdpSocket = wasmtime_wasi::p2::UdpSocket;

impl udp_create_socket::Host for WasiSocketsCtxView<'_> {
    async fn create_udp_socket(
        &mut self,
        address_family: IpAddressFamily,
    ) -> SocketResult<Resource<UpstreamUdpSocket>> {
        let address_family = match address_family {
            IpAddressFamily::Ipv4 => SocketAddressFamily::Ipv4,
            IpAddressFamily::Ipv6 => SocketAddressFamily::Ipv6,
        };
        let permit = self
            .ctx
            .socket_addr_check
            .check(
                super::util::implicit_bind_addr(address_family),
                SocketAddrUse::UdpCreate,
            )
            .map_err(super::network::socket_error_from_util)?
            .permit;
        let mut socket = UdpSocket::new(self.ctx, address_family)
            .await
            .map_err(super::network::socket_error_from_util)?;
        socket.hold_quota_slot(permit);
        let socket = self.table.push(socket)?;
        Ok(Resource::new_own(socket.rep()))
    }
}
