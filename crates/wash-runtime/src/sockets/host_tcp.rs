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

type UpstreamTcpSocket = wasmtime_wasi::p2::TcpSocket;
type UpstreamNetwork = wasmtime_wasi::p2::Network;

/// How many connections the policy may refuse in one `accept` call before the
/// guest is told to come back.
const MAX_REFUSED_ACCEPTS_PER_CALL: usize = 32;

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

        let allowed = network
            .check_socket_addr(local_address, SocketAddrUse::TcpBind)
            .map_err(super::network::socket_error_from_util)?;
        let local_address = allowed.addr;

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

    fn start_connect(
        &mut self,
        this: Resource<UpstreamTcpSocket>,
        network: Resource<UpstreamNetwork>,
        remote_address: IpSocketAddress,
    ) -> SocketResult<()> {
        let network = Resource::<super::network::Network>::new_borrow(network.rep());
        let network = self.table.get(&network)?;
        let remote_address = ip_socket_address_to_socket_addr(remote_address);

        let allowed = network
            .check_socket_addr(remote_address, SocketAddrUse::TcpConnect)
            .map_err(super::network::socket_error_from_util)?;
        let remote_address = allowed.addr;

        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get_mut(&this)?;
        let mut loopback = self
            .ctx
            .loopback
            .lock()
            .map_err(|e| super::network::SocketError::trap(wasmtime::format_err!("{e}")))?;
        let future = socket
            .start_connect(&remote_address, allowed.plane, &mut loopback)
            .map_err(se)?
            .connect(remote_address);
        socket.set_pending_connect(future).map_err(se)?;
        // Held until the guest drops the socket, so the budget bounds
        // concurrent connections rather than counting attempts.
        socket.hold_quota_slot(allowed.permit);

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

    async fn start_listen(&mut self, this: Resource<UpstreamTcpSocket>) -> SocketResult<()> {
        let this = Resource::<super::tcp::TcpSocket>::new_borrow(this.rep());
        let socket = self.table.get_mut(&this)?;
        let local_address = socket.local_address().map_err(se)?;
        self.ctx
            .socket_addr_check
            .check(local_address, SocketAddrUse::TcpListen)
            .map_err(super::network::socket_error_from_util)?;
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

        let mut refused = 0;
        let mut tcp_socket = loop {
            let accepted = socket.accept().map_err(se)?.ok_or(ErrorCode::WouldBlock)?;
            let reason = if matches!(accepted, super::tcp::TcpSocket::Network(_)) {
                SocketAddrUse::TcpAccept
            } else {
                SocketAddrUse::TcpAcceptVirtual
            };
            // A peer that reset between `accept` and here has no address left to
            // check. It is gone either way, so it is refused rather than made
            // the error of this whole `accept` call.
            let allowed = accepted
                .remote_address()
                .ok()
                .and_then(|addr| self.ctx.socket_addr_check.check(addr, reason).ok());
            if let Some(allowed) = allowed {
                let mut accepted = accepted;
                accepted.hold_quota_slot(allowed.permit);
                break accepted;
            }
            accepted.refuse();
            refused += 1;
            // A flood from a refused address would otherwise keep this one host
            // call accepting and dropping for as long as it lasts. The guest's
            // pollable is still armed, so it comes straight back.
            if refused >= MAX_REFUSED_ACCEPTS_PER_CALL {
                return Err(ErrorCode::WouldBlock.into());
            }
        };
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::host::quota::{GuestConnectionQuota, QuotaLimits};
    use crate::sockets::{
        AddrDecision, DenyReason, Plane, SocketAddrCheck, TcpSocket, WasiSocketsCtx,
    };
    use bytes::Bytes;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::timeout;
    use wasmtime::component::ResourceTable;
    use wasmtime_wasi::p2::bindings::sockets::{instance_network, tcp_create_socket};

    struct Harness {
        ctx: WasiSocketsCtx,
        table: ResourceTable,
        network: u32,
    }

    impl Harness {
        fn new(check: SocketAddrCheck) -> Self {
            let mut h = Self {
                ctx: WasiSocketsCtx {
                    socket_addr_check: check,
                    ..Default::default()
                },
                table: ResourceTable::new(),
                network: 0,
            };
            h.network = instance_network::Host::instance_network(&mut h.view())
                .unwrap()
                .rep();
            h
        }

        fn view(&mut self) -> WasiSocketsCtxView<'_> {
            WasiSocketsCtxView {
                ctx: &mut self.ctx,
                table: &mut self.table,
            }
        }

        fn create(&mut self) -> u32 {
            tcp_create_socket::Host::create_tcp_socket(&mut self.view(), IpAddressFamily::Ipv4)
                .unwrap()
                .rep()
        }

        async fn ready(&mut self, rep: u32) {
            timeout(
                Duration::from_secs(5),
                self.table
                    .get_mut(&Resource::<TcpSocket>::new_borrow(rep))
                    .unwrap()
                    .ready(),
            )
            .await
            .expect("socket readiness timed out");
        }

        async fn listen(&mut self) -> (u32, SocketAddr) {
            let rep = self.create();
            let network = self.network;
            tcp::HostTcpSocket::start_bind(
                &mut self.view(),
                Resource::new_borrow(rep),
                Resource::new_borrow(network),
                addr("127.0.0.1:0"),
            )
            .await
            .unwrap();
            tcp::HostTcpSocket::finish_bind(&mut self.view(), Resource::new_borrow(rep)).unwrap();
            tcp::HostTcpSocket::start_listen(&mut self.view(), Resource::new_borrow(rep))
                .await
                .unwrap();
            tcp::HostTcpSocket::finish_listen(&mut self.view(), Resource::new_borrow(rep)).unwrap();
            let address =
                tcp::HostTcpSocket::local_address(&mut self.view(), Resource::new_borrow(rep))
                    .unwrap();
            (rep, ip_socket_address_to_socket_addr(address))
        }

        async fn connect(
            &mut self,
            address: SocketAddr,
        ) -> (u32, Resource<DynInputStream>, Resource<DynOutputStream>) {
            let rep = self.create();
            let network = self.network;
            tcp::HostTcpSocket::start_connect(
                &mut self.view(),
                Resource::new_borrow(rep),
                Resource::new_borrow(network),
                socket_addr_to_ip_socket_address(address),
            )
            .unwrap();
            let (input, output) = match tcp::HostTcpSocket::finish_connect(
                &mut self.view(),
                Resource::new_borrow(rep),
            ) {
                Ok(streams) => streams,
                Err(error) => {
                    assert_eq!(error.downcast().unwrap(), ErrorCode::WouldBlock);
                    self.ready(rep).await;
                    tcp::HostTcpSocket::finish_connect(&mut self.view(), Resource::new_borrow(rep))
                        .unwrap()
                }
            };
            (rep, input, output)
        }

        fn close(
            &mut self,
            rep: u32,
            input: Resource<DynInputStream>,
            output: Resource<DynOutputStream>,
        ) {
            self.table.delete(input).unwrap();
            self.table.delete(output).unwrap();
            tcp::HostTcpSocket::drop(&mut self.view(), Resource::new_own(rep)).unwrap();
        }
    }

    fn addr(value: &str) -> IpSocketAddress {
        socket_addr_to_ip_socket_address(value.parse().unwrap())
    }

    fn assert_code<T>(result: SocketResult<T>, expected: ErrorCode) {
        let error = result
            .err()
            .expect("expected a socket error")
            .downcast()
            .unwrap();
        assert_eq!(error, expected);
    }

    async fn read(h: &mut Harness, input: &Resource<DynInputStream>, len: usize) -> Bytes {
        let stream = h.table.get_mut(input).unwrap();
        timeout(Duration::from_secs(5), async {
            loop {
                stream.ready().await;
                let bytes = stream.read(len).unwrap();
                if !bytes.is_empty() {
                    break bytes;
                }
            }
        })
        .await
        .unwrap()
    }

    async fn write(h: &mut Harness, output: &Resource<DynOutputStream>, data: &'static [u8]) {
        let stream = h.table.get_mut(output).unwrap();
        timeout(Duration::from_secs(5), stream.ready())
            .await
            .unwrap();
        assert!(stream.check_write().unwrap() >= data.len());
        stream.write(Bytes::from_static(data)).unwrap();
        stream.flush().unwrap();
    }

    #[tokio::test]
    async fn virtual_connections_preserve_addresses_backpressure_and_half_close() {
        let mut h = Harness::new(SocketAddrCheck::new(|addr, _| {
            AddrDecision::allow_by_address(addr)
        }));
        let (listener, address) = h.listen().await;
        assert!(
            tcp::HostTcpSocket::is_listening(&mut h.view(), Resource::new_borrow(listener))
                .unwrap()
        );
        assert_code(
            tcp::HostTcpSocket::accept(&mut h.view(), Resource::new_borrow(listener)),
            ErrorCode::WouldBlock,
        );
        let (client, input, output) = h.connect(address).await;
        h.ready(listener).await;
        let (server, server_in, server_out) =
            tcp::HostTcpSocket::accept(&mut h.view(), Resource::new_borrow(listener)).unwrap();
        let server = server.rep();
        let local =
            tcp::HostTcpSocket::local_address(&mut h.view(), Resource::new_borrow(client)).unwrap();
        assert_eq!(
            ip_socket_address_to_socket_addr(
                tcp::HostTcpSocket::remote_address(&mut h.view(), Resource::new_borrow(server))
                    .unwrap()
            ),
            ip_socket_address_to_socket_addr(local),
        );
        assert_eq!(
            ip_socket_address_to_socket_addr(
                tcp::HostTcpSocket::remote_address(&mut h.view(), Resource::new_borrow(client))
                    .unwrap()
            ),
            address,
        );
        assert!(h.table.get_mut(&input).unwrap().read(1).unwrap().is_empty());
        assert!(h.table.get_mut(&input).unwrap().read(0).unwrap().is_empty());
        let capacity = h.table.get_mut(&output).unwrap().check_write().unwrap();
        h.table
            .get_mut(&output)
            .unwrap()
            .write(Bytes::from(vec![42; capacity]))
            .unwrap();
        assert_eq!(h.table.get_mut(&output).unwrap().check_write().unwrap(), 0);
        assert!(matches!(
            h.table
                .get_mut(&output)
                .unwrap()
                .write(Bytes::from_static(b"x")),
            Err(wasmtime_wasi_io::streams::StreamError::Trap(_))
        ));
        assert_eq!(
            h.table.get_mut(&server_in).unwrap().read(3).unwrap(),
            &[42; 3][..]
        );
        assert!(h.table.get_mut(&output).unwrap().check_write().unwrap() > 0);
        assert_eq!(read(&mut h, &server_in, capacity).await.len(), capacity - 3);
        tcp::HostTcpSocket::shutdown(
            &mut h.view(),
            Resource::new_borrow(client),
            ShutdownType::Send,
        )
        .unwrap();
        assert!(matches!(
            h.table.get_mut(&output).unwrap().check_write(),
            Err(wasmtime_wasi_io::streams::StreamError::Closed)
        ));
        assert!(matches!(
            h.table.get_mut(&server_in).unwrap().read(1),
            Err(wasmtime_wasi_io::streams::StreamError::Closed)
        ));
        write(&mut h, &server_out, b"reply").await;
        assert_eq!(read(&mut h, &input, 2).await, &b"re"[..]);
        assert_eq!(read(&mut h, &input, 20).await, &b"ply"[..]);
        tcp::HostTcpSocket::shutdown(
            &mut h.view(),
            Resource::new_borrow(client),
            ShutdownType::Receive,
        )
        .unwrap();
        assert!(matches!(
            h.table.get_mut(&input).unwrap().read(1),
            Err(wasmtime_wasi_io::streams::StreamError::Closed)
        ));
        tcp::HostTcpSocket::shutdown(
            &mut h.view(),
            Resource::new_borrow(server),
            ShutdownType::Both,
        )
        .unwrap();
        h.close(client, input, output);
        h.close(server, server_in, server_out);
        tcp::HostTcpSocket::drop(&mut h.view(), Resource::new_own(listener)).unwrap();
        assert!(
            h.ctx.loopback.lock().unwrap().bind_tcp(address).is_ok(),
            "dropping the listener releases its port"
        );
    }

    #[tokio::test]
    async fn native_connection_streams_hold_the_quota_until_socket_drop() {
        let quota = GuestConnectionQuota::new(
            QuotaLimits {
                outbound_sockets: 1,
                ..Default::default()
            },
            None,
        );
        let check_quota = quota.clone();
        let mut h = Harness::new(SocketAddrCheck::new(move |addr, reason| {
            let mut allowed = AddrDecision::allow_on(addr, Plane::Host)
                .into_allowed()
                .unwrap();
            if matches!(reason, SocketAddrUse::TcpCreate) {
                let Some(permit) = check_quota.try_acquire_outbound_socket() else {
                    return AddrDecision::Deny(DenyReason::NoCapacity);
                };
                allowed.permit = Some(permit);
            }
            AddrDecision::Allow(allowed)
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (client, input, output) = h.connect(listener.local_addr().unwrap()).await;
        let (mut peer, _) = listener.accept().await.unwrap();
        assert_eq!(quota.outbound_sockets_available(), 0);
        assert_code(
            tcp_create_socket::Host::create_tcp_socket(&mut h.view(), IpAddressFamily::Ipv4),
            ErrorCode::OutOfMemory,
        );
        write(&mut h, &output, b"request").await;
        let mut buffer = [0; 7];
        timeout(Duration::from_secs(5), peer.read_exact(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buffer, b"request");
        tcp::HostTcpSocket::shutdown(
            &mut h.view(),
            Resource::new_borrow(client),
            ShutdownType::Send,
        )
        .unwrap();
        assert_eq!(
            timeout(Duration::from_secs(5), peer.read(&mut buffer))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        peer.write_all(b"reply").await.unwrap();
        let mut reply = Vec::new();
        while reply.len() < 5 {
            reply.extend(read(&mut h, &input, 5 - reply.len()).await);
        }
        assert_eq!(reply, b"reply");
        tcp::HostTcpSocket::shutdown(
            &mut h.view(),
            Resource::new_borrow(client),
            ShutdownType::Both,
        )
        .unwrap();
        assert!(matches!(
            h.table.get_mut(&input).unwrap().read(1),
            Err(wasmtime_wasi_io::streams::StreamError::Closed)
        ));
        h.close(client, input, output);
        assert_eq!(quota.outbound_sockets_available(), 1);
    }

    #[tokio::test]
    async fn unspecified_listener_accepts_native_connections_and_releases_its_port() {
        let mut h = Harness::new(SocketAddrCheck::new(|addr, _| {
            AddrDecision::allow_by_address(addr)
        }));
        let listener = h.create();
        {
            let mut loopback = h.ctx.loopback.lock().unwrap();
            h.table
                .get_mut(&Resource::<TcpSocket>::new_borrow(listener))
                .unwrap()
                .start_bind("0.0.0.0:0".parse().unwrap(), &mut loopback)
                .unwrap();
        }
        tcp::HostTcpSocket::finish_bind(&mut h.view(), Resource::new_borrow(listener)).unwrap();
        tcp::HostTcpSocket::set_listen_backlog_size(
            &mut h.view(),
            Resource::new_borrow(listener),
            2,
        )
        .unwrap();
        tcp::HostTcpSocket::start_listen(&mut h.view(), Resource::new_borrow(listener))
            .await
            .unwrap();
        tcp::HostTcpSocket::finish_listen(&mut h.view(), Resource::new_borrow(listener)).unwrap();
        let address = ip_socket_address_to_socket_addr(
            tcp::HostTcpSocket::local_address(&mut h.view(), Resource::new_borrow(listener))
                .unwrap(),
        );
        assert!(address.ip().is_loopback());
        let mut peer = tokio::net::TcpStream::connect(address).await.unwrap();
        h.ready(listener).await;
        let (server, input, output) =
            tcp::HostTcpSocket::accept(&mut h.view(), Resource::new_borrow(listener)).unwrap();
        assert_eq!(
            ip_socket_address_to_socket_addr(
                tcp::HostTcpSocket::remote_address(
                    &mut h.view(),
                    Resource::new_borrow(server.rep())
                )
                .unwrap()
            ),
            peer.local_addr().unwrap()
        );
        write(&mut h, &output, b"accepted").await;
        let mut bytes = [0; 8];
        timeout(Duration::from_secs(5), peer.read_exact(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&bytes, b"accepted");
        h.close(server.rep(), input, output);
        tcp::HostTcpSocket::drop(&mut h.view(), Resource::new_own(listener)).unwrap();
        assert!(h.ctx.loopback.lock().unwrap().tcp_ipv4.is_empty());
        assert!(tokio::net::TcpListener::bind(address).await.is_ok());
    }

    #[tokio::test]
    async fn a_denied_accept_closes_the_peer_and_keeps_the_listener_usable() {
        let mut h = Harness::new(SocketAddrCheck::new(|addr, reason| {
            if matches!(reason, SocketAddrUse::TcpAcceptVirtual) {
                AddrDecision::Deny(DenyReason::NotPermitted)
            } else {
                AddrDecision::allow_by_address(addr)
            }
        }));
        let (listener, address) = h.listen().await;
        let (client, input, output) = h.connect(address).await;
        h.ready(listener).await;
        assert_code(
            tcp::HostTcpSocket::accept(&mut h.view(), Resource::new_borrow(listener)),
            ErrorCode::WouldBlock,
        );
        assert!(
            tcp::HostTcpSocket::is_listening(&mut h.view(), Resource::new_borrow(listener))
                .unwrap()
        );
        assert!(matches!(
            h.table.get_mut(&input).unwrap().read(1),
            Err(wasmtime_wasi_io::streams::StreamError::Closed)
        ));
        h.close(client, input, output);
        tcp::HostTcpSocket::drop(&mut h.view(), Resource::new_own(listener)).unwrap();
        assert!(h.ctx.loopback.lock().unwrap().tcp_ipv4.is_empty());
    }

    #[tokio::test]
    async fn refused_virtual_connect_does_not_reserve_a_local_port() {
        let mut h = Harness::new(SocketAddrCheck::new(|addr, _| {
            AddrDecision::allow_by_address(addr)
        }));
        let client = h.create();
        let network = h.network;
        assert_code(
            tcp::HostTcpSocket::start_connect(
                &mut h.view(),
                Resource::new_borrow(client),
                Resource::new_borrow(network),
                addr("127.0.0.1:80"),
            ),
            ErrorCode::ConnectionRefused,
        );
        tcp::HostTcpSocket::drop(&mut h.view(), Resource::new_own(client)).unwrap();
        assert!(h.ctx.loopback.lock().unwrap().tcp_ipv4.is_empty());
    }

    #[tokio::test]
    async fn native_connect_reports_a_deferred_refusal_after_readiness() {
        let mut h = Harness::new(SocketAddrCheck::new(|addr, _| {
            AddrDecision::allow_on(addr, Plane::Host)
        }));
        let client = h.create();
        let (release, pending) = tokio::sync::oneshot::channel();
        {
            let socket = h
                .table
                .get_mut(&Resource::<TcpSocket>::new_borrow(client))
                .unwrap();
            let mut loopback = h.ctx.loopback.lock().unwrap();
            drop(
                socket
                    .start_connect(&"127.0.0.1:80".parse().unwrap(), Plane::Host, &mut loopback)
                    .unwrap(),
            );
            socket
                .set_pending_connect(async move {
                    pending.await.unwrap();
                    Err(std::io::ErrorKind::ConnectionRefused.into())
                })
                .unwrap();
        }
        assert_code(
            tcp::HostTcpSocket::finish_connect(&mut h.view(), Resource::new_borrow(client)),
            ErrorCode::WouldBlock,
        );
        release.send(()).unwrap();
        h.ready(client).await;
        assert_code(
            tcp::HostTcpSocket::finish_connect(&mut h.view(), Resource::new_borrow(client)),
            ErrorCode::ConnectionRefused,
        );
        assert_code(
            tcp::HostTcpSocket::remote_address(&mut h.view(), Resource::new_borrow(client)),
            ErrorCode::InvalidState,
        );
        tcp::HostTcpSocket::drop(&mut h.view(), Resource::new_own(client)).unwrap();
    }

    #[tokio::test]
    async fn invalid_lifecycle_calls_return_socket_errors() {
        let mut h = Harness::new(SocketAddrCheck::new(|addr, _| {
            AddrDecision::allow_by_address(addr)
        }));
        let rep = h.create();
        assert_code(
            tcp::HostTcpSocket::finish_bind(&mut h.view(), Resource::new_borrow(rep)),
            ErrorCode::NotInProgress,
        );
        assert_code(
            tcp::HostTcpSocket::finish_listen(&mut h.view(), Resource::new_borrow(rep)),
            ErrorCode::NotInProgress,
        );
        assert_code(
            tcp::HostTcpSocket::accept(&mut h.view(), Resource::new_borrow(rep)),
            ErrorCode::InvalidState,
        );
        assert_code(
            tcp::HostTcpSocket::remote_address(&mut h.view(), Resource::new_borrow(rep)),
            ErrorCode::InvalidState,
        );
        assert_code(
            tcp::HostTcpSocket::shutdown(
                &mut h.view(),
                Resource::new_borrow(rep),
                ShutdownType::Both,
            ),
            ErrorCode::InvalidState,
        );
        let network = h.network;
        assert_code(
            tcp::HostTcpSocket::start_connect(
                &mut h.view(),
                Resource::new_borrow(rep),
                Resource::new_borrow(network),
                addr("127.0.0.1:0"),
            ),
            ErrorCode::InvalidArgument,
        );
        let pollable =
            tcp::HostTcpSocket::subscribe(&mut h.view(), Resource::new_borrow(rep)).unwrap();
        assert!(
            tcp::HostTcpSocket::drop(&mut h.view(), Resource::new_own(rep)).is_err(),
            "a live pollable keeps its socket parent alive"
        );
        h.table.delete(pollable).unwrap();
        tcp::HostTcpSocket::drop(&mut h.view(), Resource::new_own(rep)).unwrap();
        assert!(
            tcp::HostTcpSocket::local_address(&mut h.view(), Resource::new_borrow(rep)).is_err()
        );
    }

    #[tokio::test]
    async fn policy_denials_do_not_start_binding_or_connecting() {
        let mut h = Harness::new(SocketAddrCheck::new(|addr, reason| {
            if matches!(reason, SocketAddrUse::TcpCreate) {
                AddrDecision::allow_by_address(addr)
            } else {
                AddrDecision::Deny(DenyReason::NotPermitted)
            }
        }));
        let rep = h.create();
        let network = h.network;
        assert_code(
            tcp::HostTcpSocket::start_bind(
                &mut h.view(),
                Resource::new_borrow(rep),
                Resource::new_borrow(network),
                addr("0.0.0.0:0"),
            )
            .await,
            ErrorCode::AccessDenied,
        );
        assert_code(
            tcp::HostTcpSocket::start_connect(
                &mut h.view(),
                Resource::new_borrow(rep),
                Resource::new_borrow(network),
                addr("127.0.0.1:80"),
            ),
            ErrorCode::AccessDenied,
        );
        assert_code(
            tcp::HostTcpSocket::finish_bind(&mut h.view(), Resource::new_borrow(rep)),
            ErrorCode::NotInProgress,
        );
        tcp::HostTcpSocket::drop(&mut h.view(), Resource::new_own(rep)).unwrap();
    }

    #[tokio::test]
    async fn socket_options_round_trip_through_native_and_virtual_bindings() {
        let mut h = Harness::new(SocketAddrCheck::new(|addr, _| {
            AddrDecision::allow_by_address(addr)
        }));
        let native = h.create();
        let (virtual_socket, _) = h.listen().await;
        for rep in [native, virtual_socket] {
            assert!(matches!(
                tcp::HostTcpSocket::address_family(&mut h.view(), Resource::new_borrow(rep))
                    .unwrap(),
                IpAddressFamily::Ipv4
            ));
            tcp::HostTcpSocket::set_keep_alive_enabled(
                &mut h.view(),
                Resource::new_borrow(rep),
                true,
            )
            .unwrap();
            assert!(
                tcp::HostTcpSocket::keep_alive_enabled(&mut h.view(), Resource::new_borrow(rep))
                    .unwrap()
            );
            macro_rules! option {
                ($set:ident, $get:ident, $value:expr) => {
                    tcp::HostTcpSocket::$set(&mut h.view(), Resource::new_borrow(rep), $value)
                        .unwrap();
                    assert_eq!(
                        tcp::HostTcpSocket::$get(&mut h.view(), Resource::new_borrow(rep)).unwrap(),
                        $value
                    );
                };
            }
            option!(
                set_keep_alive_idle_time,
                keep_alive_idle_time,
                10_000_000_000
            );
            option!(set_keep_alive_interval, keep_alive_interval, 2_000_000_000);
            option!(set_keep_alive_count, keep_alive_count, 3);
            option!(set_hop_limit, hop_limit, 42);
            tcp::HostTcpSocket::set_receive_buffer_size(
                &mut h.view(),
                Resource::new_borrow(rep),
                8192,
            )
            .unwrap();
            tcp::HostTcpSocket::set_send_buffer_size(
                &mut h.view(),
                Resource::new_borrow(rep),
                8192,
            )
            .unwrap();
            assert!(
                tcp::HostTcpSocket::receive_buffer_size(&mut h.view(), Resource::new_borrow(rep))
                    .unwrap()
                    >= 8192
            );
            assert!(
                tcp::HostTcpSocket::send_buffer_size(&mut h.view(), Resource::new_borrow(rep))
                    .unwrap()
                    >= 8192
            );
            assert_code(
                tcp::HostTcpSocket::set_hop_limit(&mut h.view(), Resource::new_borrow(rep), 0),
                ErrorCode::InvalidArgument,
            );
            assert_code(
                tcp::HostTcpSocket::set_keep_alive_count(
                    &mut h.view(),
                    Resource::new_borrow(rep),
                    0,
                ),
                ErrorCode::InvalidArgument,
            );
            tcp::HostTcpSocket::drop(&mut h.view(), Resource::new_own(rep)).unwrap();
        }
    }
}
