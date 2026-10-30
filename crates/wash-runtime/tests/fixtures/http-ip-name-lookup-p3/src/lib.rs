mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasi::sockets::ip_name_lookup::{resolve_addresses, ErrorCode as ResolveErrorCode};
use bindings::wasi::clocks::monotonic_clock;
use bindings::wasi::sockets::types::{
    ErrorCode as SocketErrorCode, IpAddressFamily, IpSocketAddress, Ipv4SocketAddress,
    Ipv6SocketAddress, TcpSocket, UdpSocket,
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
// The query is a list of steps joined by `&`, run in order once the lookup is
// permitted. Each but `sleep` appends its outcome to the body as
// `; <step>: ok`, `; <step>: denied` (the host's socket policy refused it), or
// `; <step>: failed: <code>` for anything the network itself answered.
//
// - `connect=<ip>:<port>`        : open a TCP connection
// - `send=<ip>:<port>`           : send a datagram from one unconnected socket
// - `send-connected=<ip>:<port>` : send a datagram on a socket connected to
//                                  that address by the first such step
// - `sleep=<ms>`                 : wait
struct Component;

impl Handler for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let path = request.get_path_with_query().unwrap_or_default();
        let (path, query) = path.split_once('?').unwrap_or((&path, ""));
        let name = path.trim_start_matches('/').to_string();

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
        if status == 200 {
            let mut datagrams = Datagrams::default();
            for step in query.split('&') {
                let Some((action, value)) = step.split_once('=') else {
                    continue;
                };
                let outcome = match action {
                    "connect" => connect(value).await,
                    "send" => datagrams.send(value).await,
                    "send-connected" => datagrams.send_connected(value).await,
                    "sleep" => {
                        let millis: u64 = value.parse().unwrap_or_default();
                        monotonic_clock::wait_for(millis.saturating_mul(1_000_000)).await;
                        continue;
                    }
                    _ => continue,
                };
                body.push_str(&match outcome {
                    Ok(()) => format!("; {action}: ok"),
                    Err(SocketErrorCode::AccessDenied) => format!("; {action}: denied"),
                    Err(e) => format!("; {action}: failed: {e:?}"),
                });
            }
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
    let (family, remote) = parse(target)?;
    let socket = TcpSocket::create(family)?;
    socket.connect(remote).await
}

/// The sockets a request's datagrams go out on, each opened by the first
/// step that needs it and kept for the steps after: a client that resolved
/// its destination once and sends for as long as it lives.
#[derive(Default)]
struct Datagrams {
    unconnected: Option<UdpSocket>,
    connected: Option<UdpSocket>,
}

impl Datagrams {
    async fn send(&mut self, target: &str) -> Result<(), SocketErrorCode> {
        let (family, remote) = parse(target)?;
        let socket = match &self.unconnected {
            Some(socket) => socket,
            None => &*self.unconnected.insert(UdpSocket::create(family)?),
        };
        socket.send(b"metric:1|c".to_vec(), Some(remote)).await
    }

    async fn send_connected(&mut self, target: &str) -> Result<(), SocketErrorCode> {
        let socket = match &self.connected {
            Some(socket) => socket,
            None => {
                let (family, remote) = parse(target)?;
                let socket = UdpSocket::create(family)?;
                socket.bind(match family {
                    IpAddressFamily::Ipv4 => IpSocketAddress::Ipv4(Ipv4SocketAddress {
                        port: 0,
                        address: (0, 0, 0, 0),
                    }),
                    IpAddressFamily::Ipv6 => IpSocketAddress::Ipv6(Ipv6SocketAddress {
                        port: 0,
                        flow_info: 0,
                        address: (0, 0, 0, 0, 0, 0, 0, 0),
                        scope_id: 0,
                    }),
                })?;
                socket.connect(remote)?;
                &*self.connected.insert(socket)
            }
        };
        socket.send(b"metric:1|c".to_vec(), None).await
    }
}

fn parse(target: &str) -> Result<(IpAddressFamily, IpSocketAddress), SocketErrorCode> {
    let target: std::net::SocketAddr = target
        .parse()
        .map_err(|_| SocketErrorCode::InvalidArgument)?;
    Ok(match target {
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
    })
}

bindings::export!(Component with_types_in bindings);
