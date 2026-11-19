//! TLS echo exchange shared by the workload and host-plugin fixtures.

use super::bindings::wasi::sockets::types::{
    IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket,
};
use super::bindings::wasi::tls::client::Connector;
use super::bindings::wit_stream;

pub async fn round_trip(addr: &str) -> Result<(), ()> {
    // Name lookup is not granted to these guests, so the address is an IPv4 literal.
    let (host, port) = addr.rsplit_once(':').ok_or(())?;
    let port = port.parse().map_err(|_| ())?;
    let octets = host.parse::<std::net::Ipv4Addr>().map_err(|_| ())?.octets();
    let [a, b, c, d] = octets;

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
