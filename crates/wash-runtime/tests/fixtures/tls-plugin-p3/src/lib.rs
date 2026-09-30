//! Host component plugin that runs client TLS over its own `wasi:sockets`
//! connection, through `wasmcloud:tls` or the upstream `wasi:tls` it mirrors.
//! The two packages have one signature set, so one body serves both.

mod bindings {
    wit_bindgen::generate!({
        world: "tls-plugin",
        generate_all,
    });
}

use bindings::exports::acme::tlsprobe::probe::Guest;
use bindings::wasi::sockets::types::{
    IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket,
};
use bindings::wit_stream;

struct Component;

fn parse_addr(addr: &str) -> Option<IpSocketAddress> {
    let (host, port) = addr.rsplit_once(':')?;
    let mut octets = host.split('.').map(|o| o.parse::<u8>());
    let address = (
        octets.next()?.ok()?,
        octets.next()?.ok()?,
        octets.next()?.ok()?,
        octets.next()?.ok()?,
    );
    Some(IpSocketAddress::Ipv4(Ipv4SocketAddress {
        port: port.parse().ok()?,
        address,
    }))
}

/// Far past every buffer between the guest and the server, so a server that
/// echoes as it reads pushes back on the guest's writes while they run.
const PAYLOAD: usize = 1024 * 1024;

macro_rules! ping_via {
    ($client:path, $sock:expr, $server_name:expr) => {{
        use $client as client;
        let sock = $sock;
        let (sock_rx, _sock_rx_done) = sock.receive();
        let conn = client::Connector::new();
        let (tls_rx, tls_rx_done) = conn.receive(sock_rx);
        let (mut data_tx, data_rx) = wit_stream::new();
        let (tls_tx, tls_tx_done) = conn.send(data_rx);
        let _sock_tx_done = sock.send(tls_tx);

        match client::Connector::connect(conn, $server_name).await {
            Ok(()) => {
                let mut payload = vec![b'x'; PAYLOAD];
                payload.extend_from_slice(b"\r\n");
                // Write and read at once: both directions under backpressure.
                let write = async move {
                    let unwritten = data_tx.write_all(payload).await;
                    drop(data_tx);
                    unwritten.is_empty()
                };
                let (written, reply) = futures::join!(write, tls_rx.collect());
                if !written {
                    return "error: write".to_string();
                }
                if let Err(e) = tls_tx_done.await {
                    return format!("error: send: {}", e.to_debug_string());
                }
                if let Err(e) = tls_rx_done.await {
                    return format!("error: receive: {}", e.to_debug_string());
                }
                String::from_utf8_lossy(&reply).into_owned()
            }
            Err(e) => format!("error: connect: {}", e.to_debug_string()),
        }
    }};
}

impl Guest for Component {
    async fn ping(addr: String, server_name: String, via_wasi: bool) -> String {
        let Some(addr) = parse_addr(&addr) else {
            return "error: addr".to_string();
        };
        let Ok(sock) = TcpSocket::create(IpAddressFamily::Ipv4) else {
            return "error: socket".to_string();
        };
        if let Err(e) = sock.connect(addr).await {
            return format!("error: tcp connect: {e:?}");
        }
        if via_wasi {
            ping_via!(bindings::wasi::tls::client, sock, server_name)
        } else {
            ping_via!(bindings::wasmcloud::tls::client, sock, server_name)
        }
    }
}

bindings::export!(Component with_types_in bindings);
