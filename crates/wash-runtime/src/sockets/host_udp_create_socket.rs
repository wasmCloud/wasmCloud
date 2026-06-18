use super::UdpSocket;
use super::WasiSocketsCtxView;
use super::network::SocketResult;
use wasmtime::component::Resource;
use wasmtime_wasi::p2::bindings::sockets::{network::IpAddressFamily, udp_create_socket};

type UpstreamUdpSocket = wasmtime_wasi::sockets::UdpSocket;

impl udp_create_socket::Host for WasiSocketsCtxView<'_> {
    fn create_udp_socket(
        &mut self,
        address_family: IpAddressFamily,
    ) -> SocketResult<Resource<UpstreamUdpSocket>> {
        let address_family = match address_family {
            IpAddressFamily::Ipv4 => cap_net_ext::AddressFamily::Ipv4,
            IpAddressFamily::Ipv6 => cap_net_ext::AddressFamily::Ipv6,
        };
        let socket = UdpSocket::new(self.ctx, address_family)
            .map_err(super::network::socket_error_from_util)?;
        let socket = self.table.push(socket)?;
        Ok(Resource::new_own(socket.rep()))
    }
}
