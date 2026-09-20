//! A p3 HTTP server over virtual loopback.
//!
//! `cli/run` binds `127.0.0.1:8080` inside the workload's virtual loopback and
//! accepts incoming TCP connections. For each connection, it parses the HTTP/1.1
//! request and replies with "hello from p3". External traffic reaches this
//! listener when the host publishes a real port that splices into it.

mod bindings {
    wit_bindgen::generate!({
        world: "http-server-p3",
        generate_all,
    });
}

use bindings::exports::wasi::cli::run::Guest as RunGuest;
use bindings::wasi::sockets::types::{
    IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket,
};
use wit_bindgen::StreamResult;

pub const HTTP_PORT: u16 = 8080;
const BODY: &[u8] = b"hello from p3";

struct Component;

async fn handle_http(sock: TcpSocket) {
    let (mut incoming, incoming_done) = sock.receive();
    let (mut outgoing_tx, outgoing_rx) = bindings::wit_stream::new();

    futures::join!(
        async {
            let _ = sock.send(outgoing_rx).await;
        },
        async {
            // Read incoming HTTP request headers until "\r\n\r\n" or EOF
            let mut buf = Vec::new();
            loop {
                let (result, data) = incoming.read(Vec::with_capacity(1024)).await;
                match result {
                    StreamResult::Complete(n) if n > 0 => {
                        buf.extend_from_slice(&data);
                        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    _ => break,
                }
            }

            // Write HTTP/1.1 response matching `Flavor::P3.expected_body()`
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                BODY.len()
            );
            outgoing_tx.write_all(header.into_bytes()).await;
            outgoing_tx.write_all(BODY.to_vec()).await;
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
                port: HTTP_PORT,
                address: (127, 0, 0, 1),
            }))
            .map_err(|_| ())?;
        listener.set_listen_backlog_size(32).map_err(|_| ())?;
        let mut accept = listener.listen().map_err(|_| ())?;

        while let Some(sock) = accept.next().await {
            wit_bindgen::spawn_local(handle_http(sock));
        }
        Ok(())
    }
}

bindings::export!(Component with_types_in bindings);

