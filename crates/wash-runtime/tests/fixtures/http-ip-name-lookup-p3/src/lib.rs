mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasi::sockets::ip_name_lookup::{resolve_addresses, ErrorCode as ResolveErrorCode};

// The fixture reports the host-side ip-name-lookup policy decision via its
// status code:
//
// - 200 OK          : lookup permitted; body is the number of addresses found
// - 403 Forbidden   : denied by the host (permanent-resolver-failure)
// - 502 Bad Gateway : any other resolution error
//
// The request path is the name to resolve, e.g. `/localhost`.
struct Component;

impl Handler for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let path = request.get_path_with_query().unwrap_or_default();
        let name = path.trim_start_matches('/').to_string();

        let (status, body) = match resolve_addresses(name.clone()).await {
            Ok(addrs) => (200, format!("{name}: {} addresses", addrs.len())),
            Err(ResolveErrorCode::PermanentResolverFailure) => {
                (403, format!("{name}: lookup denied by policy"))
            }
            Err(e) => (502, format!("{name}: lookup failed: {e:?}")),
        };

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

bindings::export!(Component with_types_in bindings);
