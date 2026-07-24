//! Tick-free p3 echo service over the virtualized loopback.
//!
//! `cli/run` binds 127.0.0.1:9099, accepts connections, and spawns a handler
//! per connection; the reply travels handler -> spawned writer task through a
//! pure guest-side waker handoff (no host waitable involved), and the writer
//! drives the outbound stream — the same shapes as a connection pooler's
//! checkout wakers and per-session stream drivers. There is NO periodic clock
//! tick: if either a spawned task or a guest-waker wake needs an unrelated
//! host event to be observed, the reply never flushes.

mod bindings {
    wit_bindgen::generate!({ world: "svc-tcp-echo", generate_all });
}

use std::cell::RefCell;
use std::rc::Rc;
use std::task::{Poll, Waker};

use bindings::exports::wasi::cli::run::Guest;
use bindings::wasi::sockets::types::{
    IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket,
};

const ECHO_PORT: u16 = 9099;

/// A single-value handoff between two tasks on this instance, woken purely by
/// a guest-side [`Waker`] — the wake class that historically needed an
/// unrelated host event to be observed.
#[derive(Default)]
struct Handoff {
    value: RefCell<Option<Vec<u8>>>,
    waker: RefCell<Option<Waker>>,
}

impl Handoff {
    fn put(&self, value: Vec<u8>) {
        *self.value.borrow_mut() = Some(value);
        if let Some(waker) = self.waker.borrow_mut().take() {
            waker.wake();
        }
    }

    async fn take(&self) -> Vec<u8> {
        std::future::poll_fn(|cx| {
            if let Some(value) = self.value.borrow_mut().take() {
                Poll::Ready(value)
            } else {
                *self.waker.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await
    }
}

struct Component;

impl Guest for Component {
    async fn run() -> Result<(), ()> {
        let listener = TcpSocket::create(IpAddressFamily::Ipv4).map_err(|_| ())?;
        listener
            .bind(IpSocketAddress::Ipv4(Ipv4SocketAddress {
                port: ECHO_PORT,
                address: (127, 0, 0, 1),
            }))
            .map_err(|_| ())?;
        listener.set_listen_backlog_size(16).map_err(|_| ())?;
        let mut accept = listener.listen().map_err(|_| ())?;

        while let Some(sock) = accept.next().await {
            wit_bindgen::spawn_local(handle(sock));
        }
        Ok(())
    }
}

async fn handle(sock: TcpSocket) {
    // Take the receive side first; the socket itself moves into the writer
    // task, which owns the send side and waits on the handoff — a pure
    // guest-waker await with no host waitable.
    let (mut rx, _rx_result) = sock.receive();
    let handoff = Rc::new(Handoff::default());
    let (mut tx, tx_stream) = bindings::wit_stream::new();
    wit_bindgen::spawn_local({
        let handoff = Rc::clone(&handoff);
        async move {
            let reply = handoff.take().await;
            let _ = futures::join!(async { sock.send(tx_stream).await }, async {
                tx.write_all(reply).await;
                drop(tx);
            });
        }
    });

    let (_result, data) = rx.read(Vec::with_capacity(1024)).await;
    let mut reply = b"echo:".to_vec();
    reply.extend_from_slice(&data);
    handoff.put(reply);
}

bindings::export!(Component with_types_in bindings);
