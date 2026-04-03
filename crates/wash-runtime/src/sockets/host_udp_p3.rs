//! P3 UDP socket host trait implementations with loopback support.

use super::WasiSocketsCtxView;
use super::udp::UdpSocket;
use crate::sockets::{
    MAX_UDP_DATAGRAM_SIZE, SocketAddrUse, WasiSockets, p3_socket_error_from_util as se,
};
use std::net::SocketAddr;
use std::sync::Arc;
use wasmtime::component::{Accessor, Resource, ResourceTable};
use wasmtime::error::Context as _;

use wasmtime_wasi::p3::bindings::sockets::types::{
    ErrorCode, HostUdpSocket, HostUdpSocketWithStore, IpAddressFamily, IpSocketAddress,
};
use wasmtime_wasi::p3::sockets::{SocketError, SocketResult};

/// The upstream UDP socket resource type from the P3 bindings.
type UpstreamUdpSocket = wasmtime_wasi::sockets::UdpSocket;

fn get_socket<'a>(
    table: &'a ResourceTable,
    socket: &Resource<UpstreamUdpSocket>,
) -> SocketResult<&'a UdpSocket> {
    let socket = Resource::<UdpSocket>::new_borrow(socket.rep());
    table
        .get(&socket)
        .context("failed to get UDP socket resource from table")
        .map_err(SocketError::trap)
}

fn get_socket_mut<'a>(
    table: &'a mut ResourceTable,
    socket: &Resource<UpstreamUdpSocket>,
) -> SocketResult<&'a mut UdpSocket> {
    let socket = Resource::<UdpSocket>::new_borrow(socket.rep());
    table
        .get_mut(&socket)
        .context("failed to get mutable UDP socket resource from table")
        .map_err(SocketError::trap)
}

impl HostUdpSocketWithStore for WasiSockets {
    async fn send<T>(
        store: &Accessor<T, Self>,
        socket: Resource<UpstreamUdpSocket>,
        data: Vec<u8>,
        remote_address: Option<IpSocketAddress>,
    ) -> SocketResult<()> {
        if data.len() > MAX_UDP_DATAGRAM_SIZE {
            return Err(ErrorCode::DatagramTooLarge.into());
        }
        let remote_address = remote_address.map(SocketAddr::from);

        if let Some(addr) = remote_address {
            let check = store.with(|mut view| view.get().ctx.socket_addr_check.clone());
            if !check(addr, SocketAddrUse::UdpOutgoingDatagram).await {
                return Err(ErrorCode::AccessDenied.into());
            }
        }

        enum SendTarget {
            Network {
                socket: Arc<tokio::net::UdpSocket>,
                addr: Option<SocketAddr>,
                connected: bool,
            },
            Loopback {
                local_address: SocketAddr,
                addr: SocketAddr,
                loopback: Arc<std::sync::Mutex<super::loopback::Network>>,
            },
        }

        let target = store.with(|mut store| {
            let view = store.get();
            let socket = get_socket_mut(view.table, &socket)?;
            match socket {
                UdpSocket::Network(net) | UdpSocket::Unspecified { net, .. } => {
                    let sock = net.socket().clone();
                    let connected = net.is_connected();
                    SocketResult::Ok(SendTarget::Network {
                        socket: sock,
                        addr: remote_address,
                        connected,
                    })
                }
                UdpSocket::Loopback(lo) => {
                    let local_address = lo.local_address().map_err(se)?;
                    let addr = match (remote_address, lo.is_connected()) {
                        (Some(a), _) => a,
                        (None, true) => lo.remote_address().map_err(se)?,
                        (None, false) => return Err(se(super::util::ErrorCode::InvalidArgument)),
                    };
                    SocketResult::Ok(SendTarget::Loopback {
                        local_address,
                        addr,
                        loopback: Arc::clone(&view.ctx.loopback),
                    })
                }
            }
        })?;

        match target {
            SendTarget::Network {
                socket: udp_socket,
                addr,
                connected,
            } => match (connected, addr) {
                (_, Some(a)) => {
                    udp_socket
                        .send_to(&data, a)
                        .await
                        .map_err(|e| se(e.into()))?;
                }
                (true, None) => {
                    udp_socket.send(&data).await.map_err(|e| se(e.into()))?;
                }
                (false, None) => return Err(se(super::util::ErrorCode::InvalidArgument)),
            },
            SendTarget::Loopback {
                local_address,
                addr,
                loopback,
            } => {
                // Shared semaphore for UDP loopback permit requirements.
                // UDP doesn't need real flow control here — the permit is only
                // to satisfy the channel's type signature.
                static UDP_PERMITS: std::sync::LazyLock<Arc<tokio::sync::Semaphore>> =
                    std::sync::LazyLock::new(|| {
                        Arc::new(tokio::sync::Semaphore::new(u16::MAX as usize))
                    });

                let mut lo = loopback
                    .lock()
                    .map_err(|e| SocketError::trap(wasmtime::format_err!("{e}")))?;
                if let Some(tx) = lo.connect_udp(&local_address, &addr).map_err(se)? {
                    let permit = UDP_PERMITS
                        .clone()
                        .try_acquire_many_owned(data.len().max(1) as u32)
                        .map_err(|_| se(super::util::ErrorCode::OutOfMemory))?;
                    _ = tx.send((
                        super::loopback::UdpDatagram {
                            source_address: local_address,
                            data,
                        },
                        permit,
                    ));
                }
            }
        }
        Ok(())
    }

    async fn receive<T>(
        store: &Accessor<T, Self>,
        socket: Resource<UpstreamUdpSocket>,
    ) -> SocketResult<(Vec<u8>, IpSocketAddress)> {
        enum RecvSource {
            Network {
                socket: Arc<tokio::net::UdpSocket>,
                connected_addr: Option<SocketAddr>,
            },
            Loopback {
                rx: Arc<
                    tokio::sync::Mutex<
                        tokio::sync::mpsc::UnboundedReceiver<(
                            super::loopback::UdpDatagram,
                            tokio::sync::OwnedSemaphorePermit,
                        )>,
                    >,
                >,
            },
        }

        let source = store.with(|mut store| {
            let socket = get_socket(store.get().table, &socket)?;
            match socket {
                UdpSocket::Network(net) | UdpSocket::Unspecified { net, .. } => {
                    let sock = net.socket().clone();
                    let addr = if net.is_connected() {
                        Some(net.remote_address().map_err(se)?)
                    } else {
                        None
                    };
                    SocketResult::Ok(RecvSource::Network {
                        socket: sock,
                        connected_addr: addr,
                    })
                }
                UdpSocket::Loopback(lo) => match &lo.state {
                    super::loopback::UdpState::Bound { rx, .. }
                    | super::loopback::UdpState::Connected { rx, .. } => {
                        SocketResult::Ok(RecvSource::Loopback { rx: Arc::clone(rx) })
                    }
                    _ => Err(se(super::util::ErrorCode::InvalidState)),
                },
            }
        })?;

        let (data, addr) = match source {
            RecvSource::Network {
                socket: udp_socket,
                connected_addr,
            } => {
                let mut buf = vec![0u8; MAX_UDP_DATAGRAM_SIZE];
                match connected_addr {
                    Some(addr) => {
                        let n = udp_socket.recv(&mut buf).await.map_err(|e| se(e.into()))?;
                        buf.truncate(n);
                        (buf, addr)
                    }
                    None => {
                        let (n, addr) = udp_socket
                            .recv_from(&mut buf)
                            .await
                            .map_err(|e| se(e.into()))?;
                        buf.truncate(n);
                        (buf, addr)
                    }
                }
            }
            RecvSource::Loopback { rx } => {
                let mut guard = rx.lock().await;
                match guard.recv().await {
                    Some((datagram, _permit)) => (datagram.data, datagram.source_address),
                    None => return Err(se(super::util::ErrorCode::ConnectionReset)),
                }
            }
        };
        Ok((data, addr.into()))
    }
}

impl HostUdpSocket for WasiSocketsCtxView<'_> {
    async fn bind(
        &mut self,
        socket: Resource<UpstreamUdpSocket>,
        local_address: IpSocketAddress,
    ) -> SocketResult<()> {
        let local_address = SocketAddr::from(local_address);
        if !(self.ctx.socket_addr_check)(local_address, SocketAddrUse::UdpBind).await {
            return Err(ErrorCode::AccessDenied.into());
        }
        let socket_ref = get_socket_mut(self.table, &socket)?;
        let mut loopback = self
            .ctx
            .loopback
            .lock()
            .map_err(|e| SocketError::trap(wasmtime::format_err!("{e}")))?;
        socket_ref.bind(local_address, &mut loopback).map_err(se)?;
        socket_ref.finish_bind().map_err(se)?;
        Ok(())
    }

    async fn connect(
        &mut self,
        socket: Resource<UpstreamUdpSocket>,
        remote_address: IpSocketAddress,
    ) -> SocketResult<()> {
        let remote_address = SocketAddr::from(remote_address);
        if !(self.ctx.socket_addr_check)(remote_address, SocketAddrUse::UdpConnect).await {
            return Err(ErrorCode::AccessDenied.into());
        }
        let mut loopback = self
            .ctx
            .loopback
            .lock()
            .map_err(|e| SocketError::trap(wasmtime::format_err!("{e}")))?;
        let socket_ref = get_socket_mut(self.table, &socket)?;
        socket_ref
            .connect(remote_address, &mut loopback)
            .map_err(se)?;
        Ok(())
    }

    fn create(
        &mut self,
        address_family: IpAddressFamily,
    ) -> SocketResult<Resource<UpstreamUdpSocket>> {
        let family = match address_family {
            IpAddressFamily::Ipv4 => cap_net_ext::AddressFamily::Ipv4,
            IpAddressFamily::Ipv6 => cap_net_ext::AddressFamily::Ipv6,
        };
        let socket = UdpSocket::new(self.ctx, family).map_err(se)?;
        let resource = self
            .table
            .push(socket)
            .context("failed to push UDP socket resource to table")
            .map_err(SocketError::trap)?;
        Ok(Resource::new_own(resource.rep()))
    }

    fn disconnect(&mut self, socket: Resource<UpstreamUdpSocket>) -> SocketResult<()> {
        let socket_ref = get_socket_mut(self.table, &socket)?;
        let mut loopback = self
            .ctx
            .loopback
            .lock()
            .map_err(|e| SocketError::trap(wasmtime::format_err!("{e}")))?;
        socket_ref.disconnect(&mut loopback).map_err(se)?;
        Ok(())
    }

    fn get_local_address(
        &mut self,
        socket: Resource<UpstreamUdpSocket>,
    ) -> SocketResult<IpSocketAddress> {
        let sock = get_socket(self.table, &socket)?;
        Ok(sock.local_address().map_err(se)?.into())
    }

    fn get_remote_address(
        &mut self,
        socket: Resource<UpstreamUdpSocket>,
    ) -> SocketResult<IpSocketAddress> {
        let sock = get_socket(self.table, &socket)?;
        Ok(sock.remote_address().map_err(se)?.into())
    }

    fn get_address_family(
        &mut self,
        socket: Resource<UpstreamUdpSocket>,
    ) -> wasmtime::Result<IpAddressFamily> {
        let sock = get_socket(self.table, &socket)?;
        Ok(sock.address_family().into())
    }

    fn get_unicast_hop_limit(&mut self, socket: Resource<UpstreamUdpSocket>) -> SocketResult<u8> {
        let sock = get_socket(self.table, &socket)?;
        sock.unicast_hop_limit().map_err(se)
    }

    fn set_unicast_hop_limit(
        &mut self,
        socket: Resource<UpstreamUdpSocket>,
        value: u8,
    ) -> SocketResult<()> {
        let sock = get_socket_mut(self.table, &socket)?;
        sock.set_unicast_hop_limit(value).map_err(se)?;
        Ok(())
    }

    fn get_receive_buffer_size(
        &mut self,
        socket: Resource<UpstreamUdpSocket>,
    ) -> SocketResult<u64> {
        let sock = get_socket(self.table, &socket)?;
        sock.receive_buffer_size().map_err(se)
    }

    fn set_receive_buffer_size(
        &mut self,
        socket: Resource<UpstreamUdpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        let sock = get_socket_mut(self.table, &socket)?;
        sock.set_receive_buffer_size(value).map_err(se)?;
        Ok(())
    }

    fn get_send_buffer_size(&mut self, socket: Resource<UpstreamUdpSocket>) -> SocketResult<u64> {
        let sock = get_socket(self.table, &socket)?;
        sock.send_buffer_size().map_err(se)
    }

    fn set_send_buffer_size(
        &mut self,
        socket: Resource<UpstreamUdpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        let sock = get_socket_mut(self.table, &socket)?;
        sock.set_send_buffer_size(value).map_err(se)?;
        Ok(())
    }

    fn drop(&mut self, sock: Resource<UpstreamUdpSocket>) -> wasmtime::Result<()> {
        let sock = Resource::<UdpSocket>::new_own(sock.rep());
        let socket = self
            .table
            .delete(sock)
            .context("failed to delete UDP socket resource from table")?;
        let mut loopback = self
            .ctx
            .loopback
            .lock()
            .map_err(|e| wasmtime::format_err!("{e}"))?;
        socket.drop(&mut loopback)
    }
}
