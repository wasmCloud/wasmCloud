use super::p2_tcp::P2TcpStreamingState;
use super::util::{
    ErrorCode, get_unicast_hop_limit, is_valid_address_family, is_valid_remote_address,
    is_valid_unicast_address, receive_buffer_size, send_buffer_size, set_keep_alive_count,
    set_keep_alive_idle_time, set_keep_alive_interval, set_receive_buffer_size,
    set_send_buffer_size, set_unicast_hop_limit, tcp_bind,
};
use super::{DEFAULT_TCP_BACKLOG, SocketAddressFamily, WasiSocketsCtx};
use io_lifetimes::AsSocketlike as _;
use io_lifetimes::views::SocketlikeView;
use rustix::io::Errno;
use rustix::net::sockopt;
use std::fmt::Debug;
use std::io;
use std::mem;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

/// The state of a TCP socket.
///
/// This represents the various states a socket can be in during the
/// activities of binding, listening, accepting, and connecting. Note that this
/// state machine encompasses both WASIp2 and WASIp3.
enum TcpState {
    /// The initial state for a newly-created socket.
    ///
    /// From here a socket can transition to `BindStarted`, `ListenStarted`, or
    /// `Connecting`.
    Default(tokio::net::TcpSocket),

    /// A state indicating that a bind has been started and must be finished
    /// subsequently with `finish_bind`.
    ///
    /// From here a socket can transition to `Bound`.
    BindStarted(tokio::net::TcpSocket),

    /// Binding finished. The socket has an address but is not yet listening for
    /// connections.
    ///
    /// From here a socket can transition to `ListenStarted`, or `Connecting`.
    Bound(tokio::net::TcpSocket),

    /// Listening on a socket has started and must be completed with
    /// `finish_listen`.
    ///
    /// From here a socket can transition to `Listening`.
    ListenStarted(tokio::net::TcpSocket),

    /// The socket is now listening and waiting for an incoming connection.
    ///
    /// Sockets will not leave this state.
    Listening {
        /// The raw tokio-basd TCP listener managing the underyling socket.
        listener: Arc<tokio::net::TcpListener>,

        /// The last-accepted connection, set during the `ready` method and read
        /// during the `accept` method. Note that this is only used for WASIp2
        /// at this time.
        pending_accept: Option<io::Result<tokio::net::TcpStream>>,
    },

    /// An outgoing connection is started.
    ///
    /// This is created via the `start_connect` method. The payload here is an
    /// optionally-specified owned future for the result of the connect. In
    /// WASIp2 the future lives here, but in WASIp3 it lives on the event loop
    /// so this is `None`.
    ///
    /// From here a socket can transition to `ConnectReady` or `Connected`.
    Connecting(Option<Pin<Box<dyn Future<Output = io::Result<ConnectingTcpStream>> + Send>>>),

    /// A connection via `Connecting` has completed.
    ///
    /// This is present for WASIp2 where the `Connecting` state stores `Some` of
    /// a future, and the result of that future is recorded here when it
    /// finishes as part of the `ready` method.
    ///
    /// From here a socket can transition to `Connected`.
    ConnectReady(io::Result<ConnectingTcpStream>),

    /// A connection has been established.
    ///
    /// This is created either via `finish_connect` or for freshly accepted
    /// sockets from a TCP listener.
    ///
    /// From here a socket can transition to `Receiving` or `P2Streaming`.
    Connected(Arc<tokio::net::TcpStream>),

    /// This is a WASIp2-bound socket which stores some extra state for
    /// read/write streams to handle TCP shutdown.
    ///
    /// A socket will not transition out of this state.
    P2Streaming(Box<P2TcpStreamingState>),

    /// The socket is closed and no more operations can be performed.
    Closed,
}

impl Debug for TcpState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Default(_) => f.debug_tuple("Default").finish(),
            Self::BindStarted(_) => f.debug_tuple("BindStarted").finish(),
            Self::Bound(_) => f.debug_tuple("Bound").finish(),
            Self::ListenStarted { .. } => f.debug_tuple("ListenStarted").finish(),
            Self::Listening { .. } => f.debug_tuple("Listening").finish(),
            Self::Connecting(..) => f.debug_tuple("Connecting").finish(),
            Self::ConnectReady(..) => f.debug_tuple("ConnectReady").finish(),
            Self::Connected { .. } => f.debug_tuple("Connected").finish(),
            Self::P2Streaming(_) => f.debug_tuple("P2Streaming").finish(),
            Self::Closed => write!(f, "Closed"),
        }
    }
}

/// A host TCP socket, plus associated bookkeeping.
pub struct NetworkTcpSocket {
    /// The current state in the bind/listen/accept/connect progression.
    tcp_state: TcpState,

    /// The desired listen queue size.
    listen_backlog_size: u32,

    family: SocketAddressFamily,

    options: NonInheritedOptions,

    /// Tracks whether the send stream has been taken (P3 only).
    #[cfg(feature = "wasip3")]
    send_taken: bool,

    /// Tracks whether the receive stream has been taken (P3 only).
    #[cfg(feature = "wasip3")]
    receive_taken: bool,
}

impl NetworkTcpSocket {
    /// Create a new socket in the given family.
    fn new(ctx: &WasiSocketsCtx, family: SocketAddressFamily) -> Result<Self, ErrorCode> {
        ctx.allowed_network_uses.check_allowed_tcp()?;

        {
            let socket = match family {
                SocketAddressFamily::Ipv4 => tokio::net::TcpSocket::new_v4()?,
                SocketAddressFamily::Ipv6 => {
                    let socket = tokio::net::TcpSocket::new_v6()?;
                    sockopt::set_ipv6_v6only(&socket, true)?;
                    socket
                }
            };

            Ok(Self::from_state(TcpState::Default(socket), family))
        }
    }

    /// Creates a new socket with the `result` of an accepted socket from a
    /// `TcpListener`.
    ///
    /// This will handle the `result` internally and `result` should be the raw
    /// result from a TCP listen operation.
    fn new_accept(
        result: io::Result<tokio::net::TcpStream>,
        options: &NonInheritedOptions,
        family: SocketAddressFamily,
    ) -> io::Result<Self> {
        let client = result.map_err(|err| match Errno::from_io_error(&err) {
            // From: https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-accept#:~:text=WSAEINPROGRESS
            // > WSAEINPROGRESS: A blocking Windows Sockets 1.1 call is in progress,
            // > or the service provider is still processing a callback function.
            //
            // wasi-sockets doesn't have an equivalent to the EINPROGRESS error,
            // because in POSIX this error is only returned by a non-blocking
            // `connect` and wasi-sockets has a different solution for that.
            #[cfg(windows)]
            Some(Errno::INPROGRESS) => Errno::INTR.into(),

            // Normalize Linux' non-standard behavior.
            //
            // From https://man7.org/linux/man-pages/man2/accept.2.html:
            // > Linux accept() passes already-pending network errors on the
            // > new socket as an error code from accept(). This behavior
            // > differs from other BSD socket implementations. (...)
            #[cfg(target_os = "linux")]
            Some(
                Errno::CONNRESET
                | Errno::NETRESET
                | Errno::HOSTUNREACH
                | Errno::HOSTDOWN
                | Errno::NETDOWN
                | Errno::NETUNREACH
                | Errno::PROTO
                | Errno::NOPROTOOPT
                | Errno::NONET
                | Errno::OPNOTSUPP,
            ) => Errno::CONNABORTED.into(),

            _ => err,
        })?;
        options.apply(family, &client);
        Ok(Self {
            tcp_state: TcpState::Connected(Arc::new(client)),
            listen_backlog_size: DEFAULT_TCP_BACKLOG,
            family,
            options: Default::default(),
            #[cfg(feature = "wasip3")]
            send_taken: false,
            #[cfg(feature = "wasip3")]
            receive_taken: false,
        })
    }

    /// Create an error socket (P3 only, for deferred accept errors).
    #[cfg(feature = "wasip3")]
    pub(crate) fn new_error(_err: std::io::Error, family: SocketAddressFamily) -> Self {
        // Create a closed socket to represent the error.
        // The upstream uses a dedicated Error state, but for simplicity
        // we just create a Closed socket since errors are deferred.
        Self::from_state(TcpState::Closed, family)
    }

    /// Create a `TcpSocket` from an existing socket.
    fn from_state(state: TcpState, family: SocketAddressFamily) -> Self {
        Self {
            tcp_state: state,
            listen_backlog_size: DEFAULT_TCP_BACKLOG,
            family,
            options: Default::default(),
            #[cfg(feature = "wasip3")]
            send_taken: false,
            #[cfg(feature = "wasip3")]
            receive_taken: false,
        }
    }

    fn as_std_view(&self) -> Result<SocketlikeView<'_, std::net::TcpStream>, ErrorCode> {
        match &self.tcp_state {
            TcpState::Default(socket)
            | TcpState::BindStarted(socket)
            | TcpState::Bound(socket)
            | TcpState::ListenStarted(socket) => Ok(socket.as_socketlike_view()),
            TcpState::Connected(stream) => Ok(stream.as_socketlike_view()),
            TcpState::Listening { listener, .. } => Ok(listener.as_socketlike_view()),
            TcpState::P2Streaming(state) => Ok(state.stream.as_socketlike_view()),
            TcpState::Connecting(..) | TcpState::ConnectReady(_) | TcpState::Closed => {
                Err(ErrorCode::InvalidState)
            }
        }
    }

    pub(crate) fn start_bind(&mut self, addr: SocketAddr) -> Result<(), ErrorCode> {
        match mem::replace(&mut self.tcp_state, TcpState::Closed) {
            TcpState::Default(sock) => {
                if let Err(err) = tcp_bind(&sock, addr) {
                    self.tcp_state = TcpState::Default(sock);
                    Err(err)
                } else {
                    self.tcp_state = TcpState::BindStarted(sock);
                    Ok(())
                }
            }
            tcp_state => {
                self.tcp_state = tcp_state;
                Err(ErrorCode::InvalidState)
            }
        }
    }

    pub(crate) fn finish_bind(&mut self) -> Result<(), ErrorCode> {
        match mem::replace(&mut self.tcp_state, TcpState::Closed) {
            TcpState::BindStarted(socket) => {
                self.tcp_state = TcpState::Bound(socket);
                Ok(())
            }
            current_state => {
                // Reset the state so that the outside world doesn't see this socket as closed
                self.tcp_state = current_state;
                Err(ErrorCode::NotInProgress)
            }
        }
    }

    fn start_connect(&mut self) -> Result<tokio::net::TcpSocket, ErrorCode> {
        let (TcpState::Default(tokio_socket) | TcpState::Bound(tokio_socket)) =
            mem::replace(&mut self.tcp_state, TcpState::Connecting(None))
        else {
            unreachable!();
        };

        Ok(tokio_socket)
    }

    /// For WASIp2 this is used to record the actual connection future as part
    /// of `start_connect` within this socket state.
    fn set_pending_connect(
        &mut self,
        future: impl Future<Output = io::Result<ConnectingTcpStream>> + Send + 'static,
    ) -> Result<(), ErrorCode> {
        match &mut self.tcp_state {
            TcpState::Connecting(slot @ None) => {
                *slot = Some(Box::pin(future));
                Ok(())
            }
            _ => Err(ErrorCode::InvalidState),
        }
    }

    /// For WASIp2 this retrieves the result from the future passed to
    /// `set_pending_connect`.
    ///
    /// Return states here are:
    ///
    /// * `Ok(Some(res))` - where `res` is the result of the connect operation.
    /// * `Ok(None)` - the connect operation isn't ready yet.
    /// * `Err(e)` - a connect operation is not in progress.
    fn take_pending_connect(
        &mut self,
    ) -> Result<Option<io::Result<ConnectingTcpStream>>, ErrorCode> {
        match mem::replace(&mut self.tcp_state, TcpState::Connecting(None)) {
            TcpState::ConnectReady(result) => Ok(Some(result)),
            TcpState::Connecting(Some(mut future)) => {
                let mut cx = Context::from_waker(Waker::noop());
                match future.as_mut().poll(&mut cx) {
                    Poll::Ready(result) => Ok(Some(result)),
                    Poll::Pending => {
                        self.tcp_state = TcpState::Connecting(Some(future));
                        Ok(None)
                    }
                }
            }
            current_state => {
                self.tcp_state = current_state;
                Err(ErrorCode::NotInProgress)
            }
        }
    }

    fn finish_connect(&mut self, result: io::Result<ConnectingTcpStream>) -> Result<(), ErrorCode> {
        if !matches!(self.tcp_state, TcpState::Connecting(None)) {
            return Err(ErrorCode::InvalidState);
        }
        match result {
            Ok(ConnectingTcpStream::Network(stream)) => {
                self.tcp_state = TcpState::Connected(Arc::new(stream));
                Ok(())
            }
            Ok(ConnectingTcpStream::Loopback(..)) => Err(ErrorCode::InvalidState),
            Err(err) => {
                self.tcp_state = TcpState::Closed;
                Err(ErrorCode::from(err))
            }
        }
    }

    pub(crate) fn start_listen(&mut self) -> Result<(), ErrorCode> {
        match mem::replace(&mut self.tcp_state, TcpState::Closed) {
            TcpState::Bound(tokio_socket) => {
                self.tcp_state = TcpState::ListenStarted(tokio_socket);
                Ok(())
            }
            previous_state => {
                self.tcp_state = previous_state;
                Err(ErrorCode::InvalidState)
            }
        }
    }

    pub(crate) fn finish_listen(&mut self) -> Result<(), ErrorCode> {
        let tokio_socket = match mem::replace(&mut self.tcp_state, TcpState::Closed) {
            TcpState::ListenStarted(tokio_socket) => tokio_socket,
            previous_state => {
                self.tcp_state = previous_state;
                return Err(ErrorCode::NotInProgress);
            }
        };

        match tokio_socket.listen(self.listen_backlog_size) {
            Ok(listener) => {
                self.tcp_state = TcpState::Listening {
                    listener: Arc::new(listener),
                    pending_accept: None,
                };
                Ok(())
            }
            Err(err) => {
                self.tcp_state = TcpState::Closed;

                Err(match Errno::from_io_error(&err) {
                    // See: https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-listen#:~:text=WSAEMFILE
                    // According to the docs, `listen` can return EMFILE on Windows.
                    // This is odd, because we're not trying to create a new socket
                    // or file descriptor of any kind. So we rewrite it to less
                    // surprising error code.
                    //
                    // At the time of writing, this behavior has never been experimentally
                    // observed by any of the wasmtime authors, so we're relying fully
                    // on Microsoft's documentation here.
                    #[cfg(windows)]
                    Some(Errno::MFILE) => Errno::NOBUFS.into(),

                    _ => err.into(),
                })
            }
        }
    }

    fn accept(&mut self) -> Result<Option<Self>, ErrorCode> {
        let TcpState::Listening {
            listener,
            pending_accept,
        } = &mut self.tcp_state
        else {
            return Err(ErrorCode::InvalidState);
        };

        let result = match pending_accept.take() {
            Some(result) => result,
            None => {
                let mut cx = std::task::Context::from_waker(Waker::noop());
                match listener.poll_accept(&mut cx).map_ok(|(stream, _)| stream) {
                    Poll::Ready(result) => result,
                    Poll::Pending => return Ok(None),
                }
            }
        };

        Ok(Some(Self::new_accept(result, &self.options, self.family)?))
    }

    fn local_address(&self) -> Result<SocketAddr, ErrorCode> {
        match &self.tcp_state {
            TcpState::Bound(socket) => Ok(socket.local_addr()?),
            TcpState::Connected(stream) => Ok(stream.local_addr()?),
            TcpState::P2Streaming(state) => Ok(state.stream.local_addr()?),
            TcpState::Listening { listener, .. } => Ok(listener.local_addr()?),
            _ => Err(ErrorCode::InvalidState),
        }
    }

    fn remote_address(&self) -> Result<SocketAddr, ErrorCode> {
        let stream = self.tcp_stream_arc()?;
        let addr = stream.peer_addr()?;
        Ok(addr)
    }

    fn is_listening(&self) -> bool {
        matches!(self.tcp_state, TcpState::Listening { .. })
    }

    pub(crate) fn address_family(&self) -> SocketAddressFamily {
        self.family
    }

    fn set_listen_backlog_size(&mut self, value: u64) -> Result<(), ErrorCode> {
        const MIN_BACKLOG: u32 = 1;
        const MAX_BACKLOG: u32 = i32::MAX as u32; // OS'es will most likely limit it down even further.

        if value == 0 {
            return Err(ErrorCode::InvalidArgument);
        }
        // Silently clamp backlog size. This is OK for us to do, because operating systems do this too.
        let value = value
            .try_into()
            .unwrap_or(MAX_BACKLOG)
            .clamp(MIN_BACKLOG, MAX_BACKLOG);
        match &self.tcp_state {
            TcpState::Default(..) | TcpState::Bound(..) => {
                // Socket not listening yet. Stash value for first invocation to `listen`.
                self.listen_backlog_size = value;
                Ok(())
            }
            TcpState::Listening { listener, .. } => {
                // Try to update the backlog by calling `listen` again.
                // Not all platforms support this. We'll only update our own value if the OS supports changing the backlog size after the fact.
                if rustix::net::listen(listener, value.try_into().unwrap_or(i32::MAX)).is_err() {
                    return Err(ErrorCode::NotSupported);
                }
                self.listen_backlog_size = value;
                Ok(())
            }
            _ => Err(ErrorCode::InvalidState),
        }
    }

    fn keep_alive_enabled(&self) -> Result<bool, ErrorCode> {
        let fd = &*self.as_std_view()?;
        let v = sockopt::socket_keepalive(fd)?;
        Ok(v)
    }

    fn set_keep_alive_enabled(&self, value: bool) -> Result<(), ErrorCode> {
        let fd = &*self.as_std_view()?;
        sockopt::set_socket_keepalive(fd, value)?;
        Ok(())
    }

    fn keep_alive_idle_time(&self) -> Result<u64, ErrorCode> {
        let fd = &*self.as_std_view()?;
        let v = sockopt::tcp_keepidle(fd)?;
        Ok(v.as_nanos().try_into().unwrap_or(u64::MAX))
    }

    fn set_keep_alive_idle_time(&mut self, value: u64) -> Result<(), ErrorCode> {
        let value = {
            let fd = self.as_std_view()?;
            set_keep_alive_idle_time(&*fd, value)?
        };
        self.options.set_keep_alive_idle_time(value);
        Ok(())
    }

    fn keep_alive_interval(&self) -> Result<u64, ErrorCode> {
        let fd = &*self.as_std_view()?;
        let v = sockopt::tcp_keepintvl(fd)?;
        Ok(v.as_nanos().try_into().unwrap_or(u64::MAX))
    }

    fn set_keep_alive_interval(&self, value: u64) -> Result<(), ErrorCode> {
        let fd = &*self.as_std_view()?;
        set_keep_alive_interval(fd, Duration::from_nanos(value))?;
        Ok(())
    }

    fn keep_alive_count(&self) -> Result<u32, ErrorCode> {
        let fd = &*self.as_std_view()?;
        let v = sockopt::tcp_keepcnt(fd)?;
        Ok(v)
    }

    fn set_keep_alive_count(&self, value: u32) -> Result<(), ErrorCode> {
        let fd = &*self.as_std_view()?;
        set_keep_alive_count(fd, value)?;
        Ok(())
    }

    fn hop_limit(&self) -> Result<u8, ErrorCode> {
        let fd = &*self.as_std_view()?;
        let n = get_unicast_hop_limit(fd, self.family)?;
        Ok(n)
    }

    fn set_hop_limit(&mut self, value: u8) -> Result<(), ErrorCode> {
        {
            let fd = &*self.as_std_view()?;
            set_unicast_hop_limit(fd, self.family, value)?;
        }
        self.options.set_hop_limit(value);
        Ok(())
    }

    fn receive_buffer_size(&self) -> Result<u64, ErrorCode> {
        let fd = &*self.as_std_view()?;
        let n = receive_buffer_size(fd)?;
        Ok(n)
    }

    fn set_receive_buffer_size(&mut self, value: u64) -> Result<(), ErrorCode> {
        let res = {
            let fd = &*self.as_std_view()?;
            set_receive_buffer_size(fd, value)?
        };
        self.options.set_receive_buffer_size(res);
        Ok(())
    }

    fn send_buffer_size(&self) -> Result<u64, ErrorCode> {
        let fd = &*self.as_std_view()?;
        let n = send_buffer_size(fd)?;
        Ok(n)
    }

    fn set_send_buffer_size(&mut self, value: u64) -> Result<(), ErrorCode> {
        let res = {
            let fd = &*self.as_std_view()?;
            set_send_buffer_size(fd, value)?
        };
        self.options.set_send_buffer_size(res);
        Ok(())
    }

    /// Start listening using P3 semantics (with implicit bind).
    #[cfg(feature = "wasip3")]
    pub(crate) fn listen_p3(&mut self) -> Result<(), ErrorCode> {
        let tokio_socket = match mem::replace(&mut self.tcp_state, TcpState::Closed) {
            TcpState::Bound(tokio_socket) => tokio_socket,
            TcpState::Default(tokio_socket) => {
                // Implicit bind to an ephemeral port
                let implicit_addr = crate::sockets::util::implicit_bind_addr(self.family);
                tcp_bind(&tokio_socket, implicit_addr)?;
                tokio_socket
            }
            previous_state => {
                self.tcp_state = previous_state;
                return Err(ErrorCode::InvalidState);
            }
        };

        match tokio_socket.listen(self.listen_backlog_size) {
            Ok(listener) => {
                self.tcp_state = TcpState::Listening {
                    listener: Arc::new(listener),
                    pending_accept: None,
                };
                Ok(())
            }
            Err(err) => {
                self.tcp_state = TcpState::Closed;
                Err(err.into())
            }
        }
    }

    /// Get the TCP listener Arc (P3 only, for creating accept streams).
    #[cfg(feature = "wasip3")]
    pub(crate) fn tcp_listener_arc(&self) -> Result<&Arc<tokio::net::TcpListener>, ErrorCode> {
        match &self.tcp_state {
            TcpState::Listening { listener, .. } => Ok(listener),
            _ => Err(ErrorCode::InvalidState),
        }
    }

    /// Take the send stream Arc (P3 only).
    #[cfg(feature = "wasip3")]
    pub(crate) fn take_send_stream(&mut self) -> Result<Arc<tokio::net::TcpStream>, ErrorCode> {
        if self.send_taken {
            return Err(ErrorCode::InvalidState);
        }
        match &self.tcp_state {
            TcpState::Connected(stream) => {
                self.send_taken = true;
                Ok(stream.clone())
            }
            _ => Err(ErrorCode::InvalidState),
        }
    }

    /// Take the receive stream Arc (P3 only).
    #[cfg(feature = "wasip3")]
    pub(crate) fn take_receive_stream(&mut self) -> Result<Arc<tokio::net::TcpStream>, ErrorCode> {
        if self.receive_taken {
            return Err(ErrorCode::InvalidState);
        }
        match &self.tcp_state {
            TcpState::Connected(stream) => {
                self.receive_taken = true;
                Ok(stream.clone())
            }
            _ => Err(ErrorCode::InvalidState),
        }
    }

    /// Get non-inherited options reference (for accepted sockets).
    #[cfg(feature = "wasip3")]
    pub(crate) fn non_inherited_options(&self) -> &NonInheritedOptions {
        &self.options
    }

    pub(crate) fn tcp_stream_arc(&self) -> Result<&Arc<tokio::net::TcpStream>, ErrorCode> {
        match &self.tcp_state {
            TcpState::Connected(socket) => Ok(socket),
            TcpState::P2Streaming(state) => Ok(&state.stream),
            _ => Err(ErrorCode::InvalidState),
        }
    }

    pub(crate) fn p2_streaming_state(&self) -> Result<&P2TcpStreamingState, ErrorCode> {
        match &self.tcp_state {
            TcpState::P2Streaming(state) => Ok(state),
            _ => Err(ErrorCode::InvalidState),
        }
    }

    pub(crate) fn set_p2_streaming_state(
        &mut self,
        state: P2TcpStreamingState,
    ) -> Result<(), ErrorCode> {
        if !matches!(self.tcp_state, TcpState::Connected(_)) {
            return Err(ErrorCode::InvalidState);
        }
        self.tcp_state = TcpState::P2Streaming(Box::new(state));
        Ok(())
    }

    /// Used for `Pollable` in the WASIp2 implementation this awaits the socket
    /// to be connected, if in the connecting state, or for a TCP accept to be
    /// ready, if this is in the listening state.
    ///
    /// For all other states this method immediately returns.
    async fn ready(&mut self) {
        match &mut self.tcp_state {
            TcpState::Default(..)
            | TcpState::BindStarted(..)
            | TcpState::Bound(..)
            | TcpState::ListenStarted(..)
            | TcpState::ConnectReady(..)
            | TcpState::Closed
            | TcpState::Connected { .. }
            | TcpState::Connecting(None)
            | TcpState::Listening {
                pending_accept: Some(_),
                ..
            }
            | TcpState::P2Streaming(_) => {}

            TcpState::Connecting(Some(future)) => {
                self.tcp_state = TcpState::ConnectReady(future.as_mut().await);
            }

            TcpState::Listening {
                listener,
                pending_accept: slot @ None,
            } => {
                let result = futures::future::poll_fn(|cx| {
                    listener.poll_accept(cx).map_ok(|(stream, _)| stream)
                })
                .await;
                *slot = Some(result);
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub use inherits_option::*;
#[cfg(not(target_os = "macos"))]
mod inherits_option {
    use super::super::SocketAddressFamily;
    use tokio::net::TcpStream;

    #[derive(Default, Clone)]
    pub struct NonInheritedOptions;

    impl NonInheritedOptions {
        pub fn set_keep_alive_idle_time(&mut self, _value: u64) {}

        pub fn set_hop_limit(&mut self, _value: u8) {}

        pub fn set_receive_buffer_size(&mut self, _value: usize) {}

        pub fn set_send_buffer_size(&mut self, _value: usize) {}

        pub(crate) fn apply(&self, _family: SocketAddressFamily, _stream: &TcpStream) {}
    }
}

#[cfg(target_os = "macos")]
pub use does_not_inherit_options::*;
#[cfg(target_os = "macos")]
mod does_not_inherit_options {
    use super::super::SocketAddressFamily;
    use rustix::net::sockopt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering::Relaxed};
    use std::time::Duration;
    use tokio::net::TcpStream;

    // The socket options below are not automatically inherited from the listener
    // on all platforms. So we keep track of which options have been explicitly
    // set and manually apply those values to newly accepted clients.
    #[derive(Default, Clone)]
    pub struct NonInheritedOptions(Arc<Inner>);

    #[derive(Default)]
    struct Inner {
        receive_buffer_size: AtomicUsize,
        send_buffer_size: AtomicUsize,
        hop_limit: AtomicU8,
        keep_alive_idle_time: AtomicU64, // nanoseconds
    }

    impl NonInheritedOptions {
        pub fn set_keep_alive_idle_time(&mut self, value: u64) {
            self.0.keep_alive_idle_time.store(value, Relaxed);
        }

        pub fn set_hop_limit(&mut self, value: u8) {
            self.0.hop_limit.store(value, Relaxed);
        }

        pub fn set_receive_buffer_size(&mut self, value: usize) {
            self.0.receive_buffer_size.store(value, Relaxed);
        }

        pub fn set_send_buffer_size(&mut self, value: usize) {
            self.0.send_buffer_size.store(value, Relaxed);
        }

        pub(crate) fn apply(&self, family: SocketAddressFamily, stream: &TcpStream) {
            // Manually inherit socket options from listener. We only have to
            // do this on platforms that don't already do this automatically
            // and only if a specific value was explicitly set on the listener.

            let receive_buffer_size = self.0.receive_buffer_size.load(Relaxed);
            if receive_buffer_size > 0 {
                // Ignore potential error.
                _ = sockopt::set_socket_recv_buffer_size(stream, receive_buffer_size);
            }

            let send_buffer_size = self.0.send_buffer_size.load(Relaxed);
            if send_buffer_size > 0 {
                // Ignore potential error.
                _ = sockopt::set_socket_send_buffer_size(stream, send_buffer_size);
            }

            // For some reason, IP_TTL is inherited, but IPV6_UNICAST_HOPS isn't.
            if family == SocketAddressFamily::Ipv6 {
                let hop_limit = self.0.hop_limit.load(Relaxed);
                if hop_limit > 0 {
                    // Ignore potential error.
                    _ = sockopt::set_ipv6_unicast_hops(stream, Some(hop_limit));
                }
            }

            let keep_alive_idle_time = self.0.keep_alive_idle_time.load(Relaxed);
            if keep_alive_idle_time > 0 {
                // Ignore potential error.
                _ = sockopt::set_tcp_keepidle(stream, Duration::from_nanos(keep_alive_idle_time));
            }
        }
    }
}

impl super::loopback::TcpSocket {
    pub fn new(
        socket: &NetworkTcpSocket,
        state: super::loopback::TcpState,
    ) -> Result<Self, ErrorCode> {
        let fd = &*socket.as_std_view()?;

        let keep_alive_enabled = sockopt::socket_keepalive(fd)?;

        let keep_alive_idle_time = sockopt::tcp_keepidle(fd)?;
        let keep_alive_idle_time = keep_alive_idle_time
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX);

        let keep_alive_interval = sockopt::tcp_keepintvl(fd)?;
        let keep_alive_interval = keep_alive_interval
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX);

        let keep_alive_count = sockopt::tcp_keepcnt(fd)?;

        let hop_limit = get_unicast_hop_limit(fd, socket.family)?;

        let receive_buffer_size = receive_buffer_size(fd)?;

        let send_buffer_size = send_buffer_size(fd)?;
        let send_buffer_size = send_buffer_size
            .try_into()
            .unwrap_or(Self::MAX_SEND_BUFFER_SIZE)
            .min(Self::MAX_SEND_BUFFER_SIZE);

        let listen_backlog_size = socket
            .listen_backlog_size
            .min(Self::MAX_LISTEN_BACKLOG_SIZE);
        Ok(Self {
            state,
            send_buffer_size,
            receive_buffer_size,
            listen_backlog_size,
            keep_alive_enabled,
            keep_alive_idle_time,
            keep_alive_interval,
            keep_alive_count,
            hop_limit,
            family: socket.family,
        })
    }
}

pub enum TcpSocket {
    Network(NetworkTcpSocket),
    Loopback(super::loopback::TcpSocket),
    // A socket bound to unspecified IP, which was not connected yet
    Unspecified {
        net: NetworkTcpSocket,
        lo: super::loopback::TcpSocket,
    },
}

pub enum ConnectingTcpSocket {
    Network(tokio::net::TcpSocket),
    Loopback(tokio::sync::mpsc::Sender<super::loopback::TcpConn>),
}

pub enum ConnectingTcpStream {
    Network(tokio::net::TcpStream),
    Loopback(tokio::sync::mpsc::OwnedPermit<super::loopback::TcpConn>),
}

impl ConnectingTcpSocket {
    pub async fn connect(self, addr: SocketAddr) -> io::Result<ConnectingTcpStream> {
        match self {
            Self::Network(socket) => socket.connect(addr).await.map(ConnectingTcpStream::Network),
            Self::Loopback(tx) => match tx.reserve_owned().await {
                Ok(tx) => Ok(ConnectingTcpStream::Loopback(tx)),
                Err(..) => Err(std::io::ErrorKind::ConnectionRefused.into()),
            },
        }
    }
}

impl TcpSocket {
    pub(crate) fn new(
        ctx: &WasiSocketsCtx,
        family: SocketAddressFamily,
    ) -> Result<Self, ErrorCode> {
        NetworkTcpSocket::new(ctx, family).map(Self::Network)
    }

    #[allow(dead_code)]
    pub(crate) fn new_accept(
        result: io::Result<tokio::net::TcpStream>,
        options: &NonInheritedOptions,
        family: SocketAddressFamily,
    ) -> io::Result<Self> {
        NetworkTcpSocket::new_accept(result, options, family).map(Self::Network)
    }

    pub(crate) fn start_bind(
        &mut self,
        mut addr: SocketAddr,
        loopback: &mut super::loopback::Network,
    ) -> Result<(), ErrorCode> {
        use core::net::{Ipv4Addr, Ipv6Addr};

        let Self::Network(socket) = self else {
            return Err(ErrorCode::InvalidState);
        };
        let ip = addr.ip();
        if !is_valid_unicast_address(ip) || !is_valid_address_family(ip, socket.family) {
            return Err(ErrorCode::InvalidArgument);
        }
        let ip = ip.to_canonical();
        if !ip.is_loopback() {
            if ip.is_unspecified() {
                // Rewrite 0.0.0.0/[::] to loopback so the OS socket only listens on loopback
                match &mut addr {
                    SocketAddr::V4(addr) => addr.set_ip(Ipv4Addr::LOCALHOST),
                    SocketAddr::V6(addr) => addr.set_ip(Ipv6Addr::LOCALHOST),
                }
            }
            socket.start_bind(addr)?;
            if !ip.is_unspecified() {
                return Ok(());
            }
            let TcpState::BindStarted(sock) = &socket.tcp_state else {
                unreachable!();
            };
            addr = sock.local_addr()?;
            match &mut addr {
                SocketAddr::V4(addr) => addr.set_ip(Ipv4Addr::LOCALHOST),
                SocketAddr::V6(addr) => addr.set_ip(Ipv6Addr::LOCALHOST),
            }
        }
        let addr = loopback.bind_tcp(addr)?;
        let lo =
            super::loopback::TcpSocket::new(socket, super::loopback::TcpState::BindStarted(addr))?;
        if ip.is_unspecified() {
            *self = Self::Unspecified {
                net: NetworkTcpSocket {
                    tcp_state: mem::replace(&mut socket.tcp_state, TcpState::Closed),
                    listen_backlog_size: socket.listen_backlog_size,
                    family: socket.family,
                    options: socket.options.clone(),
                    #[cfg(feature = "wasip3")]
                    send_taken: false,
                    #[cfg(feature = "wasip3")]
                    receive_taken: false,
                },
                lo,
            }
        } else {
            *self = Self::Loopback(lo);
        }
        Ok(())
    }

    pub(crate) fn finish_bind(&mut self) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.finish_bind(),
            Self::Loopback(socket) => socket.finish_bind(),
            Self::Unspecified { net, lo } => {
                net.finish_bind()?;
                lo.finish_bind()
            }
        }
    }

    pub(crate) fn start_connect(
        &mut self,
        addr: &SocketAddr,
        loopback: &mut super::loopback::Network,
    ) -> Result<ConnectingTcpSocket, ErrorCode> {
        if let Self::Network(socket) | Self::Unspecified { net: socket, .. } = self {
            match socket.tcp_state {
                TcpState::Default(..) | TcpState::Bound(..) => {}
                TcpState::Connecting(..) => {
                    return Err(ErrorCode::ConcurrencyConflict);
                }
                _ => return Err(ErrorCode::InvalidState),
            };

            if !is_valid_unicast_address(addr.ip())
                || !is_valid_remote_address(*addr)
                || !is_valid_address_family(addr.ip(), socket.family)
            {
                return Err(ErrorCode::InvalidArgument);
            };
        }
        if let Self::Loopback(socket) | Self::Unspecified { lo: socket, .. } = self {
            match socket.state {
                super::loopback::TcpState::Bound(..) => {}
                super::loopback::TcpState::Connecting { .. } => {
                    return Err(ErrorCode::ConcurrencyConflict);
                }
                _ => return Err(ErrorCode::InvalidState),
            };

            if !is_valid_unicast_address(addr.ip())
                || !is_valid_remote_address(*addr)
                || !is_valid_address_family(addr.ip(), socket.family)
            {
                return Err(ErrorCode::InvalidArgument);
            };
        }

        let ip = addr.ip().to_canonical();
        match (
            mem::replace(
                self,
                Self::Loopback(super::loopback::TcpSocket {
                    state: super::loopback::TcpState::Closed,
                    listen_backlog_size: 0,
                    keep_alive_enabled: false,
                    keep_alive_idle_time: 0,
                    keep_alive_interval: 0,
                    keep_alive_count: 0,
                    hop_limit: 0,
                    receive_buffer_size: 0,
                    send_buffer_size: 0,
                    family: SocketAddressFamily::Ipv4,
                }),
            ),
            ip.is_loopback(),
        ) {
            (
                Self::Network(mut socket)
                | Self::Unspecified {
                    net: mut socket, ..
                },
                false,
            ) => {
                let res = socket.start_connect().map(ConnectingTcpSocket::Network);
                *self = Self::Network(socket);
                res
            }
            (Self::Network(socket), true) => {
                if let TcpState::Bound(..) = socket.tcp_state {
                    *self = Self::Network(socket);
                    // socket wasn't bound to loopback
                    return Err(ErrorCode::InvalidState);
                }

                let mut local_address = *addr;
                local_address.set_port(0);
                let local_address = match loopback.bind_tcp(local_address) {
                    Ok(addr) => addr,
                    Err(err) => {
                        *self = Self::Network(socket);
                        return Err(err);
                    }
                };

                let tx = match loopback.connect_tcp(addr) {
                    Ok(tx) => tx,
                    Err(err) => {
                        *self = Self::Network(socket);
                        return Err(err);
                    }
                };

                match super::loopback::TcpSocket::new(
                    &socket,
                    super::loopback::TcpState::Connecting {
                        local_address,
                        remote_address: *addr,
                        future: None,
                    },
                ) {
                    Ok(socket) => {
                        *self = Self::Loopback(socket);
                        Ok(ConnectingTcpSocket::Loopback(tx.clone()))
                    }
                    Err(err) => {
                        *self = Self::Network(socket);
                        Err(err)
                    }
                }
            }
            (Self::Loopback(mut socket), ..) => {
                let tx = socket.start_connect(addr, loopback);
                *self = Self::Loopback(socket);
                tx.map(|tx| ConnectingTcpSocket::Loopback(tx.clone()))
            }
            (Self::Unspecified { mut lo, net }, true) => match lo.start_connect(addr, loopback) {
                Ok(tx) => {
                    *self = Self::Loopback(lo);
                    Ok(ConnectingTcpSocket::Loopback(tx.clone()))
                }
                Err(err) => {
                    *self = Self::Unspecified { lo, net };
                    Err(err)
                }
            },
        }
    }

    pub(crate) fn set_pending_connect(
        &mut self,
        future: impl Future<Output = io::Result<ConnectingTcpStream>> + Send + 'static,
    ) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.set_pending_connect(future),
            Self::Loopback(socket) => socket.set_pending_connect(future),
            Self::Unspecified { .. } => Err(ErrorCode::InvalidState),
        }
    }

    pub(crate) fn take_pending_connect(
        &mut self,
    ) -> Result<Option<io::Result<ConnectingTcpStream>>, ErrorCode> {
        match self {
            Self::Network(socket) => socket.take_pending_connect(),
            Self::Loopback(socket) => socket.take_pending_connect(),
            Self::Unspecified { .. } => Err(ErrorCode::InvalidState),
        }
    }

    pub(crate) fn finish_connect(
        &mut self,
        result: io::Result<ConnectingTcpStream>,
        loopback: &mut super::loopback::Network,
    ) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.finish_connect(result),
            Self::Loopback(socket) => socket.finish_connect(result, loopback),
            Self::Unspecified { .. } => Err(ErrorCode::InvalidState),
        }
    }

    pub(crate) fn start_listen(
        &mut self,
        loopback: &mut super::loopback::Network,
    ) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.start_listen(),
            Self::Loopback(socket) => socket.start_listen(loopback),
            Self::Unspecified { net, lo } => {
                net.start_listen()?;
                lo.start_listen(loopback)
            }
        }
    }

    pub(crate) fn finish_listen(&mut self) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.finish_listen(),
            Self::Loopback(socket) => socket.finish_listen(),
            Self::Unspecified { net, lo } => {
                net.finish_listen()?;
                lo.finish_listen()
            }
        }
    }

    pub(crate) fn accept(&mut self) -> Result<Option<Self>, ErrorCode> {
        match self {
            Self::Network(socket) => socket.accept().map(|sock| sock.map(Self::Network)),
            Self::Loopback(socket) => socket.accept().map(|sock| sock.map(Self::Loopback)),
            Self::Unspecified { net, lo } => {
                if let Some(sock) = net.accept()? {
                    return Ok(Some(Self::Network(sock)));
                }
                lo.accept().map(|sock| sock.map(Self::Loopback))
            }
        }
    }

    pub(crate) fn local_address(&self) -> Result<SocketAddr, ErrorCode> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => socket.local_address(),
            Self::Loopback(socket) => socket.local_address(),
        }
    }

    pub(crate) fn remote_address(&self) -> Result<SocketAddr, ErrorCode> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.remote_address()
            }
            Self::Loopback(socket) => socket.remote_address(),
        }
    }

    pub(crate) fn is_listening(&self) -> bool {
        match self {
            Self::Network(socket) => socket.is_listening(),
            Self::Loopback(socket) => socket.is_listening(),
            Self::Unspecified { net, lo } => net.is_listening() && lo.is_listening(),
        }
    }

    pub(crate) fn address_family(&self) -> SocketAddressFamily {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.address_family()
            }
            Self::Loopback(socket) => socket.address_family(),
        }
    }

    pub(crate) fn set_listen_backlog_size(&mut self, value: u64) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.set_listen_backlog_size(value),
            Self::Loopback(socket) => socket.set_listen_backlog_size(value),
            Self::Unspecified { net, lo } => {
                net.set_listen_backlog_size(value)?;
                lo.set_listen_backlog_size(value)
            }
        }
    }

    pub(crate) fn keep_alive_enabled(&self) -> Result<bool, ErrorCode> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.keep_alive_enabled()
            }
            Self::Loopback(socket) => socket.keep_alive_enabled(),
        }
    }

    pub(crate) fn set_keep_alive_enabled(&mut self, value: bool) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.set_keep_alive_enabled(value),
            Self::Loopback(socket) => socket.set_keep_alive_enabled(value),
            Self::Unspecified { net, lo } => {
                net.set_keep_alive_enabled(value)?;
                lo.set_keep_alive_enabled(value)
            }
        }
    }

    pub(crate) fn keep_alive_idle_time(&self) -> Result<u64, ErrorCode> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.keep_alive_idle_time()
            }
            Self::Loopback(socket) => socket.keep_alive_idle_time(),
        }
    }

    pub(crate) fn set_keep_alive_idle_time(&mut self, value: u64) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.set_keep_alive_idle_time(value),
            Self::Loopback(socket) => socket.set_keep_alive_idle_time(value),
            Self::Unspecified { net, lo } => {
                net.set_keep_alive_idle_time(value)?;
                lo.set_keep_alive_idle_time(value)
            }
        }
    }

    pub(crate) fn keep_alive_interval(&self) -> Result<u64, ErrorCode> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.keep_alive_interval()
            }
            Self::Loopback(socket) => socket.keep_alive_interval(),
        }
    }

    pub(crate) fn set_keep_alive_interval(&mut self, value: u64) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.set_keep_alive_interval(value),
            Self::Loopback(socket) => socket.set_keep_alive_interval(value),
            Self::Unspecified { net, lo } => {
                net.set_keep_alive_interval(value)?;
                lo.set_keep_alive_interval(value)
            }
        }
    }

    pub(crate) fn keep_alive_count(&self) -> Result<u32, ErrorCode> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.keep_alive_count()
            }
            Self::Loopback(socket) => socket.keep_alive_count(),
        }
    }

    pub(crate) fn set_keep_alive_count(&mut self, value: u32) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.set_keep_alive_count(value),
            Self::Loopback(socket) => socket.set_keep_alive_count(value),
            Self::Unspecified { net, lo } => {
                net.set_keep_alive_count(value)?;
                lo.set_keep_alive_count(value)
            }
        }
    }

    pub(crate) fn hop_limit(&self) -> Result<u8, ErrorCode> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => socket.hop_limit(),
            Self::Loopback(socket) => socket.hop_limit(),
        }
    }

    pub(crate) fn set_hop_limit(&mut self, value: u8) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.set_hop_limit(value),
            Self::Loopback(socket) => socket.set_hop_limit(value),
            Self::Unspecified { net, lo } => {
                net.set_hop_limit(value)?;
                lo.set_hop_limit(value)
            }
        }
    }

    pub(crate) fn receive_buffer_size(&self) -> Result<u64, ErrorCode> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.receive_buffer_size()
            }
            Self::Loopback(socket) => socket.receive_buffer_size(),
        }
    }

    pub(crate) fn set_receive_buffer_size(&mut self, value: u64) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.set_receive_buffer_size(value),
            Self::Loopback(socket) => socket.set_receive_buffer_size(value),
            Self::Unspecified { net, lo } => {
                net.set_receive_buffer_size(value)?;
                lo.set_receive_buffer_size(value)
            }
        }
    }

    pub(crate) fn send_buffer_size(&self) -> Result<u64, ErrorCode> {
        match self {
            Self::Network(socket) | Self::Unspecified { net: socket, .. } => {
                socket.send_buffer_size()
            }
            Self::Loopback(socket) => socket.send_buffer_size(),
        }
    }

    pub(crate) fn set_send_buffer_size(&mut self, value: u64) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.set_send_buffer_size(value),
            Self::Loopback(socket) => socket.set_send_buffer_size(value),
            Self::Unspecified { net, lo } => {
                net.set_send_buffer_size(value)?;
                lo.set_send_buffer_size(value)
            }
        }
    }

    /// Take the loopback accept channel for P3 listen streaming.
    /// Returns the receiver that yields incoming `TcpConn` connections.
    #[cfg(feature = "wasip3")]
    pub(crate) fn take_loopback_listen_rx(&mut self) -> Result<P3LoopbackListenInfo, ErrorCode> {
        let lo = match self {
            Self::Loopback(socket) => socket,
            Self::Unspecified { lo, .. } => lo,
            _ => return Err(ErrorCode::InvalidState),
        };
        take_loopback_listen_info(lo)
    }

    /// Listen using P3 semantics (with implicit bind). For loopback, requires
    /// the loopback network to register the listener.
    #[cfg(feature = "wasip3")]
    pub(crate) fn listen_p3(
        &mut self,
        loopback: &mut super::loopback::Network,
    ) -> Result<(), ErrorCode> {
        match self {
            Self::Network(socket) => socket.listen_p3(),
            Self::Loopback(socket) => {
                socket.start_listen(loopback)?;
                socket.finish_listen()
            }
            Self::Unspecified { net, lo } => {
                net.listen_p3()?;
                lo.start_listen(loopback)?;
                lo.finish_listen()
            }
        }
    }

    /// Take the send stream (P3 only). Returns the underlying stream or
    /// channel for sending data.
    #[cfg(feature = "wasip3")]
    pub(crate) fn take_send_stream(&mut self) -> Result<P3SendStream, ErrorCode> {
        match self {
            Self::Network(socket) => socket.take_send_stream().map(P3SendStream::Network),
            Self::Loopback(socket) => {
                if let super::loopback::TcpState::Connected { ref conn, .. } = socket.state {
                    let permits = Arc::new(tokio::sync::Semaphore::new(
                        socket.send_buffer_size as usize,
                    ));
                    Ok(P3SendStream::Loopback {
                        tx: conn.tx.clone(),
                        permits,
                    })
                } else {
                    Err(ErrorCode::InvalidState)
                }
            }
            Self::Unspecified { .. } => {
                // After connect, Unspecified becomes Network or Loopback
                Err(ErrorCode::InvalidState)
            }
        }
    }

    /// Take the receive stream (P3 only). Returns the underlying stream or
    /// channel for receiving data.
    #[cfg(feature = "wasip3")]
    pub(crate) fn take_receive_stream(&mut self) -> Result<P3ReceiveStream, ErrorCode> {
        match self {
            Self::Network(socket) => socket.take_receive_stream().map(P3ReceiveStream::Network),
            Self::Loopback(socket) => {
                if let super::loopback::TcpState::Connected { ref mut conn, .. } = socket.state {
                    // Swap out the receiver, replacing with a closed one
                    let (_, dummy_rx) = tokio::sync::mpsc::unbounded_channel();
                    let rx = mem::replace(&mut conn.rx, dummy_rx);
                    Ok(P3ReceiveStream::Loopback(rx))
                } else {
                    Err(ErrorCode::InvalidState)
                }
            }
            Self::Unspecified { .. } => Err(ErrorCode::InvalidState),
        }
    }

    pub(crate) async fn ready(&mut self) {
        match self {
            Self::Network(socket) => socket.ready().await,
            Self::Loopback(socket) => socket.ready().await,
            Self::Unspecified { net, lo } => {
                use core::future::poll_fn;
                use core::pin::pin;
                use core::task::Poll;

                let mut net = pin!(net.ready());
                let mut lo = pin!(lo.ready());
                poll_fn(|cx| match net.as_mut().poll(cx) {
                    Poll::Ready(()) => Poll::Ready(()),
                    Poll::Pending => lo.as_mut().poll(cx),
                })
                .await;
            }
        }
    }

    pub(crate) fn drop(self, loopback: &mut super::loopback::Network) -> wasmtime::Result<()> {
        match self {
            Self::Network(socket) => {
                drop(socket);
                Ok(())
            }
            Self::Loopback(socket) => socket.drop(loopback),
            Self::Unspecified { net, lo } => {
                drop(net);
                lo.drop(loopback)
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::sockets::WasiSocketsCtx;

    fn make_ipv4_socket() -> NetworkTcpSocket {
        let ctx = WasiSocketsCtx::default();
        NetworkTcpSocket::new(&ctx, SocketAddressFamily::Ipv4).unwrap()
    }

    fn bind_socket(socket: &mut NetworkTcpSocket) {
        let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        socket.start_bind(addr).unwrap();
        socket.finish_bind().unwrap();
    }

    // --- State transition tests ---

    #[tokio::test]
    async fn test_new_socket_default_state() {
        let socket = make_ipv4_socket();
        assert!(!socket.is_listening());
        assert!(matches!(socket.address_family(), SocketAddressFamily::Ipv4));
    }

    #[tokio::test]
    async fn test_start_bind_and_finish_bind() {
        let mut socket = make_ipv4_socket();
        let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();

        socket.start_bind(addr).unwrap();
        // BindStarted — not yet bound, local_address not available
        assert!(!socket.is_listening());

        socket.finish_bind().unwrap();
        // Now Bound
        assert!(!socket.is_listening());
    }

    #[tokio::test]
    async fn test_finish_bind_without_start_errors() {
        let mut socket = make_ipv4_socket();
        let result = socket.finish_bind();
        assert!(matches!(result, Err(ErrorCode::NotInProgress)));
    }

    #[tokio::test]
    async fn test_start_listen_from_bound() {
        let mut socket = make_ipv4_socket();
        bind_socket(&mut socket);

        let result = socket.start_listen();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_start_listen_from_default_errors() {
        let mut socket = make_ipv4_socket();
        let result = socket.start_listen();
        assert!(matches!(result, Err(ErrorCode::InvalidState)));
    }

    #[tokio::test]
    async fn test_finish_listen_from_listen_started() {
        let mut socket = make_ipv4_socket();
        bind_socket(&mut socket);
        socket.start_listen().unwrap();

        let result = socket.finish_listen();
        assert!(result.is_ok());
        assert!(socket.is_listening());
    }

    #[tokio::test]
    async fn test_finish_listen_without_start_errors() {
        let mut socket = make_ipv4_socket();
        bind_socket(&mut socket);

        let result = socket.finish_listen();
        assert!(matches!(result, Err(ErrorCode::NotInProgress)));
    }

    // --- Address tests ---

    #[tokio::test]
    async fn test_local_address_after_bind() {
        let mut socket = make_ipv4_socket();
        bind_socket(&mut socket);

        let addr = socket.local_address().unwrap();
        assert_ne!(addr.port(), 0);
        assert!(addr.ip().is_loopback());
    }

    #[tokio::test]
    async fn test_local_address_before_bind_errors() {
        let socket = make_ipv4_socket();
        let result = socket.local_address();
        assert!(matches!(result, Err(ErrorCode::InvalidState)));
    }

    #[tokio::test]
    async fn test_address_family() {
        let socket = make_ipv4_socket();
        assert!(matches!(socket.address_family(), SocketAddressFamily::Ipv4));
    }

    // --- Socket option tests ---

    #[tokio::test]
    async fn test_keep_alive_roundtrip() {
        let socket = make_ipv4_socket();
        socket.set_keep_alive_enabled(true).unwrap();
        assert!(socket.keep_alive_enabled().unwrap());

        socket.set_keep_alive_enabled(false).unwrap();
        assert!(!socket.keep_alive_enabled().unwrap());
    }

    #[tokio::test]
    async fn test_hop_limit_roundtrip() {
        let mut socket = make_ipv4_socket();
        socket.set_hop_limit(64).unwrap();
        assert_eq!(socket.hop_limit().unwrap(), 64);
    }

    #[tokio::test]
    async fn test_hop_limit_zero_errors() {
        let mut socket = make_ipv4_socket();
        let result = socket.set_hop_limit(0);
        assert!(matches!(result, Err(ErrorCode::InvalidArgument)));
    }

    #[tokio::test]
    async fn test_listen_backlog_size() {
        let mut socket = make_ipv4_socket();
        let result = socket.set_listen_backlog_size(64);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_listen_backlog_size_zero_errors() {
        let mut socket = make_ipv4_socket();
        let result = socket.set_listen_backlog_size(0);
        assert!(matches!(result, Err(ErrorCode::InvalidArgument)));
    }

    #[tokio::test]
    async fn test_receive_buffer_size_roundtrip() {
        let mut socket = make_ipv4_socket();
        socket.set_receive_buffer_size(65536).unwrap();
        // OS may clamp, just verify we get a non-zero value back
        let size = socket.receive_buffer_size().unwrap();
        assert!(size > 0);
    }

    #[tokio::test]
    async fn test_send_buffer_size_roundtrip() {
        let mut socket = make_ipv4_socket();
        socket.set_send_buffer_size(65536).unwrap();
        let size = socket.send_buffer_size().unwrap();
        assert!(size > 0);
    }

    #[cfg(feature = "wasip3")]
    mod p3 {
        use super::*;
        use crate::sockets::loopback;

        #[tokio::test]
        async fn test_listen_p3_from_bound() {
            let mut socket = make_ipv4_socket();
            bind_socket(&mut socket);
            let result = socket.listen_p3();
            assert!(result.is_ok(), "listen_p3 from Bound should succeed");
            assert!(socket.is_listening());
        }

        #[tokio::test]
        async fn test_listen_p3_implicit_bind() {
            let mut socket = make_ipv4_socket();
            let result = socket.listen_p3();
            assert!(
                result.is_ok(),
                "listen_p3 from Default should succeed with implicit bind"
            );
            assert!(socket.is_listening());
        }

        #[tokio::test]
        async fn test_listen_p3_from_listening_errors() {
            let mut socket = make_ipv4_socket();
            bind_socket(&mut socket);
            socket.listen_p3().unwrap();
            let result = socket.listen_p3();
            assert!(
                matches!(result, Err(ErrorCode::InvalidState)),
                "listen_p3 from Listening should error"
            );
        }

        #[tokio::test]
        async fn test_tcp_listener_arc_after_listen_p3() {
            let mut socket = make_ipv4_socket();
            bind_socket(&mut socket);
            socket.listen_p3().unwrap();
            let listener = socket.tcp_listener_arc();
            assert!(listener.is_ok(), "should get listener Arc after listen_p3");
        }

        #[tokio::test]
        async fn test_tcp_listener_arc_before_listen_errors() {
            let socket = make_ipv4_socket();
            let result = socket.tcp_listener_arc();
            assert!(
                matches!(result, Err(ErrorCode::InvalidState)),
                "tcp_listener_arc before listen should error"
            );
        }

        #[tokio::test]
        async fn test_take_send_stream_from_connected() {
            let mut socket = make_ipv4_socket();
            bind_socket(&mut socket);
            socket.listen_p3().unwrap();
            let listener = socket.tcp_listener_arc().unwrap().clone();
            let local_addr = listener.local_addr().unwrap();

            let client = tokio::net::TcpStream::connect(local_addr).await.unwrap();
            let (accepted, _) = listener.accept().await.unwrap();

            let mut connected = NetworkTcpSocket {
                tcp_state: TcpState::Connected(std::sync::Arc::new(accepted)),
                listen_backlog_size: DEFAULT_TCP_BACKLOG,
                family: SocketAddressFamily::Ipv4,
                options: Default::default(),
                send_taken: false,
                receive_taken: false,
            };

            let result = connected.take_send_stream();
            assert!(result.is_ok(), "first take_send_stream should succeed");

            let result = connected.take_send_stream();
            assert!(
                matches!(result, Err(ErrorCode::InvalidState)),
                "second take_send_stream should fail"
            );

            drop(client);
        }

        #[tokio::test]
        async fn test_take_receive_stream_from_connected() {
            let mut socket = make_ipv4_socket();
            bind_socket(&mut socket);
            socket.listen_p3().unwrap();
            let listener = socket.tcp_listener_arc().unwrap().clone();
            let local_addr = listener.local_addr().unwrap();

            let client = tokio::net::TcpStream::connect(local_addr).await.unwrap();
            let (accepted, _) = listener.accept().await.unwrap();

            let mut connected = NetworkTcpSocket {
                tcp_state: TcpState::Connected(std::sync::Arc::new(accepted)),
                listen_backlog_size: DEFAULT_TCP_BACKLOG,
                family: SocketAddressFamily::Ipv4,
                options: Default::default(),
                send_taken: false,
                receive_taken: false,
            };

            let result = connected.take_receive_stream();
            assert!(result.is_ok(), "first take_receive_stream should succeed");

            let result = connected.take_receive_stream();
            assert!(
                matches!(result, Err(ErrorCode::InvalidState)),
                "second take_receive_stream should fail"
            );

            drop(client);
        }

        #[test]
        fn test_take_send_stream_before_connected_errors() {
            let socket = make_ipv4_socket();
            let mut tcp = TcpSocket::Network(socket);
            let result = tcp.take_send_stream();
            assert!(
                result.is_err(),
                "take_send_stream on unconnected should fail"
            );
        }

        #[test]
        fn test_take_receive_stream_before_connected_errors() {
            let socket = make_ipv4_socket();
            let mut tcp = TcpSocket::Network(socket);
            let result = tcp.take_receive_stream();
            assert!(
                result.is_err(),
                "take_receive_stream on unconnected should fail"
            );
        }

        #[test]
        fn test_new_error_creates_closed_socket() {
            let err = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "test");
            let socket = NetworkTcpSocket::new_error(err, SocketAddressFamily::Ipv4);
            assert!(socket.local_address().is_err());
        }

        #[test]
        fn test_loopback_socket_props_roundtrip() {
            let lo_socket = loopback::TcpSocket {
                state: loopback::TcpState::Closed,
                listen_backlog_size: 42,
                keep_alive_enabled: true,
                keep_alive_idle_time: 1000,
                keep_alive_interval: 500,
                keep_alive_count: 3,
                hop_limit: 64,
                receive_buffer_size: 8192,
                send_buffer_size: 4096,
                family: SocketAddressFamily::Ipv4,
            };

            let props = LoopbackSocketProps::from(&lo_socket);
            assert_eq!(props.listen_backlog_size, 42);
            assert!(props.keep_alive_enabled);
            assert_eq!(props.keep_alive_idle_time, 1000);
            assert_eq!(props.keep_alive_interval, 500);
            assert_eq!(props.keep_alive_count, 3);
            assert_eq!(props.hop_limit, 64);
            assert_eq!(props.receive_buffer_size, 8192);
            assert_eq!(props.send_buffer_size, 4096);
            assert_eq!(props.family, SocketAddressFamily::Ipv4);
        }

        #[test]
        fn test_loopback_socket_props_to_accepted_socket() {
            let props = LoopbackSocketProps {
                listen_backlog_size: 128,
                keep_alive_enabled: false,
                keep_alive_idle_time: 7200,
                keep_alive_interval: 75,
                keep_alive_count: 9,
                hop_limit: 255,
                receive_buffer_size: 65536,
                send_buffer_size: 32768,
                family: SocketAddressFamily::Ipv6,
            };

            let (local_tx, _local_rx) = tokio::sync::mpsc::unbounded_channel();
            let (_remote_tx, remote_rx) = tokio::sync::mpsc::unbounded_channel();
            let conn = loopback::TcpConn {
                local_address: "127.0.0.1:8080".parse().unwrap(),
                remote_address: "127.0.0.1:9090".parse().unwrap(),
                rx: remote_rx,
                tx: local_tx,
            };

            let accepted = props.to_accepted_socket(conn);
            assert!(!accepted.keep_alive_enabled);
            assert_eq!(accepted.hop_limit, 255);
            assert_eq!(accepted.family, SocketAddressFamily::Ipv6);
            assert!(matches!(
                accepted.state,
                loopback::TcpState::Connected { accepted: true, .. }
            ));
        }
    }
}

/// Properties copied from a loopback TcpSocket for creating accepted sockets.
#[cfg(feature = "wasip3")]
#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct LoopbackSocketProps {
    pub listen_backlog_size: u32,
    pub keep_alive_enabled: bool,
    pub keep_alive_idle_time: u64,
    pub keep_alive_interval: u64,
    pub keep_alive_count: u32,
    pub hop_limit: u8,
    pub receive_buffer_size: u64,
    pub send_buffer_size: u32,
    pub family: SocketAddressFamily,
}

#[cfg(feature = "wasip3")]
impl From<&super::loopback::TcpSocket> for LoopbackSocketProps {
    fn from(socket: &super::loopback::TcpSocket) -> Self {
        Self {
            listen_backlog_size: socket.listen_backlog_size,
            keep_alive_enabled: socket.keep_alive_enabled,
            keep_alive_idle_time: socket.keep_alive_idle_time,
            keep_alive_interval: socket.keep_alive_interval,
            keep_alive_count: socket.keep_alive_count,
            hop_limit: socket.hop_limit,
            receive_buffer_size: socket.receive_buffer_size,
            send_buffer_size: socket.send_buffer_size,
            family: socket.family,
        }
    }
}

#[cfg(feature = "wasip3")]
impl LoopbackSocketProps {
    /// Create a loopback TcpSocket in the Connected state from an accepted connection.
    pub(crate) fn to_accepted_socket(
        &self,
        conn: super::loopback::TcpConn,
    ) -> super::loopback::TcpSocket {
        super::loopback::TcpSocket {
            state: super::loopback::TcpState::Connected {
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
        }
    }
}

/// P3 send stream type, either a network TcpStream or loopback channel.
#[cfg(feature = "wasip3")]
pub(crate) enum P3SendStream {
    Network(Arc<tokio::net::TcpStream>),
    Loopback {
        tx: tokio::sync::mpsc::UnboundedSender<(bytes::Bytes, tokio::sync::OwnedSemaphorePermit)>,
        permits: Arc<tokio::sync::Semaphore>,
    },
}

/// P3 receive stream type.
#[cfg(feature = "wasip3")]
pub(crate) enum P3ReceiveStream {
    Network(Arc<tokio::net::TcpStream>),
    Loopback(
        tokio::sync::mpsc::UnboundedReceiver<(bytes::Bytes, tokio::sync::OwnedSemaphorePermit)>,
    ),
}

/// Extract the accept channel from a loopback TcpSocket in Listening state.
#[cfg(feature = "wasip3")]
fn take_loopback_listen_info(
    lo: &mut super::loopback::TcpSocket,
) -> Result<P3LoopbackListenInfo, ErrorCode> {
    if let super::loopback::TcpState::Listening {
        ref mut rx,
        local_address: _,
        pending: _,
    } = lo.state
    {
        let (_, dummy_rx) = tokio::sync::mpsc::channel(1);
        let accept_rx = mem::replace(rx, dummy_rx);
        Ok(P3LoopbackListenInfo {
            rx: accept_rx,
            socket_props: LoopbackSocketProps::from(&*lo),
        })
    } else {
        Err(ErrorCode::InvalidState)
    }
}

/// P3 listen stream info for loopback, holding the accept channel.
#[cfg(feature = "wasip3")]
pub(crate) struct P3LoopbackListenInfo {
    pub rx: tokio::sync::mpsc::Receiver<super::loopback::TcpConn>,
    pub socket_props: LoopbackSocketProps,
}
