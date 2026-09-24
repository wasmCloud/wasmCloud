//! `wasmcloud:tls` and `wasi:tls` for host component plugins, with trust from
//! the plugin's own `allowedHosts` grant.
//!
//! The host owns the handshake and keys. Stream transforms wrap a guest's
//! transport; the dialer owns the network connection too. Both require trust
//! declared for the server name before writing any bytes.
//!
//! The two `client` interfaces share signatures and resources, so
//! [`impl_tls_host`] instantiates one body for each.

mod bindings;
mod dialer;
mod io;
pub use dialer::Connection;

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::io::AsyncWriteExt as _;
use tokio::sync::oneshot;
use wasmtime::StoreContextMut;
use wasmtime::component::{
    Access, Accessor, AccessorTask, FutureProducer, FutureReader, HasData, Linker, Resource,
    ResourceTable, StreamReader,
};

use self::io::{
    AsyncReadProducer, AsyncWriteConsumer, Closed, Deferred, Hangup, HangupWriter, Reader,
    SessionIo, Shared, Writer,
};
use super::PluginTlsPolicy;
use crate::engine::ctx::SharedCtx;

/// Whether the host serves the `namespace:package@version` TLS package: any
/// `wasmcloud:tls@0.1.x`, or exactly `wasi:tls@0.3.0-draft`, the versions
/// [`add_to_linker`] links (the linker accepts a semver-compatible patch).
pub(crate) fn serves(namespace: &str, package: &str, version: Option<&semver::Version>) -> bool {
    let Some(version) = version else {
        return false;
    };
    match (namespace, package) {
        ("wasmcloud", "tls") => version.major == 0 && version.minor == 1,
        ("wasi", "tls") => {
            (version.major, version.minor, version.patch) == (0, 3, 0)
                && version.pre.as_str() == "draft"
        }
        _ => false,
    }
}

/// Which of the clients that apply a plugin's declared trust it imports.
///
/// An import under an `(implements ..)` label counts for none: these are
/// linked by interface name, so a labeled one would not reach them.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ClientImports {
    /// `wasmcloud:tls/client` or `wasi:tls/client`, over a guest's socket.
    pub(crate) tls_client: bool,
    /// `wasmcloud:tls/dialer`, which owns the connection too.
    pub(crate) dialer: bool,
    /// `wasi:http`, whose outgoing requests take the grant's trust.
    pub(crate) http_client: bool,
}

impl ClientImports {
    pub(crate) fn of<'a>(imports: impl IntoIterator<Item = &'a crate::wit::WitInterface>) -> Self {
        let mut clients = Self::default();
        for wit in imports.into_iter().filter(|wit| wit.name.is_none()) {
            let version = wit.version.as_ref();
            if serves(&wit.namespace, &wit.package, version) {
                clients.tls_client |= wit.interfaces.contains("client");
                clients.dialer |= wit.namespace == "wasmcloud" && wit.interfaces.contains("dialer");
            }
            if (wit.namespace.as_str(), wit.package.as_str()) == ("wasi", "http") {
                clients.http_client |= match version.map(|v| (v.major, v.minor)) {
                    Some((0, 2)) => wit.interfaces.contains("outgoing-handler"),
                    Some((0, 3)) => wit.interfaces.contains("client"),
                    _ => false,
                };
            }
        }
        clients
    }

    /// Whether anything here can apply declared trust at all.
    pub(crate) fn any(self) -> bool {
        self.tls_client || self.dialer || self.http_client
    }
}

/// Link both packages into a plugin linker. Unused unless the plugin imports
/// one of them.
pub(crate) fn add_to_linker(linker: &mut Linker<SharedCtx>) -> anyhow::Result<()> {
    use bindings::{wasi, wasmcloud};
    wasmcloud::tls::types::add_to_linker::<_, PluginTls>(linker, view)?;
    wasmcloud::tls::client::add_to_linker::<_, PluginTls>(linker, view)?;
    wasmcloud::tls::dialer::add_to_linker::<_, PluginTls>(linker, view)?;
    wasi::tls::types::add_to_linker::<_, PluginTls>(linker, view)?;
    wasi::tls::client::add_to_linker::<_, PluginTls>(linker, view)?;
    Ok(())
}

fn view(ctx: &mut SharedCtx) -> PluginTlsView<'_> {
    PluginTlsView {
        table: &mut ctx.table,
        policy: ctx.active_ctx.plugin_tls.as_ref(),
        network: ctx.active_ctx.plugin_network.as_ref(),
    }
}

/// The store data this implementation reads.
pub(crate) struct PluginTlsView<'a> {
    table: &'a mut ResourceTable,
    policy: Option<&'a Arc<PluginTlsPolicy>>,
    network: Option<&'a Arc<super::PluginNetwork>>,
}

/// [`HasData`] marker for this implementation.
pub(crate) struct PluginTls;

impl HasData for PluginTls {
    type Data<'a> = PluginTlsView<'a>;
}

/// A TLS failure as the guest sees it: a message, cheap to clone into every
/// stream and future that reports it.
#[derive(Debug, Clone)]
pub struct TlsError(Arc<str>);

impl TlsError {
    fn msg(msg: impl Into<Arc<str>>) -> Self {
        Self(msg.into())
    }
}

impl std::fmt::Display for TlsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<TlsError> for std::io::Error {
    fn from(err: TlsError) -> Self {
        std::io::Error::other(err.0.to_string())
    }
}

impl From<std::io::Error> for TlsError {
    fn from(err: std::io::Error) -> Self {
        Self::msg(err.to_string())
    }
}

type Session = Box<dyn SessionIo>;

/// Host state behind a guest `connector`.
pub struct Connector {
    session: Unresolved,
    /// The ciphertext the handshake writes, set by `send`.
    send: Option<Writer>,
    /// The ciphertext the handshake reads, set by `receive`.
    recv: Option<Reader>,
}

fn new_connector(table: &mut ResourceTable) -> wasmtime::Result<Resource<Connector>> {
    Ok(table.push(Connector {
        session: Unresolved(Shared::new(Deferred::pending())),
        send: None,
        recv: None,
    })?)
}

/// A connector's session until `connect` settles it. Dropped unsettled — the
/// connector dropped before `connect`, or `connect` cancelled mid-handshake —
/// it closes the session, or the stream tasks `send` and `receive` spawned
/// would wait on it for the life of the plugin's store.
struct Unresolved(Shared<Deferred<Session>>);

impl Unresolved {
    fn resolve(self, io: Session) {
        self.0.lock().resolve(io);
    }
}

impl Drop for Unresolved {
    fn drop(&mut self) {
        self.0.lock().resolve(Box::new(Closed(TlsError::msg(
            "TLS connection abandoned before its handshake finished",
        ))));
    }
}

type ResultFuture = FutureReader<Result<(), Resource<TlsError>>>;

fn send<T: 'static>(
    mut store: Access<'_, T, PluginTls>,
    this: Resource<Connector>,
    mut cleartext: StreamReader<u8>,
) -> wasmtime::Result<(StreamReader<u8>, ResultFuture)> {
    let getter = store.getter();
    if store.get().table.get(&this)?.send.is_some() {
        cleartext.close(&mut store)?;
        return already_configured(store, getter, "send already configured");
    }

    let (ciphertext_reader, ciphertext_writer) = io::pipe();
    let (ciphertext_ended_tx, ciphertext_ended_rx) = oneshot::channel();
    let (cleartext_ended_tx, cleartext_ended_rx) = oneshot::channel();
    let (result_tx, result_rx) = oneshot::channel();

    let session = {
        let connector = store.get().table.get_mut(&this)?;
        connector.send = Some(ciphertext_writer);
        connector.session.0.clone()
    };
    cleartext.pipe(
        &mut store,
        AsyncWriteConsumer::new(session, cleartext_ended_tx),
    )?;
    let ciphertext = AsyncReadProducer::new(ciphertext_reader, ciphertext_ended_tx);
    store.spawn(FnTask(async move || {
        let cleartext_result = match cleartext_ended_rx.await? {
            // The guest closed its cleartext: send `close_notify`.
            Ok(mut session) => session.shutdown().await,
            Err(e) => Err(e),
        };
        let ciphertext_result = ciphertext_ended_rx.await?;
        let _ = result_tx.send(
            cleartext_result
                .and(ciphertext_result)
                .map_err(TlsError::from),
        );
        Ok(())
    }))?;
    Ok((
        StreamReader::new(&mut store, ciphertext)?,
        FutureReader::new(&mut store, ResultProducer::new(getter, result_rx))?,
    ))
}

fn receive<T: 'static>(
    mut store: Access<'_, T, PluginTls>,
    this: Resource<Connector>,
    mut ciphertext: StreamReader<u8>,
) -> wasmtime::Result<(StreamReader<u8>, ResultFuture)> {
    let getter = store.getter();
    if store.get().table.get(&this)?.recv.is_some() {
        ciphertext.close(&mut store)?;
        return already_configured(store, getter, "receive already configured");
    }

    let (ciphertext_reader, ciphertext_writer) = io::pipe();
    let (ciphertext_ended_tx, ciphertext_ended_rx) = oneshot::channel();
    let (cleartext_ended_tx, cleartext_ended_rx) = oneshot::channel();
    let (result_tx, result_rx) = oneshot::channel();

    let session = {
        let connector = store.get().table.get_mut(&this)?;
        connector.recv = Some(ciphertext_reader);
        connector.session.0.clone()
    };
    // A guest that stops reading cleartext stops the ciphertext feeding it,
    // rather than leaving that consumer parked on a full pipe.
    let hangup = Arc::new(Hangup::default());
    ciphertext.pipe(
        &mut store,
        AsyncWriteConsumer::new(
            HangupWriter::new(ciphertext_writer, Arc::clone(&hangup)),
            ciphertext_ended_tx,
        ),
    )?;
    let cleartext =
        AsyncReadProducer::new(session, cleartext_ended_tx).with_hangup(Arc::clone(&hangup));
    store.spawn(FnTask(async move || {
        let ciphertext_result = match ciphertext_ended_rx.await? {
            // The network side closed: tell the session its transport is gone.
            Ok(mut inner) => inner.shutdown().await,
            // The guest's own hangup, not a failure.
            Err(_) if hangup.is_set() => Ok(()),
            Err(e) => Err(e),
        };
        let cleartext_result = cleartext_ended_rx.await?;
        let _ = result_tx.send(
            cleartext_result
                .and(ciphertext_result)
                .map_err(TlsError::from),
        );
        Ok(())
    }))?;
    Ok((
        StreamReader::new(&mut store, cleartext)?,
        FutureReader::new(&mut store, ResultProducer::new(getter, result_rx))?,
    ))
}

fn already_configured<T: 'static>(
    mut store: Access<'_, T, PluginTls>,
    getter: Getter<T>,
    msg: &'static str,
) -> wasmtime::Result<(StreamReader<u8>, ResultFuture)> {
    let err = TlsError::msg(msg);
    Ok((
        StreamReader::new(&mut store, Closed(err.clone()))?,
        FutureReader::new(&mut store, ResultProducer::ready(getter, Err(err)))?,
    ))
}

async fn connect<T: 'static>(
    accessor: &Accessor<T, PluginTls>,
    this: Resource<Connector>,
    server_name: String,
) -> wasmtime::Result<Result<(), Resource<TlsError>>> {
    let (handshake, session) = accessor.with(|mut access| -> wasmtime::Result<_> {
        let view = access.get();
        let connector = view.table.delete(this)?;
        let handshake =
            prepare_handshake(view.policy, connector.send, connector.recv, &server_name);
        Ok((handshake, connector.session))
    })?;

    let outcome = match handshake {
        Ok(handshake) => handshake
            .await
            .map_err(|e| TlsError::msg(format!("TLS handshake with {server_name:?} failed: {e}"))),
        Err(e) => Err(e),
    };
    match outcome {
        Ok(stream) => {
            session.resolve(Box::new(stream));
            Ok(Ok(()))
        }
        Err(e) => {
            tracing::debug!(server_name, err = %e, "plugin TLS connection refused");
            session.resolve(Box::new(Closed(e.clone())));
            Ok(Err(accessor.with(|mut access| access.get().table.push(e))?))
        }
    }
}

type Handshake = tokio_rustls::Connect<tokio::io::Join<Reader, Writer>>;

/// Everything `connect` decides before a byte is written: that both
/// directions are set up, that the name is one a certificate can be valid
/// for, and that the plugin's grant declares trust for it.
fn prepare_handshake(
    policy: Option<&Arc<PluginTlsPolicy>>,
    send: Option<Writer>,
    recv: Option<Reader>,
    server_name: &str,
) -> Result<Handshake, TlsError> {
    let send = send.ok_or_else(|| TlsError::msg("send() must be called before connect()"))?;
    let recv = recv.ok_or_else(|| TlsError::msg("receive() must be called before connect()"))?;
    let name = rustls::pki_types::ServerName::try_from(server_name.to_owned()).map_err(|e| {
        TlsError::msg(format!(
            "{server_name:?} is not a valid TLS server name: {e}"
        ))
    })?;
    let trust = policy
        .and_then(|policy| policy.for_server_name(server_name))
        .ok_or_else(|| {
            TlsError::msg(format!(
                "no allowedHosts entry for {server_name:?} declares `tls` for this plugin"
            ))
        })?;
    let connector = tokio_rustls::TlsConnector::from(trust.client_config());
    Ok(connector.connect(name, tokio::io::join(recv, send)))
}

macro_rules! impl_tls_host {
    ($pkg:ident) => {
        impl<'a> bindings::$pkg::tls::types::Host for PluginTlsView<'a> {}
        impl<'a> bindings::$pkg::tls::client::Host for PluginTlsView<'a> {}

        impl<'a> bindings::$pkg::tls::types::HostError for PluginTlsView<'a> {
            fn to_debug_string(&mut self, this: Resource<TlsError>) -> wasmtime::Result<String> {
                Ok(self.table.get(&this)?.to_string())
            }

            fn drop(&mut self, rep: Resource<TlsError>) -> wasmtime::Result<()> {
                self.table.delete(rep)?;
                Ok(())
            }
        }

        impl<'a> bindings::$pkg::tls::client::HostConnector for PluginTlsView<'a> {
            fn new(&mut self) -> wasmtime::Result<Resource<Connector>> {
                new_connector(self.table)
            }

            fn drop(&mut self, rep: Resource<Connector>) -> wasmtime::Result<()> {
                self.table.delete(rep)?;
                Ok(())
            }
        }

        impl<T: 'static> bindings::$pkg::tls::client::HostConnectorWithStore<T> for PluginTls {
            fn send(
                store: Access<'_, T, Self>,
                this: Resource<Connector>,
                cleartext: StreamReader<u8>,
            ) -> wasmtime::Result<(StreamReader<u8>, ResultFuture)> {
                send(store, this, cleartext)
            }

            fn receive(
                store: Access<'_, T, Self>,
                this: Resource<Connector>,
                ciphertext: StreamReader<u8>,
            ) -> wasmtime::Result<(StreamReader<u8>, ResultFuture)> {
                receive(store, this, ciphertext)
            }

            async fn connect(
                accessor: &Accessor<T, Self>,
                this: Resource<Connector>,
                server_name: String,
            ) -> wasmtime::Result<Result<(), Resource<TlsError>>> {
                connect(accessor, this, server_name).await
            }
        }
    };
}

impl_tls_host!(wasmcloud);
impl_tls_host!(wasi);

struct FnTask<F>(F);

impl<F, Fut, T, D> AccessorTask<T, D> for FnTask<F>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = wasmtime::Result<()>> + Send + 'static,
    D: HasData + ?Sized,
{
    fn run(self, _accessor: &Accessor<T, D>) -> impl Future<Output = wasmtime::Result<()>> + Send {
        self.0()
    }
}

type Getter<T> = for<'a> fn(&'a mut T) -> PluginTlsView<'a>;

/// The `future<result<_, error>>` beside each stream: resolves once both of
/// its directions have ended, with the first failure if either failed.
struct ResultProducer<T> {
    result: oneshot::Receiver<Result<(), TlsError>>,
    getter: Getter<T>,
}

impl<T> ResultProducer<T> {
    fn new(getter: Getter<T>, result: oneshot::Receiver<Result<(), TlsError>>) -> Self {
        Self { result, getter }
    }

    fn ready(getter: Getter<T>, result: Result<(), TlsError>) -> Self {
        let (tx, rx) = oneshot::channel();
        let _ = tx.send(result);
        Self { result: rx, getter }
    }
}

impl<T: 'static> FutureProducer<T> for ResultProducer<T> {
    type Item = Result<(), Resource<TlsError>>;

    fn poll_produce(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<T>,
        finish: bool,
    ) -> Poll<wasmtime::Result<Option<Self::Item>>> {
        match Pin::new(&mut self.result).poll(cx) {
            Poll::Ready(Ok(Ok(()))) => Poll::Ready(Ok(Some(Ok(())))),
            Poll::Ready(Ok(Err(err))) => {
                let err = (self.getter)(store.data_mut()).table.push(err)?;
                Poll::Ready(Ok(Some(Err(err))))
            }
            Poll::Ready(Err(_)) => Poll::Ready(Err(wasmtime::format_err!(
                "TLS stream task ended without a result"
            ))),
            Poll::Pending if finish => Poll::Ready(Ok(None)),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    fn pending() -> (Unresolved, Shared<Deferred<Session>>) {
        let shared = Shared::new(Deferred::pending());
        (Unresolved(shared.clone()), shared)
    }

    /// A connector dropped before `connect`, or a `connect` cancelled
    /// mid-handshake, must not leave its stream tasks parked on the session.
    #[tokio::test]
    async fn an_abandoned_session_closes() {
        let (guard, mut session) = pending();
        let reader = tokio::spawn(async move {
            let mut buf = [0u8; 4];
            session.read(&mut buf).await
        });
        tokio::task::yield_now().await;
        drop(guard);
        let err = reader.await.unwrap().unwrap_err().to_string();
        assert!(err.contains("abandoned"), "got: {err}");
    }

    #[tokio::test]
    async fn the_first_resolution_stands() {
        let (guard, mut session) = pending();
        let (mut peer, io) = tokio::io::duplex(64);
        guard.resolve(Box::new(io));
        session
            .lock()
            .resolve(Box::new(Closed(TlsError::msg("late"))));
        session.write_all(b"ok").await.unwrap();
        let mut buf = [0u8; 2];
        peer.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ok");
    }
}
