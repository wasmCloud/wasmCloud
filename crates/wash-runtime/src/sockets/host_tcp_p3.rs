//! P3 TCP socket host trait implementations with loopback support.

use super::WasiSocketsCtxView;
use super::tcp::{NonInheritedOptions, TcpSocket};
use crate::host::quota::ConnectionSlot;
use crate::sockets::{
    SocketAddrCheck, SocketAddrUse, SocketAddressFamily, WasiSockets,
    p3_socket_error_from_util as se,
};
use bytes::BytesMut;
use core::pin::Pin;
use core::task::{Context, Poll};
use io_lifetimes::AsSocketlike as _;
use std::net::{Shutdown, SocketAddr};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use wasmtime::component::{
    Access, Accessor, Destination, FutureReader, Resource, ResourceTable, Source, StreamConsumer,
    StreamProducer, StreamReader, StreamResult,
};
use wasmtime::error::Context as _;
use wasmtime::{AsContextMut as _, StoreContextMut};

use wasmtime_wasi::p3::bindings::sockets::types::{
    self, Duration, HostTcpSocket, HostTcpSocketWithStore, IpAddressFamily, IpSocketAddress,
};
use wasmtime_wasi::p3::sockets::{SocketError, SocketResult};

/// Type aliases for the upstream resource type (used in generated bindings)
type UpstreamTcpSocket = types::TcpSocket;

struct ConnectGuard<'a, T: Send + 'static> {
    store: &'a Accessor<T, WasiSockets>,
    socket_rep: u32,
    armed: bool,
}

impl<T: Send + 'static> Drop for ConnectGuard<'_, T> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.store.with(|mut store| {
            let view = store.get();
            let socket = Resource::<TcpSocket>::new_borrow(self.socket_rep);
            let Ok(socket) = view.table.get_mut(&socket) else {
                return;
            };
            let Ok(mut loopback) = view.ctx.loopback.lock() else {
                return;
            };
            socket.cancel_connect(&mut loopback);
        });
    }
}

/// Default buffer capacity for reads.
const DEFAULT_BUFFER_CAPACITY: usize = 8192;

/// How many connections the policy may refuse in one `poll_produce` before the
/// task yields.
///
/// A peer flooding connections from a refused address would otherwise keep one
/// poll accepting, checking and dropping for as long as they arrive, with the
/// guest waiting behind it.
const MAX_REFUSED_ACCEPTS_PER_POLL: usize = 32;

fn refusal_limit_result(cx: &Context<'_>, finish: bool) -> Poll<wasmtime::Result<StreamResult>> {
    if finish {
        Poll::Ready(Ok(StreamResult::Cancelled))
    } else {
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

/// Refuse an accepted connection with a reset rather than an orderly close, so
/// the peer learns it was refused instead of seeing a successful, empty
/// exchange. Mirrors wasmtime-wasi's own accept filter.
fn refuse_accepted(stream: TcpStream) {
    _ = stream.set_zero_linger();
    drop(stream);
}

fn get_socket<'a>(
    table: &'a ResourceTable,
    socket: &Resource<UpstreamTcpSocket>,
) -> SocketResult<&'a TcpSocket> {
    let socket = Resource::<TcpSocket>::new_borrow(socket.rep());
    table
        .get(&socket)
        .context("failed to get socket resource from table")
        .map_err(SocketError::trap)
}

fn get_socket_mut<'a>(
    table: &'a mut ResourceTable,
    socket: &Resource<UpstreamTcpSocket>,
) -> SocketResult<&'a mut TcpSocket> {
    let socket = Resource::<TcpSocket>::new_borrow(socket.rep());
    table
        .get_mut(&socket)
        .context("failed to get socket resource from table")
        .map_err(SocketError::trap)
}

struct ListenStreamProducer<T> {
    listener: Arc<TcpListener>,
    family: SocketAddressFamily,
    options: NonInheritedOptions,
    permissions: SocketAddrCheck,
    pending: Option<(std::io::Result<TcpStream>, Option<ConnectionSlot>)>,
    getter: for<'a> fn(&'a mut T) -> WasiSocketsCtxView<'a>,
}

impl<D> StreamProducer<D> for ListenStreamProducer<D>
where
    D: 'static,
{
    type Item = Resource<UpstreamTcpSocket>;
    type Buffer = Option<Self::Item>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let mut refused = 0;
        while self.pending.is_none() {
            let pending = match self.listener.poll_accept(cx) {
                Poll::Ready(Ok((stream, addr))) => {
                    let Ok(allowed) = self.permissions.check(addr, SocketAddrUse::TcpAccept) else {
                        refuse_accepted(stream);
                        refused += 1;
                        if refused >= MAX_REFUSED_ACCEPTS_PER_POLL {
                            return refusal_limit_result(cx, finish);
                        }
                        continue;
                    };
                    (Ok(stream), allowed.permit)
                }
                Poll::Ready(Err(err)) => (Err(err), None),
                Poll::Pending if finish => return Poll::Ready(Ok(StreamResult::Cancelled)),
                Poll::Pending => return Poll::Pending,
            };
            self.pending = Some(pending);
        }
        if dst.remaining(&mut store) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        // The loop above leaves `pending` set, so this only returns early if
        // that ever stops holding.
        let Some((res, permit)) = self.pending.take() else {
            return Poll::Pending;
        };
        let socket = TcpSocket::new_accept(res, &self.options, self.family)
            .map(|mut socket| {
                socket.hold_quota_slot(permit);
                socket
            })
            .unwrap_or_else(|err| {
                TcpSocket::Network(super::tcp::NetworkTcpSocket::new_error(err, self.family))
            });
        let WasiSocketsCtxView { table, .. } = (self.getter)(store.data_mut());
        let socket = table
            .push(socket)
            .context("failed to push socket resource to table")?;
        let socket = Resource::new_own(socket.rep());
        dst.set_buffer(Some(socket));
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

struct ReceiveStreamProducer {
    stream: Arc<TcpStream>,
    result: Option<oneshot::Sender<Result<(), types::ErrorCode>>>,
}

impl Drop for ReceiveStreamProducer {
    fn drop(&mut self) {
        self.close(Ok(()))
    }
}

impl ReceiveStreamProducer {
    fn close(&mut self, res: Result<(), types::ErrorCode>) {
        if let Some(tx) = self.result.take() {
            _ = self
                .stream
                .as_socketlike_view::<std::net::TcpStream>()
                .shutdown(Shutdown::Read);
            _ = tx.send(res);
        }
    }
}

impl<D> StreamProducer<D> for ReceiveStreamProducer {
    type Item = u8;
    type Buffer = BytesMut;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let res = 'result: {
            if dst.remaining(store.as_context_mut()) == Some(0) {
                return match self.stream.poll_read_ready(cx) {
                    Poll::Ready(Ok(())) => Poll::Ready(Ok(StreamResult::Completed)),
                    Poll::Ready(Err(err)) => break 'result Err(err.into()),
                    Poll::Pending if finish => Poll::Ready(Ok(StreamResult::Cancelled)),
                    Poll::Pending => Poll::Pending,
                };
            }

            let mut dst = dst.as_direct(store, DEFAULT_BUFFER_CAPACITY);
            let buf = dst.remaining();
            loop {
                match self.stream.try_read(buf) {
                    Ok(0) => break 'result Ok(()),
                    Ok(n) => {
                        dst.mark_written(n);
                        return Poll::Ready(Ok(StreamResult::Completed));
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        match self.stream.poll_read_ready(cx) {
                            Poll::Ready(Ok(())) => continue,
                            Poll::Ready(Err(err)) => break 'result Err(err.into()),
                            Poll::Pending if finish => {
                                return Poll::Ready(Ok(StreamResult::Cancelled));
                            }
                            Poll::Pending => return Poll::Pending,
                        }
                    }
                    Err(err) => break 'result Err(err.into()),
                }
            }
        };
        self.close(res);
        Poll::Ready(Ok(StreamResult::Dropped))
    }
}

struct SendStreamConsumer {
    stream: Arc<TcpStream>,
    result: Option<oneshot::Sender<Result<(), types::ErrorCode>>>,
}

impl Drop for SendStreamConsumer {
    fn drop(&mut self) {
        self.close(Ok(()))
    }
}

impl SendStreamConsumer {
    fn close(&mut self, res: Result<(), types::ErrorCode>) {
        if let Some(tx) = self.result.take() {
            _ = self
                .stream
                .as_socketlike_view::<std::net::TcpStream>()
                .shutdown(Shutdown::Write);
            _ = tx.send(res);
        }
    }
}

impl<D> StreamConsumer<D> for SendStreamConsumer {
    type Item = u8;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: StoreContextMut<D>,
        src: Source<Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let mut src = src.as_direct(store);
        let res = 'result: {
            if src.remaining().is_empty() {
                return match self.stream.poll_write_ready(cx) {
                    Poll::Ready(Ok(())) => Poll::Ready(Ok(StreamResult::Completed)),
                    Poll::Ready(Err(err)) => break 'result Err(err.into()),
                    Poll::Pending if finish => Poll::Ready(Ok(StreamResult::Cancelled)),
                    Poll::Pending => Poll::Pending,
                };
            }
            loop {
                match self.stream.try_write(src.remaining()) {
                    Ok(n) => {
                        debug_assert!(n > 0);
                        src.mark_read(n);
                        return Poll::Ready(Ok(StreamResult::Completed));
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        match self.stream.poll_write_ready(cx) {
                            Poll::Ready(Ok(())) => continue,
                            Poll::Ready(Err(err)) => break 'result Err(err.into()),
                            Poll::Pending if finish => {
                                return Poll::Ready(Ok(StreamResult::Cancelled));
                            }
                            Poll::Pending => return Poll::Pending,
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => {
                        break 'result Ok(());
                    }
                    Err(err) => break 'result Err(err.into()),
                }
            }
        };
        self.close(res);
        Poll::Ready(Ok(StreamResult::Dropped))
    }
}

impl types::Host for WasiSocketsCtxView<'_> {
    fn convert_error_code(&mut self, error: SocketError) -> wasmtime::Result<types::ErrorCode> {
        error.downcast()
    }
}

impl<T: Send> HostTcpSocketWithStore<T> for WasiSockets {
    async fn connect(
        store: &Accessor<T, Self>,
        socket: Resource<UpstreamTcpSocket>,
        remote_address: IpSocketAddress,
    ) -> SocketResult<()> {
        let remote_address = SocketAddr::from(remote_address);

        // Check if address is allowed
        let check = store.with(|mut view| view.get().ctx.socket_addr_check.clone());
        let allowed = check(remote_address, SocketAddrUse::TcpConnect)
            .into_allowed()
            .map_err(se)?;
        let remote_address = allowed.addr;

        // Start connect
        let plane = allowed.plane;
        let mut permit = allowed.permit;
        let connecting = store.with(|mut store| {
            let view = store.get();
            let socket_ref = get_socket_mut(view.table, &socket)?;
            let mut loopback = view
                .ctx
                .loopback
                .lock()
                .map_err(|e| SocketError::trap(wasmtime::format_err!("{e}")))?;
            let connecting = socket_ref
                .start_connect(&remote_address, plane, &mut loopback)
                .map_err(se)?;
            // Held until the guest drops the socket, so the budget bounds
            // concurrent connections rather than counting attempts.
            socket_ref.hold_quota_slot(permit.take());
            SocketResult::Ok(connecting)
        })?;

        let mut guard = ConnectGuard {
            store,
            socket_rep: socket.rep(),
            armed: true,
        };

        // Perform the actual connect
        let res = connecting.connect(remote_address).await;

        // Finish connect
        let result = store.with(|mut store| {
            let view = store.get();
            let socket_ref = get_socket_mut(view.table, &socket)?;
            let mut loopback = view
                .ctx
                .loopback
                .lock()
                .map_err(|e| SocketError::trap(wasmtime::format_err!("{e}")))?;
            socket_ref.finish_connect(res, &mut loopback).map_err(se)?;
            Ok(())
        });
        guard.armed = false;
        result
    }

    async fn listen(
        mut store: Access<'_, T, Self>,
        socket: Resource<UpstreamTcpSocket>,
    ) -> SocketResult<StreamReader<Resource<UpstreamTcpSocket>>> {
        let getter = store.getter();

        // A socket that has not been explicitly bound implicitly binds to an
        // ephemeral port during `listen`. Run the host's `socket_addr_check`
        // against that implicit bind address first — the same check `bind`
        // performs — so `listen` cannot be used to bind to an address the
        // network policy would otherwise deny. (bytecodealliance/wasmtime#13677)
        let (implicit_addr, listen_addr) = {
            let view = store.get();
            let socket_ref = get_socket_mut(view.table, &socket)?;
            let implicit = socket_ref
                .needs_implicit_bind()
                .then(|| crate::sockets::util::implicit_bind_addr(socket_ref.address_family()));
            let listen = match implicit {
                Some(addr) => addr,
                None => socket_ref.local_address().map_err(se)?,
            };
            (implicit, listen)
        };
        let check = store.get().ctx.socket_addr_check.clone();
        if let Some(addr) = implicit_addr {
            check(addr, SocketAddrUse::TcpBind)
                .into_allowed()
                .map_err(se)?;
        }
        check(listen_addr, SocketAddrUse::TcpListen)
            .into_allowed()
            .map_err(se)?;

        // Scope: do the listen and extract info
        enum ListenKind {
            Network {
                listener: Arc<TcpListener>,
                family: SocketAddressFamily,
                options: NonInheritedOptions,
            },
            Loopback(super::tcp::P3LoopbackListenInfo),
            Merged {
                listener: Arc<TcpListener>,
                options: NonInheritedOptions,
                loopback: super::tcp::P3LoopbackListenInfo,
            },
        }

        let kind = {
            let view = store.get();
            let socket_ref = get_socket_mut(view.table, &socket)?;
            let mut loopback = view
                .ctx
                .loopback
                .lock()
                .map_err(|e| SocketError::trap(wasmtime::format_err!("{e}")))?;
            socket_ref.listen_p3(&mut loopback).map_err(se)?;

            match socket_ref {
                TcpSocket::Network(net) => {
                    let listener = net.tcp_listener_arc().map_err(se)?;
                    let family = net.address_family();
                    let options = net.non_inherited_options().clone();
                    ListenKind::Network {
                        listener,
                        family,
                        options,
                    }
                }
                TcpSocket::Unspecified { net, .. } => {
                    let listener = net.tcp_listener_arc().map_err(se)?;
                    let options = net.non_inherited_options().clone();
                    let loopback_info = socket_ref.take_loopback_listen_rx().map_err(se)?;
                    ListenKind::Merged {
                        listener,
                        options,
                        loopback: loopback_info,
                    }
                }
                TcpSocket::Loopback(_) => {
                    let info = socket_ref.take_loopback_listen_rx().map_err(se)?;
                    ListenKind::Loopback(info)
                }
            }
        };

        match kind {
            ListenKind::Network {
                listener,
                family,
                options,
            } => StreamReader::new(
                &mut store,
                ListenStreamProducer {
                    listener,
                    family,
                    options,
                    permissions: check.clone(),
                    pending: None,
                    getter,
                },
            )
            .map_err(SocketError::trap),
            ListenKind::Loopback(info) => StreamReader::new(
                &mut store,
                LoopbackListenStreamProducer {
                    rx: info.rx,
                    socket_props: info.socket_props,
                    permissions: check.clone(),
                    pending: None,
                    getter,
                },
            )
            .map_err(SocketError::trap),
            ListenKind::Merged {
                listener,
                options,
                loopback,
                ..
            } => StreamReader::new(
                &mut store,
                MergedListenStreamProducer {
                    listener,
                    options,
                    loopback_rx: loopback.rx,
                    loopback_props: loopback.socket_props,
                    permissions: check,
                    pending: None,
                    loopback_closed: false,
                    getter,
                },
            )
            .map_err(SocketError::trap),
        }
    }

    fn send(
        mut store: Access<'_, T, Self>,
        socket: Resource<UpstreamTcpSocket>,
        mut data: StreamReader<u8>,
    ) -> wasmtime::Result<FutureReader<Result<(), types::ErrorCode>>> {
        let socket_ref =
            get_socket_mut(store.get().table, &socket).map_err(|e| wasmtime::format_err!("{e}"))?;
        match socket_ref.take_send_stream().map_err(se) {
            Ok(super::tcp::P3SendStream::Network(stream)) => {
                let (result_tx, result_rx) = oneshot::channel();
                data.pipe(
                    &mut store,
                    SendStreamConsumer {
                        stream,
                        result: Some(result_tx),
                    },
                )?;
                FutureReader::new(&mut store, result_rx)
            }
            Ok(super::tcp::P3SendStream::Loopback { tx, permits }) => {
                let (result_tx, result_rx) = oneshot::channel();
                data.pipe(
                    &mut store,
                    LoopbackSendStreamConsumer {
                        tx,
                        permits,
                        result: Some(result_tx),
                        pending_permit: None,
                    },
                )?;
                FutureReader::new(&mut store, result_rx)
            }
            Err(_err) => {
                data.close(&mut store)?;
                FutureReader::new(&mut store, async move {
                    wasmtime::error::Ok(Err(types::ErrorCode::InvalidState))
                })
            }
        }
    }

    fn receive(
        mut store: Access<T, Self>,
        socket: Resource<UpstreamTcpSocket>,
    ) -> wasmtime::Result<(StreamReader<u8>, FutureReader<Result<(), types::ErrorCode>>)> {
        let socket_ref =
            get_socket_mut(store.get().table, &socket).map_err(|e| wasmtime::format_err!("{e}"))?;
        match socket_ref.take_receive_stream().map_err(se) {
            Ok(super::tcp::P3ReceiveStream::Network(stream)) => {
                let (result_tx, result_rx) = oneshot::channel();
                Ok((
                    StreamReader::new(
                        &mut store,
                        ReceiveStreamProducer {
                            stream,
                            result: Some(result_tx),
                        },
                    )?,
                    FutureReader::new(&mut store, result_rx)?,
                ))
            }
            Ok(super::tcp::P3ReceiveStream::Loopback(rx)) => {
                let (result_tx, result_rx) = oneshot::channel();
                Ok((
                    StreamReader::new(
                        &mut store,
                        LoopbackReceiveStreamProducer {
                            rx,
                            result: Some(result_tx),
                            pending: None,
                        },
                    )?,
                    FutureReader::new(&mut store, result_rx)?,
                ))
            }
            Err(_err) => {
                use core::iter;
                Ok((
                    StreamReader::new(&mut store, iter::empty())?,
                    FutureReader::new(&mut store, async move {
                        wasmtime::error::Ok(Err(types::ErrorCode::InvalidState))
                    })?,
                ))
            }
        }
    }
}

impl HostTcpSocket for WasiSocketsCtxView<'_> {
    async fn bind(
        &mut self,
        socket: Resource<UpstreamTcpSocket>,
        local_address: IpSocketAddress,
    ) -> SocketResult<()> {
        let local_address = SocketAddr::from(local_address);
        let local_address = (self.ctx.socket_addr_check)(local_address, SocketAddrUse::TcpBind)
            .into_allowed()
            .map_err(se)?
            .addr;
        let mut loopback = self
            .ctx
            .loopback
            .lock()
            .map_err(|e| SocketError::trap(wasmtime::format_err!("{e}")))?;
        let socket_ref = get_socket_mut(self.table, &socket)?;
        socket_ref
            .start_bind(local_address, &mut loopback)
            .map_err(se)?;
        socket_ref.finish_bind().map_err(se)?;
        Ok(())
    }

    fn create(
        &mut self,
        address_family: IpAddressFamily,
    ) -> SocketResult<Resource<UpstreamTcpSocket>> {
        let family = match address_family {
            IpAddressFamily::Ipv4 => SocketAddressFamily::Ipv4,
            IpAddressFamily::Ipv6 => SocketAddressFamily::Ipv6,
        };
        let permit = self
            .ctx
            .socket_addr_check
            .check(
                crate::sockets::util::implicit_bind_addr(family),
                SocketAddrUse::TcpCreate,
            )
            .map_err(se)?
            .permit;
        let mut socket = TcpSocket::new(self.ctx, family).map_err(se)?;
        socket.hold_quota_slot(permit);
        let resource = self
            .table
            .push(socket)
            .context("failed to push socket resource to table")
            .map_err(SocketError::trap)?;
        Ok(Resource::new_own(resource.rep()))
    }

    fn get_local_address(
        &mut self,
        socket: Resource<UpstreamTcpSocket>,
    ) -> SocketResult<IpSocketAddress> {
        let sock = get_socket(self.table, &socket)?;
        Ok(sock.local_address().map_err(se)?.into())
    }

    fn get_remote_address(
        &mut self,
        socket: Resource<UpstreamTcpSocket>,
    ) -> SocketResult<IpSocketAddress> {
        let sock = get_socket(self.table, &socket)?;
        Ok(sock.remote_address().map_err(se)?.into())
    }

    fn get_is_listening(&mut self, socket: Resource<UpstreamTcpSocket>) -> wasmtime::Result<bool> {
        let sock = get_socket(self.table, &socket)?;
        Ok(sock.is_listening())
    }

    fn get_address_family(
        &mut self,
        socket: Resource<UpstreamTcpSocket>,
    ) -> wasmtime::Result<IpAddressFamily> {
        let sock = get_socket(self.table, &socket)?;
        Ok(sock.address_family().into())
    }

    fn set_listen_backlog_size(
        &mut self,
        socket: Resource<UpstreamTcpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        let sock = get_socket_mut(self.table, &socket)?;
        sock.set_listen_backlog_size(value).map_err(se)?;
        Ok(())
    }

    fn get_keep_alive_enabled(
        &mut self,
        socket: Resource<UpstreamTcpSocket>,
    ) -> SocketResult<bool> {
        let sock = get_socket(self.table, &socket)?;
        sock.keep_alive_enabled().map_err(se)
    }

    fn set_keep_alive_enabled(
        &mut self,
        socket: Resource<UpstreamTcpSocket>,
        value: bool,
    ) -> SocketResult<()> {
        let sock = get_socket_mut(self.table, &socket)?;
        sock.set_keep_alive_enabled(value).map_err(se)?;
        Ok(())
    }

    fn get_keep_alive_idle_time(
        &mut self,
        socket: Resource<UpstreamTcpSocket>,
    ) -> SocketResult<Duration> {
        let sock = get_socket(self.table, &socket)?;
        sock.keep_alive_idle_time().map_err(se)
    }

    fn set_keep_alive_idle_time(
        &mut self,
        socket: Resource<UpstreamTcpSocket>,
        value: Duration,
    ) -> SocketResult<()> {
        let sock = get_socket_mut(self.table, &socket)?;
        sock.set_keep_alive_idle_time(value).map_err(se)?;
        Ok(())
    }

    fn get_keep_alive_interval(
        &mut self,
        socket: Resource<UpstreamTcpSocket>,
    ) -> SocketResult<Duration> {
        let sock = get_socket(self.table, &socket)?;
        sock.keep_alive_interval().map_err(se)
    }

    fn set_keep_alive_interval(
        &mut self,
        socket: Resource<UpstreamTcpSocket>,
        value: Duration,
    ) -> SocketResult<()> {
        let sock = get_socket_mut(self.table, &socket)?;
        sock.set_keep_alive_interval(value).map_err(se)?;
        Ok(())
    }

    fn get_keep_alive_count(&mut self, socket: Resource<UpstreamTcpSocket>) -> SocketResult<u32> {
        let sock = get_socket(self.table, &socket)?;
        sock.keep_alive_count().map_err(se)
    }

    fn set_keep_alive_count(
        &mut self,
        socket: Resource<UpstreamTcpSocket>,
        value: u32,
    ) -> SocketResult<()> {
        let sock = get_socket_mut(self.table, &socket)?;
        sock.set_keep_alive_count(value).map_err(se)?;
        Ok(())
    }

    fn get_hop_limit(&mut self, socket: Resource<UpstreamTcpSocket>) -> SocketResult<u8> {
        let sock = get_socket(self.table, &socket)?;
        sock.hop_limit().map_err(se)
    }

    fn set_hop_limit(
        &mut self,
        socket: Resource<UpstreamTcpSocket>,
        value: u8,
    ) -> SocketResult<()> {
        let sock = get_socket_mut(self.table, &socket)?;
        sock.set_hop_limit(value).map_err(se)?;
        Ok(())
    }

    fn get_receive_buffer_size(
        &mut self,
        socket: Resource<UpstreamTcpSocket>,
    ) -> SocketResult<u64> {
        let sock = get_socket(self.table, &socket)?;
        sock.receive_buffer_size().map_err(se)
    }

    fn set_receive_buffer_size(
        &mut self,
        socket: Resource<UpstreamTcpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        let sock = get_socket_mut(self.table, &socket)?;
        sock.set_receive_buffer_size(value).map_err(se)?;
        Ok(())
    }

    fn get_send_buffer_size(&mut self, socket: Resource<UpstreamTcpSocket>) -> SocketResult<u64> {
        let sock = get_socket(self.table, &socket)?;
        sock.send_buffer_size().map_err(se)
    }

    fn set_send_buffer_size(
        &mut self,
        socket: Resource<UpstreamTcpSocket>,
        value: u64,
    ) -> SocketResult<()> {
        let sock = get_socket_mut(self.table, &socket)?;
        sock.set_send_buffer_size(value).map_err(se)?;
        Ok(())
    }

    fn drop(&mut self, sock: Resource<UpstreamTcpSocket>) -> wasmtime::Result<()> {
        let sock = Resource::<TcpSocket>::new_own(sock.rep());
        let socket = self
            .table
            .delete(sock)
            .context("failed to delete socket resource from table")?;
        let mut loopback = self
            .ctx
            .loopback
            .lock()
            .map_err(|e| wasmtime::format_err!("{e}"))?;
        socket.drop(&mut loopback)
    }
}

/// Produces accepted TCP socket resources from a loopback listen channel.
struct LoopbackListenStreamProducer<T> {
    rx: tokio::sync::mpsc::Receiver<super::loopback::TcpConn>,
    socket_props: super::tcp::LoopbackSocketProps,
    permissions: SocketAddrCheck,
    pending: Option<super::loopback::TcpConn>,
    getter: for<'a> fn(&'a mut T) -> WasiSocketsCtxView<'a>,
}

impl<D> StreamProducer<D> for LoopbackListenStreamProducer<D>
where
    D: 'static,
{
    type Item = Resource<UpstreamTcpSocket>;
    type Buffer = Option<Self::Item>;

    fn poll_produce<'a>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let this = self.get_mut();
        let mut refused = 0;
        while this.pending.is_none() {
            match this.rx.poll_recv(cx) {
                Poll::Ready(Some(conn)) => {
                    if this
                        .permissions
                        .check(conn.remote_address, SocketAddrUse::TcpAcceptVirtual)
                        .is_ok()
                    {
                        this.pending = Some(conn);
                    } else {
                        refused += 1;
                        if refused >= MAX_REFUSED_ACCEPTS_PER_POLL {
                            return refusal_limit_result(cx, finish);
                        }
                    }
                }
                Poll::Ready(None) => return Poll::Ready(Ok(StreamResult::Dropped)),
                Poll::Pending if finish => return Poll::Ready(Ok(StreamResult::Cancelled)),
                Poll::Pending => return Poll::Pending,
            }
        }
        if dst.remaining(&mut store) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        // The loop above leaves `pending` set, so this only returns early if
        // that ever stops holding.
        let Some(conn) = this.pending.take() else {
            return Poll::Pending;
        };
        let tcp_socket = TcpSocket::Loopback(this.socket_props.to_accepted_socket(conn));
        let WasiSocketsCtxView { table, .. } = (this.getter)(store.data_mut());
        let resource = table
            .push(tcp_socket)
            .context("failed to push loopback socket resource to table")?;
        dst.set_buffer(Some(Resource::new_own(resource.rep())));
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

/// Consumes bytes from the guest and sends them over a loopback channel.
struct LoopbackSendStreamConsumer {
    tx: tokio::sync::mpsc::UnboundedSender<(bytes::Bytes, tokio::sync::OwnedSemaphorePermit)>,
    permits: Arc<tokio::sync::Semaphore>,
    result: Option<oneshot::Sender<Result<(), types::ErrorCode>>>,
    /// In-progress permit acquisition, polled directly instead of spawning tasks.
    pending_permit: Option<
        Pin<Box<dyn std::future::Future<Output = tokio::sync::OwnedSemaphorePermit> + Send>>,
    >,
}

impl Drop for LoopbackSendStreamConsumer {
    fn drop(&mut self) {
        if let Some(tx) = self.result.take() {
            _ = tx.send(Ok(()));
        }
    }
}

impl<D> StreamConsumer<D> for LoopbackSendStreamConsumer {
    type Item = u8;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: StoreContextMut<D>,
        src: Source<Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let mut src = src.as_direct(store);
        let data = src.remaining();
        if data.is_empty() {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        // If we have a pending permit acquisition, poll it
        if let Some(fut) = self.pending_permit.as_mut() {
            match fut.as_mut().poll(cx) {
                Poll::Ready(permit) => {
                    self.pending_permit = None;
                    let chunk = bytes::Bytes::copy_from_slice(data);
                    let n = data.len();
                    if self.tx.send((chunk, permit)).is_err() {
                        if let Some(tx) = self.result.take() {
                            _ = tx.send(Ok(()));
                        }
                        return Poll::Ready(Ok(StreamResult::Dropped));
                    }
                    src.mark_read(n);
                    return Poll::Ready(Ok(StreamResult::Completed));
                }
                Poll::Pending if finish => return Poll::Ready(Ok(StreamResult::Cancelled)),
                Poll::Pending => return Poll::Pending,
            }
        }

        // Try to acquire a permit synchronously
        match self.permits.clone().try_acquire_owned() {
            Ok(permit) => {
                let chunk = bytes::Bytes::copy_from_slice(data);
                let n = data.len();
                if self.tx.send((chunk, permit)).is_err() {
                    if let Some(tx) = self.result.take() {
                        _ = tx.send(Ok(()));
                    }
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
                src.mark_read(n);
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Err(_) => {
                // Store the future to poll on next call instead of spawning a task
                let permits = self.permits.clone();
                self.pending_permit = Some(Box::pin(async move {
                    #[allow(clippy::unwrap_used)]
                    // The semaphore is never closed while the consumer is alive
                    permits.acquire_owned().await.unwrap()
                }));
                if finish {
                    Poll::Ready(Ok(StreamResult::Cancelled))
                } else {
                    // Re-poll the newly created future to register the waker
                    if let Some(fut) = self.pending_permit.as_mut() {
                        let _ = fut.as_mut().poll(cx);
                    }
                    Poll::Pending
                }
            }
        }
    }
}

/// Produces bytes from a loopback receive channel for the guest to read.
struct LoopbackReceiveStreamProducer {
    rx: tokio::sync::mpsc::UnboundedReceiver<(bytes::Bytes, tokio::sync::OwnedSemaphorePermit)>,
    result: Option<oneshot::Sender<Result<(), types::ErrorCode>>>,
    /// Buffered data from a previous recv that couldn't be delivered (e.g. zero-length read).
    pending: Option<bytes::Bytes>,
}

impl Drop for LoopbackReceiveStreamProducer {
    fn drop(&mut self) {
        if let Some(tx) = self.result.take() {
            _ = tx.send(Ok(()));
        }
    }
}

impl<D> StreamProducer<D> for LoopbackReceiveStreamProducer {
    type Item = u8;
    type Buffer = BytesMut;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        // Zero-length read: report readiness without consuming data
        if dst.remaining(store.as_context_mut()) == Some(0) {
            if self.pending.is_some() {
                return Poll::Ready(Ok(StreamResult::Completed));
            }
            return match self.rx.poll_recv(cx) {
                Poll::Ready(Some((data, _permit))) => {
                    // Buffer the data for the next non-zero read
                    self.pending = Some(data);
                    Poll::Ready(Ok(StreamResult::Completed))
                }
                Poll::Ready(None) => {
                    self.close(Ok(()));
                    Poll::Ready(Ok(StreamResult::Dropped))
                }
                Poll::Pending if finish => Poll::Ready(Ok(StreamResult::Cancelled)),
                Poll::Pending => Poll::Pending,
            };
        }

        // Drain any buffered data first
        let data = if let Some(data) = self.pending.take() {
            data
        } else {
            match self.rx.poll_recv(cx) {
                Poll::Ready(Some((data, _permit))) => data,
                Poll::Ready(None) => {
                    self.close(Ok(()));
                    return Poll::Ready(Ok(StreamResult::Dropped));
                }
                Poll::Pending if finish => return Poll::Ready(Ok(StreamResult::Cancelled)),
                Poll::Pending => return Poll::Pending,
            }
        };

        let mut dst = dst.as_direct(store, DEFAULT_BUFFER_CAPACITY);
        let buf = dst.remaining();
        let n = data.len().min(buf.len());
        if let Some((dst, src)) = buf.get_mut(..n).zip(data.get(..n)) {
            dst.copy_from_slice(src);
        }
        dst.mark_written(n);
        // If we couldn't deliver all the data, re-buffer the remainder
        if n < data.len() {
            self.pending = Some(data.slice(n..));
        }
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

impl LoopbackReceiveStreamProducer {
    fn close(&mut self, res: Result<(), types::ErrorCode>) {
        if let Some(tx) = self.result.take() {
            _ = tx.send(res);
        }
    }
}

/// Produces accepted TCP socket resources from both a network TcpListener
/// and a loopback accept channel, merging connections from both sources.
/// Used for sockets bound to 0.0.0.0/[::] (Unspecified).
struct MergedListenStreamProducer<T> {
    listener: Arc<TcpListener>,
    options: NonInheritedOptions,
    loopback_rx: tokio::sync::mpsc::Receiver<super::loopback::TcpConn>,
    loopback_props: super::tcp::LoopbackSocketProps,
    permissions: SocketAddrCheck,
    pending: Option<MergedAccept>,
    /// Set once the loopback channel has closed, so a channel that answers
    /// `Ready(None)` forever is not polled forever.
    loopback_closed: bool,
    getter: for<'a> fn(&'a mut T) -> WasiSocketsCtxView<'a>,
}

enum MergedAccept {
    Network(std::io::Result<TcpStream>, Option<ConnectionSlot>),
    Loopback(super::loopback::TcpConn),
}

impl<D> StreamProducer<D> for MergedListenStreamProducer<D>
where
    D: 'static,
{
    type Item = Resource<UpstreamTcpSocket>;
    type Buffer = Option<Self::Item>;

    fn poll_produce<'a>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let this = self.get_mut();

        let mut refused = 0;
        while this.pending.is_none() {
            // `ready` tracks whether either source produced something this
            // round. Only when neither did is it safe to park: a source that
            // answered `Ready` has registered no waker, so returning `Pending`
            // on the strength of the *other* source's waker would strand every
            // connection this one goes on to queue.
            let mut ready = false;

            match this.listener.poll_accept(cx) {
                Poll::Ready(Ok((stream, addr))) => {
                    ready = true;
                    if let Ok(allowed) = this.permissions.check(addr, SocketAddrUse::TcpAccept) {
                        this.pending = Some(MergedAccept::Network(Ok(stream), allowed.permit));
                    } else {
                        refuse_accepted(stream);
                        refused += 1;
                    }
                }
                Poll::Ready(Err(err)) => {
                    ready = true;
                    this.pending = Some(MergedAccept::Network(Err(err), None));
                }
                Poll::Pending => {}
            }

            if this.pending.is_none() && !this.loopback_closed {
                match this.loopback_rx.poll_recv(cx) {
                    Poll::Ready(Some(conn)) => {
                        ready = true;
                        if this
                            .permissions
                            .check(conn.remote_address, SocketAddrUse::TcpAcceptVirtual)
                            .is_ok()
                        {
                            this.pending = Some(MergedAccept::Loopback(conn));
                        } else {
                            refused += 1;
                        }
                    }
                    // The loopback half is finished, but the network half is
                    // not: stop polling a closed channel — it reports `Ready`
                    // forever — and keep serving the listener.
                    Poll::Ready(None) => this.loopback_closed = true,
                    Poll::Pending => {}
                }
            }

            if this.pending.is_some() {
                break;
            }
            if refused >= MAX_REFUSED_ACCEPTS_PER_POLL {
                return refusal_limit_result(cx, finish);
            }
            if !ready {
                return if finish {
                    Poll::Ready(Ok(StreamResult::Cancelled))
                } else {
                    Poll::Pending
                };
            }
        }

        if dst.remaining(&mut store) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        // The loop above leaves `pending` set, so this only returns early if
        // that ever stops holding.
        let Some(pending) = this.pending.take() else {
            return Poll::Pending;
        };
        let socket = match pending {
            MergedAccept::Network(res, permit) => {
                TcpSocket::new_accept(res, &this.options, this.loopback_props.family)
                    .map(|mut socket| {
                        socket.hold_quota_slot(permit);
                        socket
                    })
                    .unwrap_or_else(|err| {
                        TcpSocket::Network(super::tcp::NetworkTcpSocket::new_error(
                            err,
                            this.loopback_props.family,
                        ))
                    })
            }
            MergedAccept::Loopback(conn) => {
                TcpSocket::Loopback(this.loopback_props.to_accepted_socket(conn))
            }
        };
        let WasiSocketsCtxView { table, .. } = (this.getter)(store.data_mut());
        let resource = table
            .push(socket)
            .context("failed to push socket resource to table")?;
        dst.set_buffer(Some(Resource::new_own(resource.rep())));
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_limit_cancels_finished_read() {
        let cx = Context::from_waker(std::task::Waker::noop());
        assert!(matches!(
            refusal_limit_result(&cx, true),
            Poll::Ready(Ok(StreamResult::Cancelled))
        ));
    }
}
