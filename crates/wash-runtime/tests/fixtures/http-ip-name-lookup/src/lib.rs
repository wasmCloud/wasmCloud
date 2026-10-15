mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::{
    exports::wasi::http::incoming_handler::Guest,
    wasi::{
        http::types::{Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam},
        sockets::{
            instance_network::instance_network,
            ip_name_lookup::resolve_addresses,
            network::{ErrorCode, Network},
        },
    },
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

impl Guest for Component {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        let path = request.path_with_query().unwrap_or_default();
        let (path, query) = path.split_once('?').unwrap_or((&path, ""));
        let name = path.trim_start_matches('/');

        let network = instance_network();
        let lookup = if name == "-" {
            Ok(0)
        } else {
            resolve(&network, name)
        };
        let (status, mut body) = match lookup {
            Ok(count) => (200, format!("{name}: {count} addresses")),
            Err(ErrorCode::PermanentResolverFailure) => {
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
                    "connect" => connect(value),
                    "send" => datagrams.send(value),
                    "send-connected" => datagrams.send_connected(value),
                    "sleep" => {
                        let millis = value.parse().unwrap_or_default();
                        std::thread::sleep(std::time::Duration::from_millis(millis));
                        continue;
                    }
                    _ => continue,
                };
                body.push_str(&match outcome {
                    Ok(()) => format!("; {action}: ok"),
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                        format!("; {action}: denied")
                    }
                    Err(e) => format!("; {action}: failed: {:?}", e.kind()),
                });
            }
        }

        let response = OutgoingResponse::new(Fields::new());
        response.set_status_code(status).unwrap();
        let out_body = response.body().unwrap();
        ResponseOutparam::set(response_out, Ok(response));

        let stream = out_body.write().unwrap();
        stream.blocking_write_and_flush(body.as_bytes()).unwrap();
        drop(stream);
        OutgoingBody::finish(out_body, None).unwrap();
    }
}

fn resolve(network: &Network, name: &str) -> Result<u32, ErrorCode> {
    let stream = resolve_addresses(network, name)?;
    let pollable = stream.subscribe();
    let mut count = 0;
    loop {
        match stream.resolve_next_address() {
            Ok(Some(_addr)) => count += 1,
            Ok(None) => return Ok(count),
            Err(ErrorCode::WouldBlock) => pollable.block(),
            Err(e) => return Err(e),
        }
    }
}

/// Through `std`, which speaks `wasi:sockets` itself on this target: the
/// host's refusal arrives as `PermissionDenied`.
fn connect(target: &str) -> std::io::Result<()> {
    std::net::TcpStream::connect(parse(target)?).map(drop)
}

fn parse(target: &str) -> std::io::Result<std::net::SocketAddr> {
    target
        .parse()
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))
}

/// The sockets a request's datagrams go out on, each opened by the first
/// step that needs it and kept for the steps after: a client that resolved
/// its destination once and sends for as long as it lives.
#[derive(Default)]
struct Datagrams {
    unconnected: Option<std::net::UdpSocket>,
    connected: Option<std::net::UdpSocket>,
}

impl Datagrams {
    fn send(&mut self, target: &str) -> std::io::Result<()> {
        let target = parse(target)?;
        let socket = match &self.unconnected {
            Some(socket) => socket,
            None => &*self
                .unconnected
                .insert(std::net::UdpSocket::bind("0.0.0.0:0")?),
        };
        socket.send_to(b"metric:1|c", target).map(drop)
    }

    fn send_connected(&mut self, target: &str) -> std::io::Result<()> {
        let socket = match &self.connected {
            Some(socket) => socket,
            None => {
                let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
                socket.connect(parse(target)?)?;
                &*self.connected.insert(socket)
            }
        };
        socket.send(b"metric:1|c").map(drop)
    }
}

bindings::export!(Component with_types_in bindings);
