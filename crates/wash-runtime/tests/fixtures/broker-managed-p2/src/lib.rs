//! A managed p2 component reporting what its own network reaches:
//!
//! - `GET /http?<url>` sends through `wasi:http/outgoing-handler`.
//! - `GET /tcp?<ip>:<port>` connects through `wasi:sockets`.
//! - `GET /key-info` calls the broker, which is what makes it managed.
//!
//! Each answers 200 with the outcome as text.

mod bindings {
    wit_bindgen::generate!({ world: "broker-managed-p2", generate_all });
}

use bindings::exports::wasi::http::incoming_handler::Guest;
use bindings::release;
use bindings::wasi::http::outgoing_handler;
use bindings::wasi::http::types::{
    Fields, IncomingRequest, OutgoingBody, OutgoingRequest, OutgoingResponse, ResponseOutparam,
    Scheme,
};

struct Component;

impl Guest for Component {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        let path = request.path_with_query().unwrap_or_default();
        let (route, target) = path.split_once('?').unwrap_or((&path, ""));
        let body = match route {
            "/http" => http(target),
            "/tcp" => connect(target),
            "/key-info" => format!(
                "{:?}",
                wit_bindgen::block_on(release::get_key_info()).algorithm
            ),
            _ => "not found".to_string(),
        };
        let response = OutgoingResponse::new(Fields::new());
        response.set_status_code(200).unwrap();
        let out = response.body().unwrap();
        ResponseOutparam::set(response_out, Ok(response));
        let stream = out.write().unwrap();
        stream.blocking_write_and_flush(body.as_bytes()).unwrap();
        drop(stream);
        OutgoingBody::finish(out, None).unwrap();
    }
}

fn http(url: &str) -> String {
    let rest = url.strip_prefix("https://").unwrap_or(url);
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let request = OutgoingRequest::new(Fields::new());
    let _ = request.set_scheme(Some(&Scheme::Https));
    let _ = request.set_authority(Some(authority));
    let _ = request.set_path_with_query(Some(if path.is_empty() { "/" } else { path }));
    let pending = match outgoing_handler::handle(request, None) {
        Ok(pending) => pending,
        Err(e) => return format!("{e:?}"),
    };
    pending.subscribe().block();
    match pending.get() {
        Some(Ok(Ok(response))) => format!("status {}", response.status()),
        Some(Ok(Err(e))) => format!("{e:?}"),
        other => format!("{other:?}"),
    }
}

fn connect(target: &str) -> String {
    use bindings::wasi::sockets::network::{IpAddressFamily, IpSocketAddress, Ipv4SocketAddress};
    use bindings::wasi::sockets::{instance_network, tcp_create_socket};
    let Ok(std::net::SocketAddr::V4(addr)) = target.parse() else {
        return "not an IPv4 address".to_string();
    };
    let [a, b, c, d] = addr.ip().octets();
    let to = IpSocketAddress::Ipv4(Ipv4SocketAddress {
        port: addr.port(),
        address: (a, b, c, d),
    });
    let result = tcp_create_socket::create_tcp_socket(IpAddressFamily::Ipv4).and_then(|socket| {
        socket.start_connect(&instance_network::instance_network(), to)?;
        socket.subscribe().block();
        socket.finish_connect().map(drop)
    });
    match result {
        Ok(()) => "connected".to_string(),
        Err(e) => format!("{e:?}"),
    }
}

bindings::export!(Component with_types_in bindings);
