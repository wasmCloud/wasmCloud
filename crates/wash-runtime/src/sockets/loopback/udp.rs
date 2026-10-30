use crate::sockets::loopback::Network;
use crate::sockets::util::{ErrorCode, is_valid_address_family, is_valid_remote_address};
use crate::sockets::{SocketAddrCheck, SocketAddressFamily};
use core::mem;
use core::net::SocketAddr;
use core::num::NonZeroU16;
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use wasmtime::error::Context as _;

pub struct UdpEndpoint {
    pub(super) tx: mpsc::UnboundedSender<(UdpDatagram, OwnedSemaphorePermit)>,
    pub connected_address: Option<SocketAddr>,
    /// Bytes this endpoint will queue before dropping datagrams, sized by
    /// its socket's receive buffer.
    pub(super) receive_permits: Arc<Semaphore>,
}

pub struct UdpDatagram {
    pub source_address: SocketAddr,
    pub data: Vec<u8>,
    // P2 send capacity is returned when the queued datagram is consumed or
    // dropped. This is separate from the destination's receive capacity.
    _send_permit: Option<OwnedSemaphorePermit>,
}

impl UdpEndpoint {
    /// Queue a datagram if the receiver has space. UDP silently drops it on
    /// overflow, releasing any sender reservation immediately.
    pub fn send(
        &self,
        source_address: SocketAddr,
        data: Vec<u8>,
        send_permit: Option<OwnedSemaphorePermit>,
    ) {
        let Ok(required) = u32::try_from(data.len().max(1)) else {
            return;
        };
        let Ok(receive_permit) = Arc::clone(&self.receive_permits).try_acquire_many_owned(required)
        else {
            return;
        };
        _ = self.tx.send((
            UdpDatagram {
                source_address,
                data,
                _send_permit: send_permit,
            },
            receive_permit,
        ));
    }
}

pub enum UdpState {
    BindStarted {
        local_address: SocketAddr,
        rx: mpsc::UnboundedReceiver<(UdpDatagram, OwnedSemaphorePermit)>,
    },
    Bound {
        local_address: SocketAddr,
        rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<(UdpDatagram, OwnedSemaphorePermit)>>>,
        permits: Arc<Semaphore>,
    },
    Connected {
        local_address: SocketAddr,
        remote_address: SocketAddr,
        rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<(UdpDatagram, OwnedSemaphorePermit)>>>,
        permits: Arc<Semaphore>,
    },
    Closed,
}

pub struct UdpSocket {
    pub state: UdpState,
    pub hop_limit: u8,
    pub receive_buffer_size: u64,
    pub(crate) receive_permits: Arc<Semaphore>,
    pub(crate) connection_changed: Arc<tokio::sync::Notify>,
    pub send_buffer_size: u32,
    pub(crate) family: SocketAddressFamily,
    pub(crate) socket_addr_check: Option<SocketAddrCheck>,
}

/// Resize a buffer's semaphore from `current` to `value` permits. Permits held
/// by queued datagrams cannot be forgotten, so the returned size counts them
/// until they are released.
fn resize_permits(permits: &Semaphore, current: u64, value: u64) -> Option<u64> {
    let surplus = current.saturating_sub(value);
    if surplus > 0 {
        let reduced = permits.forget_permits(surplus.try_into().ok()?) as u64;
        value.checked_add(surplus - reduced)
    } else {
        permits.add_permits((value - current).try_into().ok()?);
        Some(value)
    }
}

impl UdpSocket {
    pub const MAX_SEND_BUFFER_SIZE: u32 = 0x1_0000;
    pub const MAX_RECEIVE_BUFFER_SIZE: u64 = 0x10_0000;

    pub fn p2_udp_streams(
        &self,
        remote_address: Option<SocketAddr>,
    ) -> Result<
        (
            crate::sockets::p2_udp::LoopbackIncomingDatagramStream,
            crate::sockets::p2_udp::LoopbackOutgoingDatagramStream,
        ),
        ErrorCode,
    > {
        let Self {
            state:
                UdpState::Bound {
                    local_address,
                    rx,
                    permits,
                }
                | UdpState::Connected {
                    local_address,
                    rx,
                    permits,
                    ..
                },
            ..
        } = self
        else {
            return Err(ErrorCode::InvalidState);
        };
        Ok((
            crate::sockets::p2_udp::LoopbackIncomingDatagramStream {
                remote_address,
                rx: Arc::clone(rx),
                received: None,
                socket_addr_check: self.socket_addr_check().cloned(),
            },
            crate::sockets::p2_udp::LoopbackOutgoingDatagramStream {
                local_address: *local_address,
                remote_address,
                permits: Arc::clone(permits),
                permit: None,
                family: self.address_family(),
                socket_addr_check: self.socket_addr_check().cloned(),
            },
        ))
    }

    pub fn finish_bind(&mut self) -> Result<(), ErrorCode> {
        match mem::replace(&mut self.state, UdpState::Closed) {
            UdpState::BindStarted { local_address, rx } => {
                let permits = Arc::new(Semaphore::new(self.send_buffer_size as _));
                let rx = Arc::new(tokio::sync::Mutex::new(rx));
                self.state = UdpState::Bound {
                    local_address,
                    rx,
                    permits,
                };
                Ok(())
            }
            state => {
                self.state = state;
                Err(ErrorCode::NotInProgress)
            }
        }
    }

    pub fn is_connected(&self) -> bool {
        matches!(self.state, UdpState::Connected { .. })
    }

    pub fn is_bound(&self) -> bool {
        matches!(
            self.state,
            UdpState::Connected { .. } | UdpState::Bound { .. }
        )
    }

    pub fn disconnect(&mut self, loopback: &mut Network) -> Result<(), ErrorCode> {
        match mem::replace(&mut self.state, UdpState::Closed) {
            UdpState::Connected {
                local_address,
                rx,
                permits,
                ..
            } => {
                let net = loopback.get_udp_net_mut(local_address.ip());
                let Some(port) = NonZeroU16::new(local_address.port()) else {
                    return Err(ErrorCode::InvalidState);
                };
                let Some(UdpEndpoint {
                    connected_address: connected_address @ Some(..),
                    ..
                }) = net.get_mut(&port)
                else {
                    return Err(ErrorCode::InvalidState);
                };
                *connected_address = None;

                self.state = UdpState::Bound {
                    local_address,
                    rx,
                    permits,
                };
                Ok(())
            }
            state => {
                self.state = state;
                Err(ErrorCode::InvalidState)
            }
        }
    }

    pub fn connect(&mut self, addr: SocketAddr, loopback: &mut Network) -> Result<(), ErrorCode> {
        if !is_valid_address_family(addr.ip(), self.family) || !is_valid_remote_address(addr) {
            return Err(ErrorCode::InvalidArgument);
        }

        match mem::replace(&mut self.state, UdpState::Closed) {
            UdpState::Bound {
                local_address,
                rx,
                permits,
            }
            | UdpState::Connected {
                local_address,
                rx,
                permits,
                ..
            } => {
                let net = loopback.get_udp_net_mut(local_address.ip());
                let Some(port) = NonZeroU16::new(local_address.port()) else {
                    return Err(ErrorCode::InvalidState);
                };
                let Some(UdpEndpoint {
                    connected_address, ..
                }) = net.get_mut(&port)
                else {
                    return Err(ErrorCode::InvalidState);
                };
                *connected_address = Some(addr);

                self.state = UdpState::Connected {
                    local_address,
                    remote_address: addr,
                    rx,
                    permits,
                };
                Ok(())
            }
            state => {
                self.state = state;
                Err(ErrorCode::InvalidState)
            }
        }
    }

    pub fn local_address(&self) -> Result<SocketAddr, ErrorCode> {
        match &self.state {
            UdpState::Bound { local_address, .. } | UdpState::Connected { local_address, .. } => {
                Ok(*local_address)
            }
            _ => Err(ErrorCode::InvalidState),
        }
    }

    pub fn remote_address(&self) -> Result<SocketAddr, ErrorCode> {
        match &self.state {
            UdpState::Connected { remote_address, .. } => Ok(*remote_address),
            _ => Err(ErrorCode::InvalidState),
        }
    }

    pub(crate) fn address_family(&self) -> SocketAddressFamily {
        self.family
    }

    pub fn unicast_hop_limit(&self) -> Result<u8, ErrorCode> {
        Ok(self.hop_limit)
    }

    pub fn set_unicast_hop_limit(&mut self, value: u8) -> Result<(), ErrorCode> {
        if value == 0 {
            return Err(ErrorCode::InvalidArgument);
        }
        self.hop_limit = value;
        Ok(())
    }

    pub fn receive_buffer_size(&self) -> Result<u64, ErrorCode> {
        Ok(self.receive_buffer_size)
    }

    pub fn set_receive_buffer_size(&mut self, value: u64) -> Result<(), ErrorCode> {
        if value == 0 {
            return Err(ErrorCode::InvalidArgument);
        }
        self.receive_buffer_size = resize_permits(
            &self.receive_permits,
            self.receive_buffer_size,
            value.min(Self::MAX_RECEIVE_BUFFER_SIZE),
        )
        .ok_or(ErrorCode::Unknown)?;
        Ok(())
    }

    pub fn send_buffer_size(&self) -> Result<u64, ErrorCode> {
        Ok(self.send_buffer_size.into())
    }

    pub fn set_send_buffer_size(&mut self, value: u64) -> Result<(), ErrorCode> {
        if value == 0 {
            return Err(ErrorCode::InvalidArgument);
        }
        let mut value = value
            .try_into()
            .unwrap_or(Self::MAX_SEND_BUFFER_SIZE)
            .min(Self::MAX_SEND_BUFFER_SIZE);
        if let UdpState::Bound { permits, .. } | UdpState::Connected { permits, .. } = &self.state {
            value = resize_permits(permits, self.send_buffer_size.into(), value.into())
                .and_then(|value| value.try_into().ok())
                .ok_or(ErrorCode::Unknown)?;
        }
        self.send_buffer_size = value;
        Ok(())
    }

    pub(crate) fn socket_addr_check(&self) -> Option<&SocketAddrCheck> {
        self.socket_addr_check.as_ref()
    }

    pub(crate) fn set_socket_addr_check(&mut self, check: Option<SocketAddrCheck>) {
        self.socket_addr_check = check;
    }

    pub fn drop(self, loopback: &mut Network) -> wasmtime::Result<()> {
        let local_address = match self.state {
            UdpState::BindStarted { local_address, .. }
            | UdpState::Bound { local_address, .. }
            | UdpState::Connected { local_address, .. } => local_address,
            UdpState::Closed => return Ok(()),
        };
        let net = loopback.get_udp_net_mut(local_address.ip());
        let port =
            NonZeroU16::new(local_address.port()).context("local address port cannot be 0")?;
        net.remove(&port);
        Ok(())
    }
}
