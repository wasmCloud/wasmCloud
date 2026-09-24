//! Host-owned TLS transport; guest streams carry application bytes only.

use super::*;
use bindings::wasmcloud::tls::dialer;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

type Io = Shared<ConnectionIo>;

pub struct Connection {
    send: Option<Io>,
    receive: Option<Io>,
}

struct ConnectionIo {
    stream: tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
    _permit: Option<crate::host::quota::ConnectionSlot>,
}

impl AsyncRead for ConnectionIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for ConnectionIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

impl dialer::Host for PluginTlsView<'_> {}

impl<T: 'static> dialer::HostWithStore<T> for PluginTls {
    async fn connect(
        accessor: &Accessor<T, Self>,
        endpoint: String,
    ) -> wasmtime::Result<Result<Resource<Connection>, Resource<TlsError>>> {
        let (policy, network) = accessor.with(|mut access| {
            let view = access.get();
            (view.policy.cloned(), view.network.cloned())
        });
        let result = match (policy, network) {
            (Some(policy), Some(network)) => network.dial(&policy, &endpoint).await,
            _ => Err(anyhow::anyhow!("TLS dialer has no plugin network grant")),
        };
        accessor.with(|mut access| {
            let table = access.get().table;
            match result {
                Ok((stream, permit)) => {
                    let io = Shared::new(ConnectionIo {
                        stream,
                        _permit: permit,
                    });
                    Ok(Ok(table.push(Connection {
                        send: Some(io.clone()),
                        receive: Some(io),
                    })?))
                }
                Err(err) => Ok(Err(
                    table.push(TlsError::msg(format!("TLS dial failed: {err:#}")))?
                )),
            }
        })
    }
}

impl dialer::HostConnection for PluginTlsView<'_> {
    fn drop(&mut self, this: Resource<Connection>) -> wasmtime::Result<()> {
        self.table.delete(this)?;
        Ok(())
    }
}

impl<T: 'static> dialer::HostConnectionWithStore<T> for PluginTls {
    fn send(
        mut store: Access<'_, T, Self>,
        this: Resource<Connection>,
        mut data: StreamReader<u8>,
    ) -> wasmtime::Result<ResultFuture> {
        let getter = store.getter();
        let Some(io) = store.get().table.get_mut(&this)?.send.take() else {
            data.close(&mut store)?;
            return FutureReader::new(
                &mut store,
                ResultProducer::ready(getter, Err(TlsError::msg("send already configured"))),
            );
        };
        let (ended_tx, ended_rx) = oneshot::channel();
        let (result_tx, result_rx) = oneshot::channel();
        data.pipe(&mut store, AsyncWriteConsumer::new(io, ended_tx))?;
        store.spawn(FnTask(async move || {
            let result = match ended_rx.await? {
                Ok(mut io) => io.shutdown().await,
                Err(err) => Err(err),
            };
            let _ = result_tx.send(result.map_err(TlsError::from));
            Ok(())
        }))?;
        FutureReader::new(&mut store, ResultProducer::new(getter, result_rx))
    }

    fn receive(
        mut store: Access<'_, T, Self>,
        this: Resource<Connection>,
    ) -> wasmtime::Result<(StreamReader<u8>, ResultFuture)> {
        let getter = store.getter();
        let Some(io) = store.get().table.get_mut(&this)?.receive.take() else {
            return already_configured(store, getter, "receive already configured");
        };
        let (ended_tx, ended_rx) = oneshot::channel();
        let (result_tx, result_rx) = oneshot::channel();
        let stream = StreamReader::new(&mut store, AsyncReadProducer::new(io, ended_tx))?;
        store.spawn(FnTask(async move || {
            let _ = result_tx.send(ended_rx.await?.map(drop).map_err(TlsError::from));
            Ok(())
        }))?;
        Ok((
            stream,
            FutureReader::new(&mut store, ResultProducer::new(getter, result_rx))?,
        ))
    }
}
