//! Resolve a name, then connect a raw TCP socket to what it resolved to.
//!
//! Request path is `/<name>/<port>`. The status code reports what the host's
//! policy decided:
//!
//! - `200` connected
//! - `404` the name did not resolve
//! - `403` resolution or connection was refused by policy (`access-denied`)
//! - `502` the connection failed for some other reason, e.g. nothing listening
//!
//! `403` versus `502` is the distinction that matters: reaching a permitted
//! port with nothing behind it must look different from being refused, or a
//! policy bug is indistinguishable from a missing service.

mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasi::sockets::ip_name_lookup::{ErrorCode as ResolveErrorCode, resolve_addresses};
use bindings::wasi::sockets::types::{
    ErrorCode as SocketErrorCode, IpAddress, IpAddressFamily, IpSocketAddress, Ipv4SocketAddress,
    Ipv6SocketAddress, TcpSocket,
};

struct Component;

fn socket_address(addr: IpAddress, port: u16) -> (IpSocketAddress, IpAddressFamily) {
    match addr {
        IpAddress::Ipv4(address) => (
            IpSocketAddress::Ipv4(Ipv4SocketAddress { port, address }),
            IpAddressFamily::Ipv4,
        ),
        IpAddress::Ipv6(address) => (
            IpSocketAddress::Ipv6(Ipv6SocketAddress {
                port,
                address,
                flow_info: 0,
                scope_id: 0,
            }),
            IpAddressFamily::Ipv6,
        ),
    }
}

async fn probe(name: &str, port: u16) -> (u16, String) {
    let addrs = match resolve_addresses(name.to_string()).await {
        Ok(addrs) => addrs,
        Err(ResolveErrorCode::PermanentResolverFailure) => {
            return (403, format!("{name}: resolution denied by policy"));
        }
        Err(ResolveErrorCode::NameUnresolvable) => {
            return (404, format!("{name}: name unresolvable"));
        }
        Err(e) => return (502, format!("{name}: resolution failed: {e:?}")),
    };
    let Some(first) = addrs.into_iter().next() else {
        return (404, format!("{name}: resolved to no addresses"));
    };

    let (addr, family) = socket_address(first, port);
    let socket = match TcpSocket::create(family) {
        Ok(socket) => socket,
        Err(e) => return (502, format!("{name}: socket create failed: {e:?}")),
    };
    match socket.connect(addr).await {
        Ok(()) => (200, format!("{name}:{port}: connected")),
        Err(SocketErrorCode::AccessDenied) => {
            (403, format!("{name}:{port}: connection denied by policy"))
        }
        Err(e) => (502, format!("{name}:{port}: connect failed: {e:?}")),
    }
}

impl Handler for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let path = request.get_path_with_query().unwrap_or_default();
        let mut parts = path.trim_start_matches('/').splitn(2, '/');
        let name = parts.next().unwrap_or_default().to_string();
        let port: u16 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(80);

        let (status, body) = probe(&name, port).await;

        let (mut tx, rx) = bindings::wit_stream::new();
        let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());

        wit_bindgen::spawn_local(async move {
            tx.write_all(body.into_bytes()).await;
            drop(tx);
            let _ = trailers_tx.write(Ok(None)).await;
        });

        let (response, _result) = Response::new(Fields::new(), Some(rx), trailers_rx);
        response
            .set_status_code(status)
            .map_err(|()| ErrorCode::InternalError(Some("failed to set status code".to_string())))?;
        Ok(response)
    }
}

bindings::export!(Component with_types_in bindings);
