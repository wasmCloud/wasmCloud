wit_bindgen::generate!({
    world: "tls-echo-client",
    generate_all,
    features: ["tls"],
});

use exports::wasi::cli::run::Guest;
use wasi::io::poll::poll;
use wasi::io::streams::StreamError;
use wasi::sockets::instance_network::instance_network;
use wasi::sockets::network::{ErrorCode, IpAddressFamily, IpSocketAddress, Ipv4SocketAddress};
use wasi::sockets::tcp_create_socket::create_tcp_socket;
use wasi::tls::types::ClientHandshake;

struct Component;

/// Parse "host:port" from `ECHO_ADDR`. The host portion must be an IPv4
/// literal, since `wasi:sockets/ip-name-lookup` is not granted to this guest.
fn echo_addr() -> ([u8; 4], u16) {
    let addr = std::env::var("ECHO_ADDR").expect("ECHO_ADDR must be set");
    let (host, port) = addr.rsplit_once(':').expect("ECHO_ADDR must be host:port");
    let port: u16 = port.parse().expect("port must be a number");
    let mut octs = host.split('.');
    let a: u8 = octs.next().unwrap().parse().unwrap();
    let b: u8 = octs.next().unwrap().parse().unwrap();
    let c: u8 = octs.next().unwrap().parse().unwrap();
    let d: u8 = octs.next().unwrap().parse().unwrap();
    ([a, b, c, d], port)
}

impl Guest for Component {
    fn run() -> Result<(), ()> {
        let ([a, b, c, d], port) = echo_addr();

        let net = instance_network();
        let sock = create_tcp_socket(IpAddressFamily::Ipv4).map_err(|_| ())?;
        let remote = IpSocketAddress::Ipv4(Ipv4SocketAddress {
            port,
            address: (a, b, c, d),
        });

        sock.start_connect(&net, remote).map_err(|_| ())?;
        let (rx, tx) = loop {
            let p = sock.subscribe();
            poll(&[&p]);
            match sock.finish_connect() {
                Ok(streams) => break streams,
                Err(ErrorCode::WouldBlock) => continue,
                Err(_) => return Err(()),
            }
        };

        let handshake = ClientHandshake::new("localhost", rx, tx);
        let fut = ClientHandshake::finish(handshake);
        let (conn, tls_rx, tls_tx) = loop {
            let p = fut.subscribe();
            poll(&[&p]);
            if let Some(outer) = fut.get() {
                match outer {
                    Ok(Ok((c, rx, tx))) => break (c, rx, tx),
                    _ => return Err(()),
                }
            }
        };

        // Send "PING\r\n" and close the outgoing side.
        let p = tls_tx.subscribe();
        poll(&[&p]);
        tls_tx.write(b"PING\r\n").map_err(|_| ())?;
        tls_tx.flush().map_err(|_| ())?;
        conn.close_output();

        // Read until we have at least "PONG\r\n" or the stream closes.
        let mut response = Vec::new();
        loop {
            let p = tls_rx.subscribe();
            poll(&[&p]);
            match tls_rx.read(64) {
                Ok(bytes) => {
                    response.extend_from_slice(&bytes);
                    if response.len() >= 6 {
                        break;
                    }
                }
                Err(StreamError::Closed) => break,
                Err(_) => return Err(()),
            }
        }

        if response.starts_with(b"PONG\r\n") {
            Ok(())
        } else {
            Err(())
        }
    }
}

export!(Component);
