mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasi::sockets::ip_name_lookup::{resolve_addresses, ErrorCode as ResolveErrorCode};
use bindings::wasi::sockets::types::{
    ErrorCode as SocketErrorCode, IpAddressFamily, IpSocketAddress, Ipv4SocketAddress,
    Ipv6SocketAddress, TcpSocket,
};

// The fixture reports the host-side ip-name-lookup policy decision via its
// status code:
//
// - 200 OK          : lookup permitted; body is the number of addresses found
// - 403 Forbidden   : denied by the host (permanent-resolver-failure)
// - 502 Bad Gateway : any other resolution error
//
// The request path is the name to resolve, e.g. `/localhost`. `/-` resolves
// nothing.
//
// With `?connect=<ip>:<port>` it then opens a TCP connection to that address
// and appends the outcome to the body: `connect: ok`, `connect: denied` (the
// host's socket policy refused it), or `connect: failed: <code>` for anything
// the network itself answered.
struct Component;

impl Handler for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let path = request.get_path_with_query().unwrap_or_default();
        let (path, query) = path.split_once('?').unwrap_or((&path, ""));
        let name = path.trim_start_matches('/').to_string();
        let target = query.strip_prefix("connect=");

        let lookup = if name == "-" {
            Ok(0)
        } else {
            resolve_addresses(name.clone())
                .await
                .map(|addrs| addrs.len())
        };
        let (status, mut body) = match lookup {
            Ok(count) => (200, format!("{name}: {count} addresses")),
            Err(ResolveErrorCode::PermanentResolverFailure) => {
                (403, format!("{name}: lookup denied by policy"))
            }
            Err(e) => (502, format!("{name}: lookup failed: {e:?}")),
        };
        if let (200, Some(target)) = (status, target) {
            body.push_str(&match connect(target).await {
                Ok(()) => "; connect: ok".to_string(),
                Err(SocketErrorCode::AccessDenied) => "; connect: denied".to_string(),
                Err(e) => format!("; connect: failed: {e:?}"),
            });
        }

        let (mut tx, rx) = bindings::wit_stream::new();
        let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());

        wit_bindgen::spawn_local(async move {
            tx.write_all(body.into_bytes()).await;
            drop(tx);
            let _ = trailers_tx.write(Ok(None)).await;
        });

        let (response, _result) = Response::new(Fields::new(), Some(rx), trailers_rx);
        response.set_status_code(status).map_err(|()| {
            ErrorCode::InternalError(Some("failed to set status code".to_string()))
        })?;
        Ok(response)
    }
}

async fn connect(target: &str) -> Result<(), SocketErrorCode> {
    let target: std::net::SocketAddr = target
        .parse()
        .map_err(|_| SocketErrorCode::InvalidArgument)?;
    let (family, remote) = match target {
        std::net::SocketAddr::V4(v4) => {
            let [a, b, c, d] = v4.ip().octets();
            (
                IpAddressFamily::Ipv4,
                IpSocketAddress::Ipv4(Ipv4SocketAddress {
                    port: v4.port(),
                    address: (a, b, c, d),
                }),
            )
        }
        std::net::SocketAddr::V6(v6) => {
            let [a, b, c, d, e, f, g, h] = v6.ip().segments();
            (
                IpAddressFamily::Ipv6,
                IpSocketAddress::Ipv6(Ipv6SocketAddress {
                    port: v6.port(),
                    flow_info: 0,
                    address: (a, b, c, d, e, f, g, h),
                    scope_id: 0,
                }),
            )
        }
    };
    let socket = TcpSocket::create(family)?;
    socket.connect(remote).await
}

bindings::export!(Component with_types_in bindings);
