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

            let line_end = buf.iter().position(|&b| b == b'\n').unwrap_or(buf.len());
            let mut parts = buf[..line_end].split(|&b| b == b' ');
            let _method = parts.next();
            let path = parts.next().unwrap_or_default();
            let is_bulk = path.starts_with(b"/bulk");
            let is_stream = path.starts_with(b"/stream");

            if is_bulk {
                let header = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
                outgoing_tx.write_all(header.as_bytes().to_vec()).await;
                let chunk_data = vec![b'x'; 65536];
                let chunk_hdr = format!("{:x}\r\n", chunk_data.len()).into_bytes();
                for _ in 0..16 {
                    outgoing_tx.write_all(chunk_hdr.clone()).await;
                    outgoing_tx.write_all(chunk_data.clone()).await;
                    outgoing_tx.write_all(b"\r\n".to_vec()).await;
                }
                outgoing_tx.write_all(b"0\r\n\r\n".to_vec()).await;
                drop(outgoing_tx);
            } else if is_stream {
                let header = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
                outgoing_tx.write_all(header.as_bytes().to_vec()).await;
                let chunk = format!("{:x}\r\nhello from p3\r\n", BODY.len()).into_bytes();
                for _ in 0..500 {
                    outgoing_tx.write_all(chunk.clone()).await;
                }
                outgoing_tx.write_all(b"0\r\n\r\n".to_vec()).await;
                drop(outgoing_tx);
            } else {
                // Write HTTP/1.1 response matching `Flavor::P3.expected_body()`
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    BODY.len()
                );
                outgoing_tx.write_all(header.into_bytes()).await;
                outgoing_tx.write_all(BODY.to_vec()).await;
                drop(outgoing_tx);
            }
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

