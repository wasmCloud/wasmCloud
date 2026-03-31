use super::{SocketAddrCheck, SocketAddressFamily};
use std::net::SocketAddr;
use std::sync::Arc;

pub struct NetworkIncomingDatagramStream {
    pub(crate) inner: Arc<tokio::net::UdpSocket>,

    /// If this has a value, the stream is "connected".
    pub(crate) remote_address: Option<SocketAddr>,
}

pub struct NetworkOutgoingDatagramStream {
    pub(crate) inner: Arc<tokio::net::UdpSocket>,

    /// If this has a value, the stream is "connected".
    pub(crate) remote_address: Option<SocketAddr>,

    /// Socket address family.
    pub(crate) family: SocketAddressFamily,

    /// The check of allowed addresses
    pub(crate) socket_addr_check: Option<SocketAddrCheck>,

    /// Remaining number of datagrams permitted by most recent `check-send` call.
    pub(crate) check_send_permit_count: usize,
}

pub struct LoopbackIncomingDatagramStream {
    pub remote_address: Option<SocketAddr>,
    pub rx: Arc<
        tokio::sync::Mutex<
            tokio::sync::mpsc::UnboundedReceiver<(
                super::loopback::UdpDatagram,
                tokio::sync::OwnedSemaphorePermit,
            )>,
        >,
    >,
    pub received: Option<(
        super::loopback::UdpDatagram,
        tokio::sync::OwnedSemaphorePermit,
    )>,
}

impl LoopbackIncomingDatagramStream {
    pub fn recv(
        &mut self,
        datagrams: &mut Vec<wasmtime_wasi::p2::bindings::sockets::udp::IncomingDatagram>,
        max_results: usize,
    ) -> Result<(), super::util::ErrorCode> {
        let Ok(mut rx) = self.rx.try_lock() else {
            return Err(super::util::ErrorCode::Unknown);
        };

        let mut rx = core::iter::chain(
            self.received.take(),
            core::iter::from_fn(|| rx.try_recv().ok()),
        );
        while datagrams.len() < max_results {
            let Some((dgram, _permit)) = rx.next() else {
                break;
            };
            match self.remote_address {
                Some(connected_addr) if connected_addr != dgram.source_address => continue,
                _ => datagrams.push(
                    wasmtime_wasi::p2::bindings::sockets::udp::IncomingDatagram {
                        data: dgram.data,
                        remote_address: super::host_network::socket_addr_to_ip_socket_address(
                            dgram.source_address,
                        ),
                    },
                ),
            }
        }
        Ok(())
    }
}

pub struct LoopbackOutgoingDatagramStream {
    pub local_address: SocketAddr,
    pub remote_address: Option<SocketAddr>,
    pub(crate) family: SocketAddressFamily,
    pub(crate) socket_addr_check: Option<SocketAddrCheck>,
    pub permits: Arc<tokio::sync::Semaphore>,
    pub(crate) permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

impl LoopbackOutgoingDatagramStream {
    pub fn check_send(&mut self) -> bool {
        if self.permit.is_some() {
            return true;
        };
        let Ok(p) =
            Arc::clone(&self.permits).try_acquire_many_owned(self.permits.available_permits() as _)
        else {
            return false;
        };
        self.permit = Some(p);
        true
    }
}

pub enum IncomingDatagramStream {
    Network(NetworkIncomingDatagramStream),
    Loopback(LoopbackIncomingDatagramStream),
    Unspecified {
        net: NetworkIncomingDatagramStream,
        lo: LoopbackIncomingDatagramStream,
    },
}

pub enum OutgoingDatagramStream {
    Network(NetworkOutgoingDatagramStream),
    Loopback(LoopbackOutgoingDatagramStream),
    Unspecified {
        net: NetworkOutgoingDatagramStream,
        lo: LoopbackOutgoingDatagramStream,
    },
}
