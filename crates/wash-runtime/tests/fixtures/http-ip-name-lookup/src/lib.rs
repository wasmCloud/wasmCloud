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
            instance_network::instance_network, ip_name_lookup::resolve_addresses,
            network::ErrorCode,
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
// The request path is the name to resolve, e.g. `/localhost`.
struct Component;

impl Guest for Component {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        let path = request.path_with_query().unwrap_or_default();
        let name = path.trim_start_matches('/');

        let (status, body) = match resolve(name) {
            Ok(count) => (200, format!("{name}: {count} addresses")),
            Err(ErrorCode::PermanentResolverFailure) => {
                (403, format!("{name}: lookup denied by policy"))
            }
            Err(e) => (502, format!("{name}: lookup failed: {e:?}")),
        };

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

fn resolve(name: &str) -> Result<u32, ErrorCode> {
    let network = instance_network();
    let stream = resolve_addresses(&network, name)?;
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

bindings::export!(Component with_types_in bindings);
