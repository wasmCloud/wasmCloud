//! P3 TCP socket host trait implementations with loopback support.

use super::WasiSocketsCtxView;
use super::tcp::{NonInheritedOptions, TcpSocket};
use crate::sockets::{
    SocketAddrUse, SocketAddressFamily, WasiSockets, p3_socket_error_from_util as se,
};
use bytes::BytesMut;
use core::pin::Pin;
use core::task::{Context, Poll};
use io_lifetimes::AsSocketlike as _;
use std::io::Cursor;
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

/// Default buffer capacity for reads.
const DEFAULT_BUFFER_CAPACITY: usize = 8192;

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
    getter: for<'a> fn(&'a mut T) -> WasiSocketsCtxView<'a>,
}

impl<D> StreamProducer<D> for ListenStreamProducer<D>
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
        if dst.remaining(&mut store) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        let res = match self.listener.poll_accept(cx) {
            Poll::Ready(res) => res.map(|(stream, _)| stream),
            Poll::Pending if finish => return Poll::Ready(Ok(StreamResult::Cancelled)),
            Poll::Pending => return Poll::Pending,
        };
        let socket = TcpSocket::new_accept(res, &self.options, self.family).unwrap_or_else(|err| {
            // Create a Network socket in error state - wrap in Connected with a dummy
            // For simplicity, just create a closed socket on error
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
    type Buffer = Cursor<BytesMut>;

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

impl HostTcpSocketWithStore for WasiSockets {
    async fn connect<T>(
        store: &Accessor<T, Self>,
        socket: Resource<UpstreamTcpSocket>,
        remote_address: IpSocketAddress,
    ) -> SocketResult<()> {
        let remote_address = SocketAddr::from(remote_address);

        // Check if address is allowed
        let check = store.with(|mut view| view.get().ctx.socket_addr_check.clone());
        if !check(remote_address, SocketAddrUse::TcpConnect).await {
            return Err(types::ErrorCode::AccessDenied.into());
        }

        // Start connect
        let connecting = store.with(|mut store| {
            let view = store.get();
            let socket_ref = get_socket_mut(view.table, &socket)?;
            let mut loopback = view
                .ctx
                .loopback
                .lock()
                .map_err(|e| SocketError::trap(wasmtime::format_err!("{e}")))?;
            let connecting = socket_ref
                .start_connect(&remote_address, &mut loopback)
                .map_err(se)?;
            SocketResult::Ok(connecting)
        })?;

        // Perform the actual connect
        let res = connecting.connect(remote_address).await;

        // Finish connect
        store.with(|mut store| {
            let view = store.get();
            let socket_ref = get_socket_mut(view.table, &socket)?;
            let mut loopback = view
                .ctx
                .loopback
                .lock()
                .map_err(|e| SocketError::trap(wasmtime::format_err!("{e}")))?;
            socket_ref.finish_connect(res, &mut loopback).map_err(se)?;
            Ok(())
        })
    }

    fn listen<T: 'static>(
        mut store: Access<'_, T, Self>,
        socket: Resource<UpstreamTcpSocket>,
    ) -> SocketResult<StreamReader<Resource<UpstreamTcpSocket>>> {
        let getter = store.getter();

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
                    let listener = net.tcp_listener_arc().map_err(se)?.clone();
                    let family = net.address_family();
                    let options = net.non_inherited_options().clone();
                    ListenKind::Network {
                        listener,
                        family,
                        options,
                    }
                }
                TcpSocket::Unspecified { net, .. } => {
                    let listener = net.tcp_listener_arc().map_err(se)?.clone();
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
                    getter,
                },
            )
            .map_err(SocketError::trap),
            ListenKind::Loopback(info) => StreamReader::new(
                &mut store,
                LoopbackListenStreamProducer {
                    rx: info.rx,
                    socket_props: info.socket_props,
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
                    getter,
                },
            )
            .map_err(SocketError::trap),
        }
    }

    fn send<T: 'static>(
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

    fn receive<T: 'static>(
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
        if !(self.ctx.socket_addr_check)(local_address, SocketAddrUse::TcpBind).await {
            return Err(types::ErrorCode::AccessDenied.into());
        }
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
        let socket = TcpSocket::new(self.ctx, family).map_err(se)?;
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
        if dst.remaining(&mut store) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        let this = self.get_mut();
        match this.rx.poll_recv(cx) {
            Poll::Ready(Some(conn)) => {
                let tcp_socket = TcpSocket::Loopback(this.socket_props.to_accepted_socket(conn));
                let WasiSocketsCtxView { table, .. } = (this.getter)(store.data_mut());
                let resource = table
                    .push(tcp_socket)
                    .context("failed to push loopback socket resource to table")?;
                let resource = Resource::new_own(resource.rep());
                dst.set_buffer(Some(resource));
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Poll::Ready(None) => {
                // Channel closed — listener was dropped
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Poll::Pending if finish => Poll::Ready(Ok(StreamResult::Cancelled)),
            Poll::Pending => Poll::Pending,
        }
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
    type Buffer = Cursor<BytesMut>;

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
    getter: for<'a> fn(&'a mut T) -> WasiSocketsCtxView<'a>,
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
        if dst.remaining(&mut store) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        let this = self.get_mut();

        // Poll network listener
        match this.listener.poll_accept(cx) {
            Poll::Ready(res) => {
                let res = res.map(|(stream, _)| stream);
                let socket = TcpSocket::new_accept(res, &this.options, this.loopback_props.family)
                    .unwrap_or_else(|err| {
                        TcpSocket::Network(super::tcp::NetworkTcpSocket::new_error(
                            err,
                            this.loopback_props.family,
                        ))
                    });
                let WasiSocketsCtxView { table, .. } = (this.getter)(store.data_mut());
                let resource = table
                    .push(socket)
                    .context("failed to push socket resource to table")?;
                dst.set_buffer(Some(Resource::new_own(resource.rep())));
                return Poll::Ready(Ok(StreamResult::Completed));
            }
            Poll::Pending => {}
        }

        // Poll loopback channel
        match this.loopback_rx.poll_recv(cx) {
            Poll::Ready(Some(conn)) => {
                let tcp_socket = TcpSocket::Loopback(this.loopback_props.to_accepted_socket(conn));
                let WasiSocketsCtxView { table, .. } = (this.getter)(store.data_mut());
                let resource = table
                    .push(tcp_socket)
                    .context("failed to push loopback socket resource to table")?;
                dst.set_buffer(Some(Resource::new_own(resource.rep())));
                return Poll::Ready(Ok(StreamResult::Completed));
            }
            Poll::Ready(None) => {
                // Loopback channel closed — not fatal, network listener may still be active
            }
            Poll::Pending => {}
        }

        if finish {
            Poll::Ready(Ok(StreamResult::Cancelled))
        } else {
            Poll::Pending
        }
    }
}
