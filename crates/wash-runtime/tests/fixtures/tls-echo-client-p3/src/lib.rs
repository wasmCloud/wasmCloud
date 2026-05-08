mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::cli::run::Guest;
use bindings::wasi::sockets::types::{IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket};
use bindings::wasi::tls::client::Connector;
use bindings::wit_stream;

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
    async fn run() -> Result<(), ()> {
        let ([a, b, c, d], port) = echo_addr();

        let sock = TcpSocket::create(IpAddressFamily::Ipv4).map_err(|_| ())?;
        sock.connect(IpSocketAddress::Ipv4(Ipv4SocketAddress {
            port,
            address: (a, b, c, d),
        }))
        .await
        .map_err(|_| ())?;

        let (sock_rx, sock_rx_done) = sock.receive();
        let conn = Connector::new();

        // Wire up TLS decryption (sock → tls plaintext) and encryption (plaintext → sock).
        let (tls_rx, tls_rx_done) = conn.receive(sock_rx);
        let (mut data_tx, data_rx) = wit_stream::new();
        let (tls_tx, tls_tx_done) = conn.send(data_rx);
        let sock_tx_done = sock.send(tls_tx);

        Connector::connect(conn, "localhost".into())
            .await
            .map_err(|_| ())?;

        let remaining = data_tx.write_all(b"PING\r\n".to_vec()).await;
        assert!(remaining.is_empty());
        drop(data_tx);

        let response = tls_rx.collect().await;

        sock_rx_done.await.map_err(|_| ())?;
        sock_tx_done.await.map_err(|_| ())?;
        tls_rx_done.await.map_err(|_| ())?;
        tls_tx_done.await.map_err(|_| ())?;

        if response.starts_with(b"PONG\r\n") {
            Ok(())
        } else {
            Err(())
        }
    }
}

bindings::export!(Component with_types_in bindings);
