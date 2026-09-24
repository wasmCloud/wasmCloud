//! Host component plugin that runs client TLS over its own `wasi:sockets`
//! connection, through `wasmcloud:tls/client` or the upstream `wasi:tls/client`
//! it mirrors — one signature set, so one body serves both — and over a
//! host-owned connection through `wasmcloud:tls/dialer`.

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
    async fn raw_denied() -> bool {
        use bindings::wasi::sockets::types::UdpSocket;
        TcpSocket::create(IpAddressFamily::Ipv4).is_err()
            && TcpSocket::create(IpAddressFamily::Ipv6).is_err()
            && UdpSocket::create(IpAddressFamily::Ipv4).is_err()
            && UdpSocket::create(IpAddressFamily::Ipv6).is_err()
    }

    async fn dial(endpoint: String) -> String {
        let conn = match bindings::wasmcloud::tls::dialer::connect(endpoint).await {
            Ok(conn) => conn,
            Err(err) => return format!("error: {}", err.to_debug_string()),
        };
        let (rx, received) = conn.receive();
        let (mut tx, data) = wit_stream::new();
        let sent = conn.send(data);
        let mut payload = vec![b'x'; PAYLOAD];
        payload.extend_from_slice(b"\r\n");
        let write = async move {
            let unwritten = tx.write_all(payload).await;
            drop(tx);
            unwritten.is_empty()
        };
        let (written, reply) = futures::join!(write, rx.collect());
        if !written {
            return "error: write".to_string();
        }
        if let Err(e) = sent.await {
            return format!("error: send: {}", e.to_debug_string());
        }
        if let Err(e) = received.await {
            return format!("error: receive: {}", e.to_debug_string());
        }
        String::from_utf8_lossy(&reply).into_owned()
    }

    async fn http_get(endpoint: String, grpc: bool) -> String {
        use bindings::wasi::http::{
            client,
            types::{Fields, Request, Scheme},
        };
        let (scheme, authority) = match endpoint.split_once("://") {
            Some(("https", authority)) => (Scheme::Https, authority),
            Some(("http", authority)) => (Scheme::Http, authority),
            _ => return "error: endpoint".to_string(),
        };
        let (tx, trailers) = bindings::wit_future::new(|| Ok(None));
        wit_bindgen::spawn_local(async move {
            let _ = tx.write(Ok(None)).await;
        });
        let headers = Fields::new();
        if grpc {
            let _ = headers.set("content-type", &[b"application/grpc".to_vec()]);
        }
        let (request, _sent) = Request::new(headers, None, trailers, None);
        let _ = request.set_scheme(Some(&scheme));
        let _ = request.set_authority(Some(authority));
        let _ = request.set_path_with_query(Some("/"));
        match client::send(request).await {
            Ok(response) => response.get_status_code().to_string(),
            Err(err) => format!("error: {err:?}"),
        }
    }

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
