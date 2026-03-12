use wasmtime::component::Resource;
use wasmtime_wasi::p2::bindings::sockets::{
    network::{ErrorCode, IpAddressFamily, IpSocketAddress},
    tcp::{self, ShutdownType},
};
use wasmtime_wasi_io::poll::Pollable;
use wasmtime_wasi_io::{
    poll::DynPollable,
    streams::{DynInputStream, DynOutputStream},
};

use super::host_network::{ip_socket_address_to_socket_addr, socket_addr_to_ip_socket_address};
use super::network::{SocketResult, socket_error_from_util as se};
use super::{SocketAddrUse, WasiSocketsCtxView};

type UpstreamTcpSocket = wasmtime_wasi::sockets::TcpSocket;
type UpstreamNetwork = wasmtime_wasi::p2::Network;

impl tcp::Host for WasiSocketsCtxView<'_> {}

impl tcp::HostTcpSocket for WasiSocketsCtxView<'_> {
    async fn start_bind(
        &mut self,
        this: Resource<UpstreamTcpSocket>,
        network: Resource<UpstreamNetwork>,
        local_address: IpSocketAddress,
    ) -> SocketResult<()> {
        let network = Resource::<super::network::Network>::new_borrow(network.rep());
        let network = self.table.get(&network)?;
        let mut local_address = ip_socket_address_to_socket_addr(local_address);

        // Rewrite unspecified addresses (0.0.0.0 / [::]) to loopback before the
        // permission check and OS bind — components must not listen on all interfaces.
        if local_address.ip().is_unspecified() {
            let rewritten = match local_address {
                std::net::SocketAddr::V4(ref a) => std::net::SocketAddr::V4(
                    std::net::SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, a.port()),
                ),
                std::net::SocketAddr::V6(ref a) => {
                    std::net::SocketAddr::V6(std::net::SocketAddrV6::new(
                        std::net::Ipv6Addr::LOCALHOST,
                        a.port(),
                        a.flowinfo(),
                        a.scope_id(),
                    ))
                }
            };
            tracing::debug!(
                original = %local_address,
                rewritten = %rewritten,
                "rewriting unspecified bind address to loopback"
            );
            local_address = rewritten;
        }

        network
            .check_socket_addr(local_address, SocketAddrUse::TcpBind)
            .await
            .map_err(super::network::socket_error_from_io)?;

        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let mut loopback = self
            .ctx
            .loopback
            .lock()
            .map_err(|e| super::network::SocketError::trap(wasmtime::format_err!("{e}")))?;
        self.table
            .get_mut(&this)?
            .start_bind(local_address, &mut loopback)
            .map_err(se)?;

        Ok(())
    }

    fn finish_bind(&mut self, this: Resource<UpstreamTcpSocket>) -> SocketResult<()> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get_mut(&this)?;
        socket.finish_bind().map_err(se)?;
        Ok(())
    }

    async fn start_connect(
        &mut self,
        this: Resource<UpstreamTcpSocket>,
        network: Resource<UpstreamNetwork>,
        remote_address: IpSocketAddress,
    ) -> SocketResult<()> {
        let network = Resource::<super::network::Network>::new_borrow(network.rep());
        let network = self.table.get(&network)?;
        let remote_address = ip_socket_address_to_socket_addr(remote_address);

        network
            .check_socket_addr(remote_address, SocketAddrUse::TcpConnect)
            .await
            .map_err(super::network::socket_error_from_io)?;

        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get_mut(&this)?;
        let mut loopback = self
            .ctx
            .loopback
            .lock()
            .map_err(|e| super::network::SocketError::trap(wasmtime::format_err!("{e}")))?;
        let future = socket
            .start_connect(&remote_address, &mut loopback)
            .map_err(se)?
            .connect(remote_address);
        socket.set_pending_connect(future).map_err(se)?;

        Ok(())
    }

    fn finish_connect(
        &mut self,
        this: Resource<UpstreamTcpSocket>,
    ) -> SocketResult<(Resource<DynInputStream>, Resource<DynOutputStream>)> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get_mut(&this)?;

        let result = socket
            .take_pending_connect()
            .map_err(se)?
            .ok_or(ErrorCode::WouldBlock)?;
        let mut loopback = self
            .ctx
            .loopback
            .lock()
            .map_err(|e| super::network::SocketError::trap(wasmtime::format_err!("{e}")))?;
        socket.finish_connect(result, &mut loopback).map_err(se)?;
        let (input, output) = socket.p2_streams()?;
        let input = self.table.push_child(input, &this)?;
        let output = self.table.push_child(output, &this)?;
        Ok((input, output))
    }

    fn start_listen(&mut self, this: Resource<UpstreamTcpSocket>) -> SocketResult<()> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get_mut(&this)?;
        let mut loopback = self
            .ctx
            .loopback
            .lock()
            .map_err(|e| super::network::SocketError::trap(wasmtime::format_err!("{e}")))?;
        socket.start_listen(&mut loopback).map_err(se)?;
        Ok(())
    }

    fn finish_listen(&mut self, this: Resource<UpstreamTcpSocket>) -> SocketResult<()> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get_mut(&this)?;
        socket.finish_listen().map_err(se)?;
        Ok(())
    }

    fn accept(
        &mut self,
        this: Resource<UpstreamTcpSocket>,
    ) -> SocketResult<(
        Resource<UpstreamTcpSocket>,
        Resource<DynInputStream>,
        Resource<DynOutputStream>,
    )> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get_mut(&this)?;

        let mut tcp_socket = socket.accept().map_err(se)?.ok_or(ErrorCode::WouldBlock)?;
        let (input, output) = tcp_socket.p2_streams()?;

        let tcp_socket = self.table.push(tcp_socket)?;
        let input_stream = self.table.push_child(input, &tcp_socket)?;
        let output_stream = self.table.push_child(output, &tcp_socket)?;

        let tcp_socket: Resource<UpstreamTcpSocket> = Resource::new_own(tcp_socket.rep());

        Ok((tcp_socket, input_stream, output_stream))
    }

    fn local_address(
        &mut self,
        this: Resource<UpstreamTcpSocket>,
    ) -> SocketResult<IpSocketAddress> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get(&this)?;
        Ok(socket_addr_to_ip_socket_address(
            socket.local_address().map_err(se)?,
        ))
    }

    fn remote_address(
        &mut self,
        this: Resource<UpstreamTcpSocket>,
    ) -> SocketResult<IpSocketAddress> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get(&this)?;
        Ok(socket_addr_to_ip_socket_address(
            socket.remote_address().map_err(se)?,
        ))
    }

    fn is_listening(&mut self, this: Resource<UpstreamTcpSocket>) -> wasmtime::Result<bool> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get(&this)?;
        Ok(socket.is_listening())
    }

    fn address_family(
        &mut self,
        this: Resource<UpstreamTcpSocket>,
    ) -> wasmtime::Result<IpAddressFamily> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get(&this)?;
        match socket.address_family() {
            super::SocketAddressFamily::Ipv4 => Ok(IpAddressFamily::Ipv4),
            super::SocketAddressFamily::Ipv6 => Ok(IpAddressFamily::Ipv6),
        }
    }

    fn set_listen_backlog_size(
        &mut self,
        this: Resource<UpstreamTcpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get_mut(&this)?;
        socket.set_listen_backlog_size(value).map_err(se)?;
        Ok(())
    }

    fn keep_alive_enabled(&mut self, this: Resource<UpstreamTcpSocket>) -> SocketResult<bool> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get(&this)?;
        socket.keep_alive_enabled().map_err(se)
    }

    fn set_keep_alive_enabled(
        &mut self,
        this: Resource<UpstreamTcpSocket>,
        value: bool,
    ) -> SocketResult<()> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get_mut(&this)?;
        socket.set_keep_alive_enabled(value).map_err(se)?;
        Ok(())
    }

    fn keep_alive_idle_time(&mut self, this: Resource<UpstreamTcpSocket>) -> SocketResult<u64> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get(&this)?;
        socket.keep_alive_idle_time().map_err(se)
    }

    fn set_keep_alive_idle_time(
        &mut self,
        this: Resource<UpstreamTcpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get_mut(&this)?;
        socket.set_keep_alive_idle_time(value).map_err(se)?;
        Ok(())
    }

    fn keep_alive_interval(&mut self, this: Resource<UpstreamTcpSocket>) -> SocketResult<u64> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get(&this)?;
        socket.keep_alive_interval().map_err(se)
    }

    fn set_keep_alive_interval(
        &mut self,
        this: Resource<UpstreamTcpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get_mut(&this)?;
        socket.set_keep_alive_interval(value).map_err(se)?;
        Ok(())
    }

    fn keep_alive_count(&mut self, this: Resource<UpstreamTcpSocket>) -> SocketResult<u32> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get(&this)?;
        socket.keep_alive_count().map_err(se)
    }

    fn set_keep_alive_count(
        &mut self,
        this: Resource<UpstreamTcpSocket>,
        value: u32,
    ) -> SocketResult<()> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get_mut(&this)?;
        socket.set_keep_alive_count(value).map_err(se)?;
        Ok(())
    }

    fn hop_limit(&mut self, this: Resource<UpstreamTcpSocket>) -> SocketResult<u8> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get(&this)?;
        socket.hop_limit().map_err(se)
    }

    fn set_hop_limit(&mut self, this: Resource<UpstreamTcpSocket>, value: u8) -> SocketResult<()> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get_mut(&this)?;
        socket.set_hop_limit(value).map_err(se)?;
        Ok(())
    }

    fn receive_buffer_size(&mut self, this: Resource<UpstreamTcpSocket>) -> SocketResult<u64> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get(&this)?;
        socket.receive_buffer_size().map_err(se)
    }

    fn set_receive_buffer_size(
        &mut self,
        this: Resource<UpstreamTcpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get_mut(&this)?;
        socket.set_receive_buffer_size(value).map_err(se)?;
        Ok(())
    }

    fn send_buffer_size(&mut self, this: Resource<UpstreamTcpSocket>) -> SocketResult<u64> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get(&this)?;
        socket.send_buffer_size().map_err(se)
    }

    fn set_send_buffer_size(
        &mut self,
        this: Resource<UpstreamTcpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get_mut(&this)?;
        socket.set_send_buffer_size(value).map_err(se)?;
        Ok(())
    }

    fn subscribe(
        &mut self,
        this: Resource<UpstreamTcpSocket>,
    ) -> wasmtime::Result<Resource<DynPollable>> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        wasmtime_wasi_io::poll::subscribe(self.table, this)
    }

    fn shutdown(
        &mut self,
        this: Resource<UpstreamTcpSocket>,
        shutdown_type: ShutdownType,
    ) -> SocketResult<()> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get(&this)?;

        match socket {
            super::tcp::TcpSocket::Network(socket) => {
                let how = match shutdown_type {
                    ShutdownType::Receive => std::net::Shutdown::Read,
                    ShutdownType::Send => std::net::Shutdown::Write,
                    ShutdownType::Both => std::net::Shutdown::Both,
                };
                let state = socket.p2_streaming_state().map_err(se)?;
                state.shutdown(how)?;
                Ok(())
            }
            super::tcp::TcpSocket::Loopback(socket) => {
                use super::loopback::TcpState;
                match &socket.state {
                    TcpState::P2Streaming { tx, rx, .. } => {
                        match shutdown_type {
                            ShutdownType::Receive => {
                                if let Ok(mut guard) = rx.try_lock()
                                    && let Some(mut rx) = guard.take()
                                {
                                    rx.close();
                                }
                            }
                            ShutdownType::Send => {
                                tx.lock()
                                    .map_err(|e| {
                                        super::network::SocketError::trap(wasmtime::format_err!(
                                            "{e}"
                                        ))
                                    })?
                                    .take();
                            }
                            ShutdownType::Both => {
                                tx.lock()
                                    .map_err(|e| {
                                        super::network::SocketError::trap(wasmtime::format_err!(
                                            "{e}"
                                        ))
                                    })?
                                    .take();
                                if let Ok(mut guard) = rx.try_lock()
                                    && let Some(mut rx) = guard.take()
                                {
                                    rx.close();
                                }
                            }
                        }
                        Ok(())
                    }
                    _ => Err(ErrorCode::InvalidState.into()),
                }
            }
            super::tcp::TcpSocket::Unspecified { .. } => Err(ErrorCode::InvalidState.into()),
        }
    }

    fn drop(&mut self, this: Resource<UpstreamTcpSocket>) -> wasmtime::Result<()> {
        let this = Resource::<super::tcp::TcpSocket>::new_own(this.rep());
        let socket = self.table.delete(this)?;
        let mut loopback = self
            .ctx
            .loopback
            .lock()
            .map_err(|e| wasmtime::format_err!("{e}"))?;
        socket.drop(&mut loopback)?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl Pollable for super::tcp::TcpSocket {
    async fn ready(&mut self) {
        <super::tcp::TcpSocket>::ready(self).await;
    }
}
