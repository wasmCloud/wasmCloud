//! A host component plugin that listens on a TCP port and echoes.
//!
//! Its `wasi:cli/run` export binds `127.0.0.1:ECHO_PORT` inside the plugin's own
//! private virtual loopback and serves an accept loop there forever. Nothing can
//! reach that listener until the host publishes a real port that splices into
//! it — which is exactly what the integration test does.
//!
//! The accept loop is **awaited from `cli/run`**, not spawned. A loopback accept
//! stream only delivers when it is driven from the run task, so spawning it
//! produces a plugin that binds successfully and then silently never accepts.

mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::acme::echo::control::Guest as ControlGuest;
use bindings::exports::wasi::cli::run::Guest as RunGuest;
use bindings::wasi::sockets::types::{
    IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket,
};
use wit_bindgen::StreamResult;

/// The port the plugin binds inside its own virtual network. Private to this
/// plugin, so it cannot collide with anything else on the host — the real port
/// the outside world sees is chosen by the operator, not here.
const ECHO_PORT: u16 = 50051;

struct Component;

/// Echo one accepted connection until the peer closes.
async fn echo_one(sock: TcpSocket) {
    let (mut incoming, incoming_done) = sock.receive();
    let (mut outgoing_tx, outgoing_rx) = bindings::wit_stream::new();

    futures::join!(
        async {
            // Drives the send side for as long as the peer keeps writing.
            let _ = sock.send(outgoing_rx).await;
        },
        async {
            loop {
                let (result, data) = incoming.read(Vec::with_capacity(8192)).await;
                match result {
                    StreamResult::Complete(n) if n > 0 => {
                        outgoing_tx.write_all(data).await;
                    }
                    // Peer half-closed or the connection ended.
                    _ => break,
                }
            }
            drop(outgoing_tx);
        }
    );
    let _ = incoming_done.await;
}

impl RunGuest for Component {
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

        // Awaited here, in the run task. Spawning this loop instead would bind
        // the port and then never deliver a connection.
        while let Some(sock) = accept.next().await {
            echo_one(sock).await;
        }
        Ok(())
    }
}

impl ControlGuest for Component {
    async fn listening_port() -> u16 {
        ECHO_PORT
    }
}

bindings::export!(Component with_types_in bindings);
