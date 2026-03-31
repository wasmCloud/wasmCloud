use super::host_network::{ip_socket_address_to_socket_addr, socket_addr_to_ip_socket_address};
use super::network::{SocketError, SocketResult};
use super::p2_udp::{IncomingDatagramStream, OutgoingDatagramStream};
use super::util::{is_valid_address_family, is_valid_remote_address};
use super::{
    MAX_UDP_DATAGRAM_SIZE, SocketAddrUse, SocketAddressFamily, UdpSocket, WasiSocketsCtxView,
};
use async_trait::async_trait;
use std::net::SocketAddr;
use tokio::io::Interest;
use wasmtime::component::Resource;
use wasmtime_wasi::p2::bindings::sockets::network::{
    ErrorCode, IpAddressFamily, IpSocketAddress, Network,
};
use wasmtime_wasi::p2::bindings::sockets::udp;
use wasmtime_wasi_io::poll::DynPollable;
use wasmtime_wasi_io::poll::Pollable;

/// Rebind a borrowed resource from the upstream type to our local type.
fn rebind_udp_borrow(this: &Resource<udp::UdpSocket>) -> Resource<UdpSocket> {
    Resource::<UdpSocket>::new_borrow(this.rep())
}

/// Rebind an owned resource from the upstream type to our local type (for drop).
fn rebind_udp_own(this: Resource<udp::UdpSocket>) -> Resource<UdpSocket> {
    Resource::<UdpSocket>::new_own(this.rep())
}

/// Rebind a borrowed resource from the upstream network type to our local type.
fn rebind_network_borrow(this: &Resource<Network>) -> Resource<super::network::Network> {
    Resource::<super::network::Network>::new_borrow(this.rep())
}

/// Rebind a borrowed resource from the upstream incoming datagram stream type to our local type.
fn rebind_incoming_borrow(
    this: &Resource<udp::IncomingDatagramStream>,
) -> Resource<IncomingDatagramStream> {
    Resource::<IncomingDatagramStream>::new_borrow(this.rep())
}

/// Rebind an owned resource from the upstream incoming datagram stream type to our local type (for drop).
fn rebind_incoming_own(
    this: Resource<udp::IncomingDatagramStream>,
) -> Resource<IncomingDatagramStream> {
    Resource::<IncomingDatagramStream>::new_own(this.rep())
}

/// Rebind a borrowed resource from the upstream outgoing datagram stream type to our local type.
fn rebind_outgoing_borrow(
    this: &Resource<udp::OutgoingDatagramStream>,
) -> Resource<OutgoingDatagramStream> {
    Resource::<OutgoingDatagramStream>::new_borrow(this.rep())
}

/// Rebind an owned resource from the upstream outgoing datagram stream type to our local type (for drop).
fn rebind_outgoing_own(
    this: Resource<udp::OutgoingDatagramStream>,
) -> Resource<OutgoingDatagramStream> {
    Resource::<OutgoingDatagramStream>::new_own(this.rep())
}

impl udp::Host for WasiSocketsCtxView<'_> {}

impl udp::HostUdpSocket for WasiSocketsCtxView<'_> {
    async fn start_bind(
        &mut self,
        this: Resource<udp::UdpSocket>,
        network: Resource<Network>,
        local_address: IpSocketAddress,
    ) -> SocketResult<()> {
        let this = rebind_udp_borrow(&this);
        let network = rebind_network_borrow(&network);
        let local_address = ip_socket_address_to_socket_addr(local_address);
        let check = self.table.get(&network)?.socket_addr_check.clone();
        check
            .check(local_address, SocketAddrUse::UdpBind)
            .await
            .map_err(super::network::socket_error_from_io)?;

        let socket = self.table.get_mut(&this)?;

        let mut loopback = self
            .ctx
            .loopback
            .lock()
            .map_err(|e| SocketError::trap(wasmtime::format_err!("{e}")))?;
        socket
            .bind(local_address, &mut loopback)
            .map_err(super::network::socket_error_from_util)?;
        socket.set_socket_addr_check(Some(check));

        Ok(())
    }

    fn finish_bind(&mut self, this: Resource<udp::UdpSocket>) -> SocketResult<()> {
        let this = rebind_udp_borrow(&this);
        self.table
            .get_mut(&this)?
            .finish_bind()
            .map_err(super::network::socket_error_from_util)?;
        Ok(())
    }

    async fn stream(
        &mut self,
        this: Resource<udp::UdpSocket>,
        remote_address: Option<IpSocketAddress>,
    ) -> SocketResult<(
        Resource<udp::IncomingDatagramStream>,
        Resource<udp::OutgoingDatagramStream>,
    )> {
        let this = rebind_udp_borrow(&this);

        let has_active_streams = self
            .table
            .iter_children(&this)?
            .any(|c| c.is::<IncomingDatagramStream>() || c.is::<OutgoingDatagramStream>());

        if has_active_streams {
            return Err(SocketError::trap(wasmtime::format_err!(
                "UDP streams not dropped yet"
            )));
        }

        let socket = self.table.get_mut(&this)?;
        let remote_address = remote_address.map(ip_socket_address_to_socket_addr);

        if !socket.is_bound() {
            return Err(ErrorCode::InvalidState.into());
        }

        if let Some(connect_addr) = remote_address {
            let Some(check) = socket.socket_addr_check() else {
                return Err(ErrorCode::InvalidState.into());
            };
            check
                .check(connect_addr, SocketAddrUse::UdpConnect)
                .await
                .map_err(super::network::socket_error_from_io)?;
            let mut loopback = self
                .ctx
                .loopback
                .lock()
                .map_err(|e| SocketError::trap(wasmtime::format_err!("{e}")))?;
            socket
                .connect(connect_addr, &mut loopback)
                .map_err(super::network::socket_error_from_util)?;
        } else if socket.is_connected() {
            let mut loopback = self
                .ctx
                .loopback
                .lock()
                .map_err(|e| SocketError::trap(wasmtime::format_err!("{e}")))?;
            socket
                .disconnect(&mut loopback)
                .map_err(super::network::socket_error_from_util)?;
        }
        let is_loopback = remote_address.map(|addr| addr.ip().to_canonical().is_loopback());

        let (incoming_stream, outgoing_stream) = match (socket, is_loopback) {
            (UdpSocket::Network(socket), ..)
            | (UdpSocket::Unspecified { net: socket, .. }, Some(false)) => (
                IncomingDatagramStream::Network(super::p2_udp::NetworkIncomingDatagramStream {
                    inner: socket.socket().clone(),
                    remote_address,
                }),
                OutgoingDatagramStream::Network(super::p2_udp::NetworkOutgoingDatagramStream {
                    inner: socket.socket().clone(),
                    remote_address,
                    family: socket.address_family(),
                    check_send_permit_count: 0,
                    socket_addr_check: socket.socket_addr_check().cloned(),
                }),
            ),
            (UdpSocket::Loopback(socket), ..)
            | (UdpSocket::Unspecified { lo: socket, .. }, Some(true)) => {
                let (rx, tx) = socket
                    .p2_udp_streams(remote_address)
                    .map_err(super::network::socket_error_from_util)?;
                (
                    IncomingDatagramStream::Loopback(rx),
                    OutgoingDatagramStream::Loopback(tx),
                )
            }
            (UdpSocket::Unspecified { lo, net }, None) => {
                let (lo_rx, lo_tx) = lo
                    .p2_udp_streams(remote_address)
                    .map_err(super::network::socket_error_from_util)?;
                (
                    IncomingDatagramStream::Unspecified {
                        lo: lo_rx,
                        net: super::p2_udp::NetworkIncomingDatagramStream {
                            inner: net.socket().clone(),
                            remote_address,
                        },
                    },
                    OutgoingDatagramStream::Unspecified {
                        lo: lo_tx,
                        net: super::p2_udp::NetworkOutgoingDatagramStream {
                            inner: net.socket().clone(),
                            remote_address,
                            family: net.address_family(),
                            check_send_permit_count: 0,
                            socket_addr_check: net.socket_addr_check().cloned(),
                        },
                    },
                )
            }
        };
        let incoming: Resource<IncomingDatagramStream> =
            self.table.push_child(incoming_stream, &this)?;
        let outgoing: Resource<OutgoingDatagramStream> =
            self.table.push_child(outgoing_stream, &this)?;
        Ok((
            Resource::new_own(incoming.rep()),
            Resource::new_own(outgoing.rep()),
        ))
    }

    fn local_address(&mut self, this: Resource<udp::UdpSocket>) -> SocketResult<IpSocketAddress> {
        let this = rebind_udp_borrow(&this);
        let socket = self.table.get(&this)?;
        socket
            .local_address()
            .map(socket_addr_to_ip_socket_address)
            .map_err(super::network::socket_error_from_util)
    }

    fn remote_address(&mut self, this: Resource<udp::UdpSocket>) -> SocketResult<IpSocketAddress> {
        let this = rebind_udp_borrow(&this);
        let socket = self.table.get(&this)?;
        socket
            .remote_address()
            .map(socket_addr_to_ip_socket_address)
            .map_err(super::network::socket_error_from_util)
    }

    fn address_family(
        &mut self,
        this: Resource<udp::UdpSocket>,
    ) -> wasmtime::Result<IpAddressFamily> {
        let this = rebind_udp_borrow(&this);
        let socket = self.table.get(&this)?;
        Ok(socket.address_family().into())
    }

    fn unicast_hop_limit(&mut self, this: Resource<udp::UdpSocket>) -> SocketResult<u8> {
        let this = rebind_udp_borrow(&this);
        let socket = self.table.get(&this)?;
        socket
            .unicast_hop_limit()
            .map_err(super::network::socket_error_from_util)
    }

    fn set_unicast_hop_limit(
        &mut self,
        this: Resource<udp::UdpSocket>,
        value: u8,
    ) -> SocketResult<()> {
        let this = rebind_udp_borrow(&this);
        let socket = self.table.get_mut(&this)?;
        socket
            .set_unicast_hop_limit(value)
            .map_err(super::network::socket_error_from_util)?;
        Ok(())
    }

    fn receive_buffer_size(&mut self, this: Resource<udp::UdpSocket>) -> SocketResult<u64> {
        let this = rebind_udp_borrow(&this);
        let socket = self.table.get(&this)?;
        socket
            .receive_buffer_size()
            .map_err(super::network::socket_error_from_util)
    }

    fn set_receive_buffer_size(
        &mut self,
        this: Resource<udp::UdpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        let this = rebind_udp_borrow(&this);
        let socket = self.table.get_mut(&this)?;
        socket
            .set_receive_buffer_size(value)
            .map_err(super::network::socket_error_from_util)?;
        Ok(())
    }

    fn send_buffer_size(&mut self, this: Resource<udp::UdpSocket>) -> SocketResult<u64> {
        let this = rebind_udp_borrow(&this);
        let socket = self.table.get(&this)?;
        socket
            .send_buffer_size()
            .map_err(super::network::socket_error_from_util)
    }

    fn set_send_buffer_size(
        &mut self,
        this: Resource<udp::UdpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        let this = rebind_udp_borrow(&this);
        let socket = self.table.get_mut(&this)?;
        socket
            .set_send_buffer_size(value)
            .map_err(super::network::socket_error_from_util)?;
        Ok(())
    }

    fn subscribe(
        &mut self,
        this: Resource<udp::UdpSocket>,
    ) -> wasmtime::Result<Resource<DynPollable>> {
        let this = rebind_udp_borrow(&this);
        wasmtime_wasi_io::poll::subscribe(self.table, this)
    }

    fn drop(&mut self, this: Resource<udp::UdpSocket>) -> wasmtime::Result<()> {
        let this = rebind_udp_own(this);
        // As in the filesystem implementation, we assume closing a socket
        // doesn't block.
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

#[async_trait]
impl Pollable for UdpSocket {
    async fn ready(&mut self) {
        // None of the socket-level operations block natively
    }
}

impl udp::HostIncomingDatagramStream for WasiSocketsCtxView<'_> {
    fn receive(
        &mut self,
        this: Resource<udp::IncomingDatagramStream>,
        max_results: u64,
    ) -> SocketResult<Vec<udp::IncomingDatagram>> {
        let this = rebind_incoming_borrow(&this);

        // Returns Ok(None) when the message was dropped.
        fn recv_one(
            stream: &super::p2_udp::NetworkIncomingDatagramStream,
        ) -> SocketResult<Option<udp::IncomingDatagram>> {
            let mut buf = [0; MAX_UDP_DATAGRAM_SIZE];
            let (size, received_addr) = stream
                .inner
                .try_recv_from(&mut buf)
                .map_err(super::network::socket_error_from_io)?;
            debug_assert!(size <= buf.len());

            match stream.remote_address {
                Some(connected_addr) if connected_addr != received_addr => {
                    // Normally, this should have already been checked for us by the OS.
                    return Ok(None);
                }
                _ => {}
            }

            Ok(Some(udp::IncomingDatagram {
                data: buf.get(..size).unwrap_or_default().into(),
                remote_address: socket_addr_to_ip_socket_address(received_addr),
            }))
        }

        let max_results: usize = max_results.try_into().unwrap_or(usize::MAX);

        if max_results == 0 {
            return Ok(vec![]);
        }

        let mut datagrams = vec![];

        let stream = self.table.get_mut(&this)?;
        let stream = match stream {
            IncomingDatagramStream::Network(stream) => stream,
            IncomingDatagramStream::Loopback(stream) => {
                stream
                    .recv(&mut datagrams, max_results)
                    .map_err(super::network::socket_error_from_util)?;
                return Ok(datagrams);
            }
            IncomingDatagramStream::Unspecified { net, lo } => {
                lo.recv(&mut datagrams, max_results)
                    .map_err(super::network::socket_error_from_util)?;
                net
            }
        };

        while datagrams.len() < max_results {
            match recv_one(stream) {
                Ok(Some(datagram)) => {
                    datagrams.push(datagram);
                }
                Ok(None) => {
                    // Message was dropped
                }
                Err(_) if !datagrams.is_empty() => {
                    return Ok(datagrams);
                }
                Err(e) if matches!(e.downcast_ref(), Some(ErrorCode::WouldBlock)) => {
                    return Ok(datagrams);
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        Ok(datagrams)
    }

    fn subscribe(
        &mut self,
        this: Resource<udp::IncomingDatagramStream>,
    ) -> wasmtime::Result<Resource<DynPollable>> {
        let this = rebind_incoming_borrow(&this);
        wasmtime_wasi_io::poll::subscribe(self.table, this)
    }

    fn drop(&mut self, this: Resource<udp::IncomingDatagramStream>) -> wasmtime::Result<()> {
        let this = rebind_incoming_own(this);
        // As in the filesystem implementation, we assume closing a socket
        // doesn't block.
        let dropped = self.table.delete(this)?;
        drop(dropped);

        Ok(())
    }
}

#[async_trait]
impl Pollable for IncomingDatagramStream {
    async fn ready(&mut self) {
        let stream = match self {
            IncomingDatagramStream::Network(stream) => stream,
            IncomingDatagramStream::Loopback(stream) => {
                let mut rx = stream.rx.lock().await;
                stream.received = rx.recv().await;
                return;
            }
            IncomingDatagramStream::Unspecified { net, lo } => {
                let mut lo_rx = lo.rx.lock().await;
                let mut net_ready = core::pin::pin!(async {
                    // FIXME: Add `Interest::ERROR` when we update to tokio 1.32.
                    _ = net.inner.ready(Interest::READABLE).await;
                });
                core::future::poll_fn(|cx| match lo_rx.poll_recv(cx) {
                    core::task::Poll::Ready(received) => {
                        lo.received = received;
                        core::task::Poll::Ready(())
                    }
                    core::task::Poll::Pending => net_ready.as_mut().poll(cx),
                })
                .await;
                return;
            }
        };
        _ = stream
            .inner
            .ready(Interest::READABLE.add(Interest::ERROR))
            .await;
    }
}

impl udp::HostOutgoingDatagramStream for WasiSocketsCtxView<'_> {
    fn check_send(&mut self, this: Resource<udp::OutgoingDatagramStream>) -> SocketResult<u64> {
        let this = rebind_outgoing_borrow(&this);
        let stream = self.table.get_mut(&this)?;
        let is_unspecified = matches!(stream, &mut OutgoingDatagramStream::Unspecified { .. });
        let stream = match stream {
            OutgoingDatagramStream::Network(stream) => stream,
            OutgoingDatagramStream::Loopback(lo) => return Ok(lo.check_send().into()),
            OutgoingDatagramStream::Unspecified { net, lo } => {
                if !lo.check_send() {
                    return Ok(0);
                }
                net
            }
        };

        let count: u64 =
            if std::pin::pin!(stream.inner.ready(Interest::WRITABLE.add(Interest::ERROR)))
                .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
                .is_ready()
            {
                16
            } else {
                0
            };

        stream.check_send_permit_count = count as usize;

        if count > 1 && is_unspecified {
            return Ok(1);
        }
        Ok(count)
    }

    async fn send(
        &mut self,
        this: Resource<udp::OutgoingDatagramStream>,
        datagrams: Vec<udp::OutgoingDatagram>,
    ) -> SocketResult<u64> {
        let this = rebind_outgoing_borrow(&this);

        async fn prepare_one(
            remote_address: Option<SocketAddr>,
            family: SocketAddressFamily,
            socket_addr_check: Option<&super::SocketAddrCheck>,
            datagram: &udp::OutgoingDatagram,
        ) -> SocketResult<SocketAddr> {
            if datagram.data.len() > MAX_UDP_DATAGRAM_SIZE {
                return Err(ErrorCode::DatagramTooLarge.into());
            }

            let provided_addr = datagram
                .remote_address
                .map(ip_socket_address_to_socket_addr);
            let addr = match (remote_address, provided_addr) {
                (None, Some(addr)) => {
                    let Some(check) = socket_addr_check else {
                        return Err(ErrorCode::InvalidState.into());
                    };
                    check
                        .check(addr, SocketAddrUse::UdpOutgoingDatagram)
                        .await
                        .map_err(super::network::socket_error_from_io)?;
                    addr
                }
                (Some(addr), None) => addr,
                (Some(connected_addr), Some(provided_addr)) if connected_addr == provided_addr => {
                    connected_addr
                }
                _ => return Err(ErrorCode::InvalidArgument.into()),
            };

            if !is_valid_remote_address(addr) || !is_valid_address_family(addr.ip(), family) {
                return Err(ErrorCode::InvalidArgument.into());
            }
            Ok(addr)
        }

        fn send_one_net(
            stream: &super::p2_udp::NetworkOutgoingDatagramStream,
            datagram: &udp::OutgoingDatagram,
            addr: SocketAddr,
        ) -> SocketResult<()> {
            if stream.remote_address == Some(addr) {
                stream
                    .inner
                    .try_send(&datagram.data)
                    .map_err(super::network::socket_error_from_io)?;
            } else {
                stream
                    .inner
                    .try_send_to(&datagram.data, addr)
                    .map_err(super::network::socket_error_from_io)?;
            }

            Ok(())
        }

        async fn send_one_lo(
            stream: &mut super::p2_udp::LoopbackOutgoingDatagramStream,
            datagram: udp::OutgoingDatagram,
            loopback: &std::sync::Mutex<super::loopback::Network>,
        ) -> SocketResult<()> {
            let addr = prepare_one(
                stream.remote_address,
                stream.family,
                stream.socket_addr_check.as_ref(),
                &datagram,
            )
            .await?;
            let Some(mut permit) = stream.permit.take() else {
                return Err(SocketError::trap(wasmtime::format_err!(
                    "unpermitted: must call check-send first"
                )));
            };
            if permit.num_permits() < datagram.data.len() {
                return Err(ErrorCode::DatagramTooLarge.into());
            }
            let required = core::num::NonZeroUsize::new(datagram.data.len())
                .unwrap_or(core::num::NonZeroUsize::MIN);
            let Some(unused) = permit.num_permits().checked_sub(required.into()) else {
                return Err(ErrorCode::DatagramTooLarge.into());
            };
            if unused > 0 {
                _ = permit.split(unused);
            }
            let mut loopback = loopback
                .lock()
                .map_err(|e| SocketError::trap(wasmtime::format_err!("{e}")))?;
            if let Some(tx) = loopback
                .connect_udp(&stream.local_address, &addr)
                .map_err(super::network::socket_error_from_util)?
            {
                _ = tx.send((
                    super::loopback::UdpDatagram {
                        source_address: stream.local_address,
                        data: datagram.data,
                    },
                    permit,
                ));
            }
            Ok(())
        }

        let stream = self.table.get_mut(&this)?;
        let (mut lo, stream) = match stream {
            OutgoingDatagramStream::Network(stream) => (None, stream),
            OutgoingDatagramStream::Loopback(stream) => {
                let mut datagrams = datagrams.into_iter();
                let datagram = match core::array::from_fn(|_| datagrams.next()) {
                    [None, None] => return Ok(0),
                    [Some(datagram), None] => datagram,
                    _ => {
                        return Err(SocketError::trap(wasmtime::format_err!(
                            "unpermitted: argument exceeds permitted size"
                        )));
                    }
                };
                send_one_lo(stream, datagram, &self.ctx.loopback).await?;
                return Ok(1);
            }
            OutgoingDatagramStream::Unspecified { lo, net } => {
                if datagrams.len() > 1 {
                    return Err(SocketError::trap(wasmtime::format_err!(
                        "unpermitted: argument exceeds permitted size"
                    )));
                }
                (Some(lo), net)
            }
        };

        if datagrams.is_empty() {
            return Ok(0);
        }

        if datagrams.len() > stream.check_send_permit_count {
            return Err(SocketError::trap(wasmtime::format_err!(
                "unpermitted: argument exceeds permitted size"
            )));
        }

        stream.check_send_permit_count -= datagrams.len();

        let mut count = 0;

        for datagram in datagrams {
            let addr = prepare_one(
                stream.remote_address,
                stream.family,
                stream.socket_addr_check.as_ref(),
                &datagram,
            )
            .await?;

            if addr.ip().to_canonical().is_loopback()
                && let Some(stream) = lo.as_mut()
            {
                send_one_lo(stream, datagram, &self.ctx.loopback).await?;
                count += 1;
                continue;
            }
            match send_one_net(stream, &datagram, addr) {
                Ok(_) => count += 1,
                Err(_) if count > 0 => {
                    // WIT: "If at least one datagram has been sent successfully, this function never returns an error."
                    return Ok(count);
                }
                Err(e) if matches!(e.downcast_ref(), Some(ErrorCode::WouldBlock)) => {
                    debug_assert!(count == 0);
                    return Ok(0);
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        Ok(count)
    }

    fn subscribe(
        &mut self,
        this: Resource<udp::OutgoingDatagramStream>,
    ) -> wasmtime::Result<Resource<DynPollable>> {
        let this = rebind_outgoing_borrow(&this);
        wasmtime_wasi_io::poll::subscribe(self.table, this)
    }

    fn drop(&mut self, this: Resource<udp::OutgoingDatagramStream>) -> wasmtime::Result<()> {
        let this = rebind_outgoing_own(this);
        // As in the filesystem implementation, we assume closing a socket
        // doesn't block.
        let dropped = self.table.delete(this)?;
        drop(dropped);

        Ok(())
    }
}

#[async_trait]
impl Pollable for OutgoingDatagramStream {
    async fn ready(&mut self) {
        let stream = match self {
            OutgoingDatagramStream::Network(stream) => stream,
            OutgoingDatagramStream::Loopback(stream) => {
                if stream.permit.is_none() {
                    _ = stream.permits.acquire().await;
                }
                return;
            }
            OutgoingDatagramStream::Unspecified { net, lo } => {
                if lo.permit.is_none() {
                    _ = lo.permits.acquire().await;
                }
                net
            }
        };
        _ = stream
            .inner
            .ready(Interest::WRITABLE.add(Interest::ERROR))
            .await;
    }
}

impl From<SocketAddressFamily> for IpAddressFamily {
    fn from(family: SocketAddressFamily) -> IpAddressFamily {
        match family {
            SocketAddressFamily::Ipv4 => IpAddressFamily::Ipv4,
            SocketAddressFamily::Ipv6 => IpAddressFamily::Ipv6,
        }
    }
}
