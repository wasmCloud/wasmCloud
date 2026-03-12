use crate::sockets::SocketAddressFamily;
use crate::sockets::loopback::Network;
use crate::sockets::tcp::ConnectingTcpStream;
use crate::sockets::util::ErrorCode;
use bytes::Bytes;
use core::mem;
use core::net::SocketAddr;
use core::num::NonZeroU16;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::collections::hash_map;
use std::sync::Arc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use wasmtime::error::Context as _;

type LoopbackChannel = (Bytes, OwnedSemaphorePermit);

/// wash-runtime always runs within a tokio context, so just call the closure.
fn with_ambient_tokio_runtime<R>(f: impl FnOnce() -> R) -> R {
    f()
}

#[derive(Debug)]
pub enum TcpEndpoint {
    Bound,
    Listening(mpsc::Sender<TcpConn>),
}

pub struct TcpConn {
    pub local_address: SocketAddr,
    pub remote_address: SocketAddr,
    pub rx: mpsc::UnboundedReceiver<(Bytes, OwnedSemaphorePermit)>,
    pub tx: mpsc::UnboundedSender<(Bytes, OwnedSemaphorePermit)>,
}

impl TcpConn {
    // Returns a tuple of local side of the connection and the remote side of the connection
    fn pair(local_address: SocketAddr, remote_address: SocketAddr) -> (Self, Self) {
        let (local_tx, remote_rx) = mpsc::unbounded_channel();
        let (remote_tx, local_rx) = mpsc::unbounded_channel();
        (
            Self {
                local_address,
                remote_address,
                rx: local_rx,
                tx: local_tx,
            },
            Self {
                local_address: remote_address,
                remote_address: local_address,
                rx: remote_rx,
                tx: remote_tx,
            },
        )
    }
}

pub enum TcpState {
    BindStarted(SocketAddr),
    Bound(SocketAddr),
    ListenStarted {
        local_address: SocketAddr,
        rx: Option<mpsc::Receiver<TcpConn>>,
    },
    Listening {
        local_address: SocketAddr,
        rx: mpsc::Receiver<TcpConn>,
        pending: Option<TcpConn>,
    },
    Connecting {
        local_address: SocketAddr,
        remote_address: SocketAddr,
        future: Option<Pin<Box<dyn Future<Output = std::io::Result<ConnectingTcpStream>> + Send>>>,
    },
    ConnectReady {
        local_address: SocketAddr,
        remote_address: SocketAddr,
        result: std::io::Result<ConnectingTcpStream>,
    },
    Connected {
        conn: TcpConn,
        accepted: bool,
    },
    P2Streaming {
        local_address: SocketAddr,
        remote_address: SocketAddr,
        accepted: bool,
        permits: Arc<Semaphore>,
        rx: Arc<tokio::sync::Mutex<Option<mpsc::UnboundedReceiver<LoopbackChannel>>>>,
        tx: Arc<std::sync::Mutex<Option<mpsc::UnboundedSender<LoopbackChannel>>>>,
    },
    Closed,
}

pub struct TcpSocket {
    pub state: TcpState,
    pub listen_backlog_size: u32,
    pub keep_alive_enabled: bool,
    pub keep_alive_idle_time: u64,
    pub keep_alive_interval: u64,
    pub keep_alive_count: u32,
    pub hop_limit: u8,
    pub receive_buffer_size: u64,
    pub send_buffer_size: u32,
    pub(crate) family: SocketAddressFamily,
}

impl TcpSocket {
    pub const MAX_SEND_BUFFER_SIZE: u32 = 0x10_0000;
    pub const MAX_LISTEN_BACKLOG_SIZE: u32 = 4096;

    pub fn finish_bind(&mut self) -> Result<(), ErrorCode> {
        match self.state {
            TcpState::BindStarted(addr) => {
                self.state = TcpState::Bound(addr);
                Ok(())
            }
            _ => Err(ErrorCode::NotInProgress),
        }
    }

    pub fn start_connect<'a>(
        &mut self,
        addr: &SocketAddr,
        loopback: &'a mut Network,
    ) -> Result<&'a mpsc::Sender<TcpConn>, ErrorCode> {
        let TcpState::Bound(local_address) = self.state else {
            return Err(ErrorCode::InvalidState);
        };
        let tx = loopback.connect_tcp(addr)?;
        self.state = TcpState::Connecting {
            local_address,
            remote_address: *addr,
            future: None,
        };
        Ok(tx)
    }

    pub fn set_pending_connect(
        &mut self,
        future: impl Future<Output = std::io::Result<ConnectingTcpStream>> + Send + 'static,
    ) -> Result<(), ErrorCode> {
        match &mut self.state {
            TcpState::Connecting {
                future: slot @ None,
                ..
            } => {
                *slot = Some(Box::pin(future));
                Ok(())
            }
            _ => Err(ErrorCode::InvalidState),
        }
    }

    pub fn take_pending_connect(
        &mut self,
    ) -> Result<Option<std::io::Result<ConnectingTcpStream>>, ErrorCode> {
        match mem::replace(&mut self.state, TcpState::Closed) {
            TcpState::ConnectReady {
                local_address,
                remote_address,
                result,
            } => {
                self.state = TcpState::Connecting {
                    local_address,
                    remote_address,
                    future: None,
                };
                Ok(Some(result))
            }
            TcpState::Connecting {
                local_address,
                remote_address,
                future: Some(mut future),
            } => {
                let mut cx = Context::from_waker(Waker::noop());
                match with_ambient_tokio_runtime(|| future.as_mut().poll(&mut cx)) {
                    Poll::Ready(result) => {
                        self.state = TcpState::Connecting {
                            local_address,
                            remote_address,
                            future: None,
                        };
                        Ok(Some(result))
                    }
                    Poll::Pending => {
                        self.state = TcpState::Connecting {
                            local_address,
                            remote_address,
                            future: Some(future),
                        };
                        Ok(None)
                    }
                }
            }
            state => {
                self.state = state;
                Err(ErrorCode::NotInProgress)
            }
        }
    }

    pub fn finish_connect(
        &mut self,
        result: std::io::Result<ConnectingTcpStream>,
        loopback: &mut Network,
    ) -> Result<(), ErrorCode> {
        let TcpState::Connecting {
            local_address,
            remote_address,
            future: None,
        } = self.state
        else {
            return Err(ErrorCode::InvalidState);
        };
        match result {
            Ok(ConnectingTcpStream::Network(..)) => Err(ErrorCode::InvalidState),
            Ok(ConnectingTcpStream::Loopback(tx)) => {
                let (clt, srv) = TcpConn::pair(local_address, remote_address);
                tx.send(srv);
                self.state = TcpState::Connected {
                    conn: clt,
                    accepted: false,
                };
                Ok(())
            }
            Err(err) => {
                self.state = TcpState::Closed;
                let net = loopback.get_tcp_net_mut(local_address.ip());
                let Some(port) = NonZeroU16::new(local_address.port()) else {
                    return Err(ErrorCode::InvalidState);
                };
                let Some(TcpEndpoint::Bound) = net.remove(&port) else {
                    return Err(ErrorCode::InvalidState);
                };
                Err(ErrorCode::from(err))
            }
        }
    }

    pub fn start_listen(&mut self, loopback: &mut Network) -> Result<(), ErrorCode> {
        let TcpState::Bound(addr) = self.state else {
            return Err(ErrorCode::InvalidState);
        };
        let net = loopback.get_tcp_net_mut(addr.ip());
        let Some(port) = NonZeroU16::new(addr.port()) else {
            return Err(ErrorCode::InvalidArgument);
        };
        let hash_map::Entry::Occupied(mut entry) = net.entry(port) else {
            return Err(ErrorCode::InvalidState);
        };
        let TcpEndpoint::Bound = entry.get() else {
            return Err(ErrorCode::InvalidState);
        };

        let cap = self.listen_backlog_size.min(Self::MAX_LISTEN_BACKLOG_SIZE);
        let cap = cap.try_into().unwrap_or(Semaphore::MAX_PERMITS);
        let (tx, rx) = mpsc::channel(cap);
        entry.insert(TcpEndpoint::Listening(tx));
        self.state = TcpState::ListenStarted {
            local_address: addr,
            rx: Some(rx),
        };
        Ok(())
    }

    pub fn finish_listen(&mut self) -> Result<(), ErrorCode> {
        let TcpState::ListenStarted {
            local_address,
            ref mut rx,
        } = self.state
        else {
            return Err(ErrorCode::NotInProgress);
        };
        let Some(rx) = rx.take() else {
            return Err(ErrorCode::InvalidState);
        };
        self.state = TcpState::Listening {
            local_address,
            rx,
            pending: None,
        };
        Ok(())
    }

    pub fn accept(&mut self) -> Result<Option<Self>, ErrorCode> {
        let TcpState::Listening {
            rx,
            pending,
            local_address,
        } = &mut self.state
        else {
            return Err(ErrorCode::InvalidState);
        };

        let conn = if let Some(conn) = pending.take() {
            conn
        } else {
            match rx.try_recv() {
                Ok(conn) => conn,
                Err(TryRecvError::Empty) => return Ok(None),
                Err(TryRecvError::Disconnected) => return Err(ErrorCode::ConnectionReset),
            }
        };
        if conn.local_address != *local_address {
            return Err(ErrorCode::Unknown);
        }
        Ok(Some(Self {
            state: TcpState::Connected {
                conn,
                accepted: true,
            },
            listen_backlog_size: self.listen_backlog_size,
            keep_alive_enabled: self.keep_alive_enabled,
            keep_alive_idle_time: self.keep_alive_idle_time,
            keep_alive_interval: self.keep_alive_interval,
            keep_alive_count: self.keep_alive_count,
            hop_limit: self.hop_limit,
            receive_buffer_size: self.receive_buffer_size,
            send_buffer_size: self.send_buffer_size,
            family: self.family,
        }))
    }

    pub fn local_address(&self) -> Result<SocketAddr, ErrorCode> {
        match &self.state {
            TcpState::Bound(local_address)
            | TcpState::Connected {
                conn: TcpConn { local_address, .. },
                ..
            }
            | TcpState::Listening { local_address, .. }
            | TcpState::P2Streaming { local_address, .. } => Ok(*local_address),
            _ => Err(ErrorCode::InvalidState),
        }
    }

    pub fn remote_address(&self) -> Result<SocketAddr, ErrorCode> {
        match &self.state {
            TcpState::Connected {
                conn: TcpConn { remote_address, .. },
                ..
            }
            | TcpState::P2Streaming { remote_address, .. } => Ok(*remote_address),
            _ => Err(ErrorCode::InvalidState),
        }
    }

    pub fn is_listening(&self) -> bool {
        matches!(self.state, TcpState::Listening { .. })
    }

    pub(crate) fn address_family(&self) -> SocketAddressFamily {
        self.family
    }

    pub fn set_listen_backlog_size(&mut self, value: u64) -> Result<(), ErrorCode> {
        let value = value.try_into().unwrap_or(u32::MAX);
        match &self.state {
            TcpState::Bound(..) => {
                self.listen_backlog_size = value;
                Ok(())
            }
            TcpState::Listening { .. } => Err(ErrorCode::NotSupported),
            _ => Err(ErrorCode::InvalidState),
        }
    }

    pub fn keep_alive_enabled(&self) -> Result<bool, ErrorCode> {
        Ok(self.keep_alive_enabled)
    }

    pub fn set_keep_alive_enabled(&mut self, value: bool) -> Result<(), ErrorCode> {
        self.keep_alive_enabled = value;
        Ok(())
    }

    pub fn keep_alive_idle_time(&self) -> Result<u64, ErrorCode> {
        Ok(self.keep_alive_idle_time)
    }

    pub fn set_keep_alive_idle_time(&mut self, value: u64) -> Result<(), ErrorCode> {
        if value == 0 {
            return Err(ErrorCode::InvalidArgument);
        }
        self.keep_alive_idle_time = value;
        Ok(())
    }

    pub fn keep_alive_interval(&self) -> Result<u64, ErrorCode> {
        Ok(self.keep_alive_interval)
    }

    pub fn set_keep_alive_interval(&mut self, value: u64) -> Result<(), ErrorCode> {
        if value == 0 {
            return Err(ErrorCode::InvalidArgument);
        }
        self.keep_alive_interval = value;
        Ok(())
    }

    pub fn keep_alive_count(&self) -> Result<u32, ErrorCode> {
        Ok(self.keep_alive_count)
    }

    pub fn set_keep_alive_count(&mut self, value: u32) -> Result<(), ErrorCode> {
        if value == 0 {
            return Err(ErrorCode::InvalidArgument);
        }
        self.keep_alive_count = value;
        Ok(())
    }

    pub fn hop_limit(&self) -> Result<u8, ErrorCode> {
        Ok(self.hop_limit)
    }

    pub fn set_hop_limit(&mut self, value: u8) -> Result<(), ErrorCode> {
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
        self.receive_buffer_size = value;
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
        if let TcpState::P2Streaming { permits, .. } = &self.state {
            let surplus = self.send_buffer_size.saturating_sub(value);
            if surplus > 0 {
                let reduced = permits.forget_permits(surplus as _) as _;
                let leaked = surplus.saturating_sub(reduced);
                value = value.checked_add(leaked).ok_or(ErrorCode::Unknown)?;
            } else {
                permits.add_permits(value.saturating_sub(self.send_buffer_size) as _);
            }
        }
        self.send_buffer_size = value;
        Ok(())
    }

    pub async fn ready(&mut self) {
        match &mut self.state {
            TcpState::BindStarted(..)
            | TcpState::Bound(..)
            | TcpState::ListenStarted { .. }
            | TcpState::Listening {
                pending: Some(..), ..
            }
            | TcpState::Connecting { future: None, .. }
            | TcpState::ConnectReady { .. }
            | TcpState::Connected { .. }
            | TcpState::P2Streaming { .. }
            | TcpState::Closed => {}
            TcpState::Connecting {
                local_address,
                remote_address,
                future: Some(future),
            } => {
                let result = future.as_mut().await;
                self.state = TcpState::ConnectReady {
                    local_address: *local_address,
                    remote_address: *remote_address,
                    result,
                };
            }
            TcpState::Listening {
                rx,
                pending: pending @ None,
                ..
            } => *pending = rx.recv().await,
        }
    }

    pub fn drop(self, loopback: &mut Network) -> wasmtime::Result<()> {
        let addr = match self.state {
            TcpState::BindStarted(local_address)
            | TcpState::Bound(local_address)
            | TcpState::ListenStarted { local_address, .. }
            | TcpState::Listening { local_address, .. }
            | TcpState::Connecting { local_address, .. }
            | TcpState::ConnectReady { local_address, .. }
            | TcpState::Connected {
                accepted: false,
                conn: TcpConn { local_address, .. },
                ..
            }
            | TcpState::P2Streaming {
                accepted: false,
                local_address,
                ..
            } => local_address,
            TcpState::Connected { accepted: true, .. }
            | TcpState::P2Streaming { accepted: true, .. }
            | TcpState::Closed => return Ok(()),
        };
        let net = loopback.get_tcp_net_mut(addr.ip());
        let port = NonZeroU16::new(addr.port()).context("local address port cannot be 0")?;
        net.remove(&port);
        Ok(())
    }
}
