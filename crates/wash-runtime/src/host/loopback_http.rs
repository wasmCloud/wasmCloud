//! `wasi:http` over the reserved `*.wasmcloud.internal` names.
//!
//! Both names are answered entirely inside the host and never reach the
//! ordinary outgoing handler, which is what lets each carry its own policy:
//!
//! - **`service.wasmcloud.internal`** dials the workload's *virtual* loopback.
//!   This is new capability rather than a rewrite: `wasi:http` to
//!   `127.0.0.1:8080` reaches the machine, not the workload, so until now a
//!   component could not make an HTTP call to its own service at all.
//! - **`host.wasmcloud.internal`** dials the machine's real loopback, gated by
//!   the same `allowedHostLoopback` grant the sockets path uses.
//!
//! Both still require the name to appear in `allowedHosts` literally — `*`
//! grants the internet, not the machine or the workload's own insides. See
//! [`crate::host::http::check_allowed_hosts`].
//!
//! # The duplex adapter
//!
//! The virtual transport is a pair of mpsc channels carrying
//! `(Bytes, OwnedSemaphorePermit)`, not a socket. [`LoopbackStream`] adapts it
//! to `AsyncRead`/`AsyncWrite` so `hyper` can speak HTTP/1.1 over it, keeping
//! the permit discipline that bounds the channels: a write takes a permit
//! before queueing, and the permit is released when the peer consumes the
//! chunk.

use core::future::Future;
use core::net::SocketAddr;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use bytes::Bytes;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore, mpsc};

use crate::sockets::loopback;

/// How many chunks may be queued toward the peer before a write waits.
///
/// The channels underneath are unbounded; this semaphore is the only thing
/// bounding them, exactly as it is for a guest-to-guest connection.
const MAX_INFLIGHT_CHUNKS: usize = 16;

/// An in-progress permit acquisition, polled in place rather than spawned.
type Acquiring = Pin<Box<dyn Future<Output = Result<OwnedSemaphorePermit, AcquireError>> + Send>>;

/// An `AsyncRead`/`AsyncWrite` view of one side of a virtual connection.
pub struct LoopbackStream {
    rx: mpsc::UnboundedReceiver<(Bytes, OwnedSemaphorePermit)>,
    tx: Option<mpsc::UnboundedSender<(Bytes, OwnedSemaphorePermit)>>,
    permits: Arc<Semaphore>,
    /// Remainder of a chunk a previous `poll_read` could not fit.
    pending: Bytes,
    /// A permit this side is waiting on, resumed on the next `poll_write`.
    acquiring: Option<Acquiring>,
}

impl LoopbackStream {
    fn new(conn: loopback::TcpConn) -> Result<Self> {
        let loopback::TcpConn { rx, tx, .. } = conn;
        let Some(rx) = rx else {
            bail!("virtual connection was created without a receiver");
        };
        let Some(tx) = tx else {
            bail!("virtual connection was created without a sender");
        };
        Ok(Self {
            rx,
            tx: Some(tx),
            permits: Arc::new(Semaphore::new(MAX_INFLIGHT_CHUNKS)),
            pending: Bytes::new(),
            acquiring: None,
        })
    }
}

impl AsyncRead for LoopbackStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        // Returning having filled nothing is how this trait spells EOF, so a
        // caller that left no room would be told the peer hung up and would
        // silently truncate the response. Name what actually happened.
        if buf.remaining() == 0 {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "read buffer has no remaining capacity",
            )));
        }
        loop {
            // Drain whatever a previous read left over before taking a new
            // chunk, so a caller with a small buffer still makes progress.
            if !self.pending.is_empty() {
                let take = self.pending.len().min(buf.remaining());
                let head = self.pending.split_to(take);
                buf.put_slice(&head);
                return Poll::Ready(Ok(()));
            }
            match self.rx.poll_recv(cx) {
                Poll::Ready(Some((chunk, permit))) => {
                    // Dropping the permit here returns the peer's capacity as
                    // soon as the bytes are ours, which is the point at which
                    // they stop occupying the channel.
                    drop(permit);
                    // An empty chunk would fill nothing, which the caller reads
                    // as EOF; go round for one that carries bytes.
                    self.pending = chunk;
                }
                // Peer closed: EOF.
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl AsyncWrite for LoopbackStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        // Take a permit before queueing, so a peer that stops reading stops
        // this side writing rather than growing the channel without limit.
        // Holding the acquisition future across polls is what registers the
        // waker: the semaphore wakes this task when the peer's read frees a
        // permit, so a full channel costs nothing while it waits.
        let mut acquire = match self.acquiring.take() {
            Some(fut) => fut,
            None => Box::pin(Arc::clone(&self.permits).acquire_owned()),
        };
        let permit = match acquire.as_mut().poll(cx) {
            Poll::Ready(Ok(permit)) => permit,
            // The semaphore only closes when the stream is being torn down.
            Poll::Ready(Err(_)) => {
                return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
            }
            Poll::Pending => {
                self.acquiring = Some(acquire);
                return Poll::Pending;
            }
        };
        let Some(tx) = self.tx.as_ref() else {
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        };
        if tx.send((Bytes::copy_from_slice(data), permit)).is_err() {
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        Poll::Ready(Ok(data.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        // Every write is queued synchronously; there is no buffer to flush.
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        // Dropping the sender is what the peer observes as EOF.
        self.tx = None;
        Poll::Ready(Ok(()))
    }
}

/// Open a connection to `target` inside `network`.
///
/// Mirrors what an external client gets through a published port: the pair is
/// built the same way, so the accepting guest sees a `remote_address` it can
/// reason about rather than a synthetic one.
///
/// # Errors
///
/// Fails if nothing is listening at `target` — which, for
/// `service.wasmcloud.internal`, means the workload has no service or its
/// service has not bound that port yet.
pub fn connect(
    network: &Mutex<loopback::Network>,
    from: SocketAddr,
    target: SocketAddr,
) -> Result<LoopbackStream> {
    let accept_tx = {
        let mut net = network
            .lock()
            .map_err(|e| anyhow::anyhow!("loopback network lock poisoned: {e}"))?;
        match net.connect_tcp(&target) {
            Ok(tx) => tx.clone(),
            Err(_) => bail!(
                "nothing is listening on {target} in this workload's virtual network; \
                 service.wasmcloud.internal reaches the workload's own service, which must be \
                 running and bound to that port"
            ),
        }
    };
    let (client, server) = loopback::TcpConn::pair(from, target);
    accept_tx
        .try_send(server)
        .map_err(|_| anyhow::anyhow!("virtual listener on {target} is not accepting"))?;
    LoopbackStream::new(client)
}

/// Where a request to a reserved name should go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalRoute {
    /// The workload's own service, inside its virtual network.
    Service { target: SocketAddr },
    /// The machine the host process runs on.
    HostLoopback { target: SocketAddr },
}

/// Classify an outgoing request's authority against the reserved zone.
///
/// Returns `None` for every ordinary name, which the caller passes on to the
/// normal outgoing handler. Returns `Some(Err(..))` for a reserved name the
/// policy refuses, so the caller reports that rather than silently falling
/// through to real egress.
///
/// The `allowedHosts` check is *not* done here — it runs in
/// [`check_allowed_hosts`](super::http::check_allowed_hosts), which already
/// requires a reserved name to be listed literally.
pub fn route(
    uri: &http::Uri,
    policy: &crate::sockets::policy::SocketPolicy,
) -> Option<Result<InternalRoute>> {
    use crate::host::allowed_loopback::check_allowed_loopback;
    use crate::host::declared_port::Protocol;
    use crate::sockets::internal_names::{self, InternalName};

    let host = uri.host()?;
    let internal = internal_names::resolve(host)?;
    let Ok(internal) = internal else {
        return Some(Err(anyhow::anyhow!(
            "'{host}' is inside the reserved wasmcloud.internal zone but names nothing"
        )));
    };
    // Default by scheme, the way any HTTP client would.
    let port = uri.port_u16().unwrap_or(match uri.scheme_str() {
        Some("https") => 443,
        _ => 80,
    });
    let target = SocketAddr::new(core::net::IpAddr::V4(core::net::Ipv4Addr::LOCALHOST), port);

    Some(match internal {
        InternalName::Service => Ok(InternalRoute::Service { target }),
        InternalName::Host => {
            if !policy.host_loopback_enabled {
                Err(anyhow::anyhow!(
                    "reaching host.wasmcloud.internal needs the host to run with \
                     --allow-host-loopback"
                ))
            } else if !check_allowed_loopback(&policy.host_loopback, target, Protocol::Tcp) {
                Err(anyhow::anyhow!(
                    "reaching host.wasmcloud.internal:{port} needs '{port}' in this workload's \
                     allowedHostLoopback"
                ))
            } else if policy
                .host_owned_ports
                .as_ref()
                .is_some_and(|table| table.is_published(Protocol::Tcp, target))
            {
                // The same refusal the sockets path makes, for the same reason:
                // a port this host published is its own ingress or another
                // tenant's service. Checked after the allowlist so an operator
                // cannot grant it by listing the port — and checked here too,
                // because a policy enforced on only one of the two APIs that
                // share it is not enforced at all.
                Err(anyhow::anyhow!(
                    "host.wasmcloud.internal:{port} is a port this host published; it is not \
                     reachable from a workload even when granted"
                ))
            } else {
                Ok(InternalRoute::HostLoopback { target })
            }
        }
    })
}

/// One transport or the other, so the HTTP client above does not care which.
pub enum InternalStream {
    Virtual(LoopbackStream),
    Host(tokio::net::TcpStream),
}

impl AsyncRead for InternalStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Virtual(s) => Pin::new(s).poll_read(cx, buf),
            Self::Host(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for InternalStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Virtual(s) => Pin::new(s).poll_write(cx, data),
            Self::Host(s) => Pin::new(s).poll_write(cx, data),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Virtual(s) => Pin::new(s).poll_flush(cx),
            Self::Host(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Virtual(s) => Pin::new(s).poll_shutdown(cx),
            Self::Host(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// Open the transport a route calls for.
///
/// # Errors
///
/// Fails if the workload has no virtual network to dial (it has no service),
/// nothing is listening there, or the real loopback refuses the connection.
pub async fn open(
    route: InternalRoute,
    network: &Arc<Mutex<loopback::Network>>,
) -> Result<InternalStream> {
    match route {
        InternalRoute::Service { target } => {
            // Port 0 as the local address: nothing dials this side back, and
            // the virtual network keys endpoints by port only.
            let from = SocketAddr::new(core::net::IpAddr::V4(core::net::Ipv4Addr::LOCALHOST), 0);
            connect(network, from, target).map(InternalStream::Virtual)
        }
        InternalRoute::HostLoopback { target } => {
            let stream = tokio::net::TcpStream::connect(target).await?;
            // Request/response traffic dominates here and the guest never sees
            // the real socket, so it cannot set this itself.
            let _ = stream.set_nodelay(true);
            Ok(InternalStream::Host(stream))
        }
    }
}

/// Send a P2 request over the transport `route` calls for.
///
/// HTTP/1.1 only. The virtual transport has no ALPN and no TLS to negotiate h2
/// over, and the host-loopback case is a plaintext local hop, so there is
/// nothing on either side that would select h2.
///
/// # Errors
///
/// Fails if the transport cannot be opened or the peer does not speak HTTP.
pub async fn send_p2(
    route: InternalRoute,
    network: &Arc<Mutex<loopback::Network>>,
    request: hyper::Request<wasmtime_wasi_http::p2::body::HyperOutgoingBody>,
    config: wasmtime_wasi_http::p2::types::OutgoingRequestConfig,
) -> Result<wasmtime_wasi_http::p2::types::IncomingResponse> {
    let stream = tokio::time::timeout(config.connect_timeout, open(route, network))
        .await
        .map_err(|_| anyhow::anyhow!("connecting to {route:?} timed out"))??;

    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
    // The connection task must keep running while the response body streams, so
    // it is spawned rather than awaited.
    let worker = wasmtime_wasi::runtime::spawn(async move {
        let _ = conn.await;
    });

    let response = tokio::time::timeout(config.first_byte_timeout, sender.send_request(request))
        .await
        .map_err(|_| {
            anyhow::anyhow!("no response from {route:?} within the first-byte timeout")
        })??;

    Ok(wasmtime_wasi_http::p2::types::IncomingResponse {
        resp: response.map(|body| {
            use http_body_util::BodyExt as _;
            body.map_err(|e| {
                wasmtime_wasi_http::p2::bindings::http::types::ErrorCode::InternalError(Some(
                    e.to_string(),
                ))
            })
            .boxed_unsync()
        }),
        worker: Some(worker),
        between_bytes_timeout: config.between_bytes_timeout,
    })
}

/// Send a P3 request over the transport `route` calls for. See [`send_p2`].
///
/// # Errors
///
/// Fails if the transport cannot be opened or the peer does not speak HTTP.
pub async fn send_p3(
    route: InternalRoute,
    network: &Arc<Mutex<loopback::Network>>,
    request: hyper::Request<crate::host::http_p3::P3Body>,
) -> Result<hyper::Response<crate::host::http_p3::P3Body>> {
    let stream = open(route, network).await?;
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
    wasmtime_wasi::runtime::spawn(async move {
        let _ = conn.await;
    });
    let response = sender.send_request(request).await?;
    Ok(response.map(|body| {
        use http_body_util::BodyExt as _;
        body.map_err(|e| {
            wasmtime_wasi_http::p3::bindings::http::types::ErrorCode::InternalError(Some(
                e.to_string(),
            ))
        })
        .boxed_unsync()
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    /// Register a listener the way a guest's `listen` does.
    fn listen(
        network: &Arc<Mutex<loopback::Network>>,
        target: SocketAddr,
    ) -> mpsc::Receiver<loopback::TcpConn> {
        let mut net = network.lock().unwrap();
        net.bind_tcp(target).unwrap();
        let (tx, rx) = mpsc::channel(4);
        net.get_tcp_net_mut(target.ip()).insert(
            target.port().try_into().unwrap(),
            loopback::TcpEndpoint::Listening(tx),
        );
        rx
    }

    /// `wasi:http` and `wasi:sockets` share one policy object, so a grant must
    /// mean the same thing through both. A port the host published is the case
    /// where they can silently diverge: the sockets path refuses it after the
    /// allowlist, and an HTTP route that skipped that check would hand a
    /// workload another tenant's service.
    #[test]
    fn the_http_route_refuses_a_host_owned_port_like_the_sockets_path() {
        use crate::host::allowed_loopback::AllowedLoopbackPort;
        use crate::host::declared_port::Protocol;
        use crate::host::ports::{PortOwner, PortTable};
        use crate::sockets::SocketAddrUse;

        let table = PortTable::new();
        let published = addr("127.0.0.1:8080");
        let _held = table
            .reserve(
                Protocol::Tcp,
                published,
                PortOwner::Workload("other".into()),
            )
            .unwrap();

        // Granted the port explicitly — the operator cannot open this door.
        let policy = crate::sockets::policy::SocketPolicy {
            host_loopback_enabled: true,
            host_loopback: Arc::from([AllowedLoopbackPort::tcp(8080)]),
            host_owned_ports: Some(Arc::clone(&table)),
            allowed_hosts: Arc::from([crate::host::allowed_hosts::AllowedHost::Any]),
            egress_mode: crate::sockets::policy::EgressMode::Enforce,
            ..crate::sockets::policy::SocketPolicy::for_kind(
                crate::sockets::policy::GuestKind::Component,
            )
        };

        let uri: http::Uri = "http://host.wasmcloud.internal:8080/".parse().unwrap();
        let err = route(&uri, &policy)
            .expect("the reserved zone answers this name")
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(err.contains("published"), "got: {err}");

        // The sockets path refuses it too — that agreement is the point.
        let sentinel = addr("127.255.255.254:8080");
        assert!(
            matches!(
                policy.decide(SocketAddrUse::TcpConnect, sentinel),
                crate::sockets::AddrDecision::Deny(crate::sockets::DenyReason::HostOwnedPort)
            ),
            "the sockets path must refuse the same port"
        );

        // A port the host did not publish stays reachable through both.
        let policy = crate::sockets::policy::SocketPolicy {
            host_loopback: Arc::from([AllowedLoopbackPort::tcp(5432)]),
            ..policy
        };
        let uri: http::Uri = "http://host.wasmcloud.internal:5432/".parse().unwrap();
        assert!(
            route(&uri, &policy).expect("answered").is_ok(),
            "an unpublished granted port is still reachable"
        );
    }

    #[tokio::test]
    async fn connecting_to_nothing_says_what_is_missing() {
        let network: Arc<Mutex<loopback::Network>> = Arc::default();
        let err = connect(&network, addr("127.0.0.1:0"), addr("127.0.0.1:8080"))
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(err.contains("nothing is listening"), "got: {err}");
    }

    #[tokio::test]
    async fn the_stream_round_trips_bytes_in_both_directions() {
        let network: Arc<Mutex<loopback::Network>> = Arc::default();
        let target = addr("127.0.0.1:8080");
        let mut accepts = listen(&network, target);

        let mut client = connect(&network, addr("127.0.0.1:1234"), target).unwrap();
        let accepted = accepts.recv().await.expect("connect should deliver");
        assert_eq!(accepted.local_address, target);
        assert_eq!(accepted.remote_address, addr("127.0.0.1:1234"));

        let mut server = LoopbackStream::new(accepted).unwrap();

        client.write_all(b"GET / HTTP/1.1").await.unwrap();
        let mut buf = vec![0u8; b"GET / HTTP/1.1".len()];
        server.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"GET / HTTP/1.1");

        server.write_all(b"HTTP/1.1 200 OK").await.unwrap();
        let mut buf = vec![0u8; b"HTTP/1.1 200 OK".len()];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"HTTP/1.1 200 OK");
    }

    /// A reader with a buffer smaller than the chunk must still make progress
    /// rather than dropping the remainder — hyper reads in fixed-size buffers.
    #[tokio::test]
    async fn a_partial_read_keeps_the_remainder() {
        let network: Arc<Mutex<loopback::Network>> = Arc::default();
        let target = addr("127.0.0.1:8081");
        let mut accepts = listen(&network, target);

        let mut client = connect(&network, addr("127.0.0.1:1234"), target).unwrap();
        let mut server = LoopbackStream::new(accepts.recv().await.unwrap()).unwrap();

        client.write_all(b"abcdefghij").await.unwrap();
        let mut first = [0u8; 4];
        server.read_exact(&mut first).await.unwrap();
        assert_eq!(&first, b"abcd");
        let mut rest = [0u8; 6];
        server.read_exact(&mut rest).await.unwrap();
        assert_eq!(&rest, b"efghij");
    }

    /// A zero-length chunk carries no bytes, and filling nothing is how this
    /// trait spells EOF — so passing one through would truncate the response at
    /// whatever point the peer happened to send one.
    #[tokio::test]
    async fn an_empty_chunk_is_not_end_of_stream() {
        let network: Arc<Mutex<loopback::Network>> = Arc::default();
        let target = addr("127.0.0.1:8083");
        let mut accepts = listen(&network, target);

        let mut client = connect(&network, addr("127.0.0.1:1234"), target).unwrap();
        let accepted = accepts.recv().await.unwrap();
        let tx = accepted.tx.clone().expect("accepted side has a sender");

        // Queue an empty chunk ahead of real bytes, the way a guest writing a
        // zero-length body frame does.
        let permits = Arc::new(Semaphore::new(4));
        let empty = Arc::clone(&permits).try_acquire_owned().unwrap();
        let real = Arc::clone(&permits).try_acquire_owned().unwrap();
        tx.send((Bytes::new(), empty)).unwrap();
        tx.send((Bytes::from_static(b"body"), real)).unwrap();

        let mut got = [0u8; 4];
        client.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"body");
    }

    /// Filling nothing means EOF, so a caller that left no room must be told it
    /// asked for the impossible rather than that the peer hung up.
    #[tokio::test]
    async fn a_read_with_no_room_is_an_error_not_eof() {
        let network: Arc<Mutex<loopback::Network>> = Arc::default();
        let target = addr("127.0.0.1:8084");
        let mut accepts = listen(&network, target);

        let mut client = connect(&network, addr("127.0.0.1:1234"), target).unwrap();
        let _accepted = accepts.recv().await.unwrap();

        let err = client.read_exact(&mut []).await;
        // `read_exact` of nothing is trivially satisfied without ever reading;
        // drive `poll_read` itself to reach the case.
        assert!(err.is_ok());
        let waker = std::task::Waker::noop();
        let mut cx = Context::from_waker(waker);
        let mut empty = ReadBuf::new(&mut []);
        match Pin::new(&mut client).poll_read(&mut cx, &mut empty) {
            Poll::Ready(Err(err)) => {
                assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    /// A write that has to wait for the peer must *park* on the semaphore. A
    /// self-wake would turn backpressure into a spin, burning a core for as
    /// long as the peer is slow — and the slow peer is the case this exists for.
    #[tokio::test]
    async fn a_blocked_write_parks_instead_of_spinning() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::task::Wake;

        struct Counting(AtomicUsize);
        impl Wake for Counting {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
            fn wake_by_ref(self: &Arc<Self>) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let network: Arc<Mutex<loopback::Network>> = Arc::default();
        let target = addr("127.0.0.1:8085");
        let mut accepts = listen(&network, target);

        let mut client = connect(&network, addr("127.0.0.1:1234"), target).unwrap();
        let mut server = LoopbackStream::new(accepts.recv().await.unwrap()).unwrap();

        // Fill the channel: the peer has not read, so every permit is out.
        for _ in 0..MAX_INFLIGHT_CHUNKS {
            client.write_all(b"x").await.unwrap();
        }

        let counter = Arc::new(Counting(AtomicUsize::new(0)));
        let waker = std::task::Waker::from(Arc::clone(&counter));
        let mut cx = Context::from_waker(&waker);

        // The next write has nowhere to put its bytes.
        for _ in 0..3 {
            assert!(
                Pin::new(&mut client).poll_write(&mut cx, b"y").is_pending(),
                "the channel is full, so the write cannot complete"
            );
            assert_eq!(
                counter.0.load(Ordering::SeqCst),
                0,
                "a blocked write must not wake itself"
            );
        }

        // The peer reading is what frees a permit, and the semaphore is what
        // turns that into a wakeup.
        let mut buf = [0u8; 1];
        server.read_exact(&mut buf).await.unwrap();
        tokio::task::yield_now().await;
        assert!(
            counter.0.load(Ordering::SeqCst) >= 1,
            "freeing a permit must wake the parked writer"
        );
        assert!(matches!(
            Pin::new(&mut client).poll_write(&mut cx, b"y"),
            Poll::Ready(Ok(1))
        ));
    }

    /// The adapter exists to carry HTTP, so the test that matters is a real
    /// `hyper` client and server talking across it — chunked framing, header
    /// parsing, connection shutdown and all.
    #[tokio::test]
    async fn hyper_speaks_http_across_the_virtual_transport() {
        use http_body_util::{BodyExt as _, Full};

        let network: Arc<Mutex<loopback::Network>> = Arc::default();
        let target = addr("127.0.0.1:8080");
        let mut accepts = listen(&network, target);

        let client_stream = connect(&network, addr("127.0.0.1:1234"), target).unwrap();
        let server_stream = LoopbackStream::new(accepts.recv().await.unwrap()).unwrap();

        // Serve one request on the accepted side.
        tokio::spawn(async move {
            let service = hyper::service::service_fn(
                |req: hyper::Request<hyper::body::Incoming>| async move {
                    let body = req.into_body().collect().await.unwrap().to_bytes();
                    let echoed = format!("service saw {} bytes", body.len());
                    Ok::<_, std::convert::Infallible>(hyper::Response::new(Full::new(Bytes::from(
                        echoed,
                    ))))
                },
            );
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(server_stream), service)
                .await;
        });

        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(client_stream))
            .await
            .unwrap();
        tokio::spawn(async move {
            let _ = conn.await;
        });

        let request = hyper::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "service.wasmcloud.internal")
            .body(Full::new(Bytes::from_static(b"hello service")))
            .unwrap();
        let response = sender.send_request(request).await.unwrap();
        assert_eq!(response.status(), 200);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], b"service saw 13 bytes");
    }

    #[tokio::test]
    async fn shutdown_is_observed_as_eof() {
        let network: Arc<Mutex<loopback::Network>> = Arc::default();
        let target = addr("127.0.0.1:8082");
        let mut accepts = listen(&network, target);

        let mut client = connect(&network, addr("127.0.0.1:1234"), target).unwrap();
        let mut server = LoopbackStream::new(accepts.recv().await.unwrap()).unwrap();

        client.write_all(b"bye").await.unwrap();
        client.shutdown().await.unwrap();
        drop(client);

        let mut got = Vec::new();
        server.read_to_end(&mut got).await.unwrap();
        assert_eq!(&got, b"bye");
    }
}
