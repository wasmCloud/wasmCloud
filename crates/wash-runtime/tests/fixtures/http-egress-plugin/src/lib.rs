//! Host component plugin that imports `wasi:http/outgoing-handler`, gated by
//! its own `allowedHosts` policy — proving a plugin's own outgoing HTTP calls
//! are secured independently of any workload's.

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "http-egress-plugin", generate_all });
}

use bindings::exports::acme::httpegress::fetch::Guest;
use bindings::wasi::http::outgoing_handler::handle;
use bindings::wasi::http::types::{ErrorCode, Fields, Method, OutgoingRequest, RequestOptions, Scheme};

struct Component;

impl Guest for Component {
    async fn fetch(host: String) -> String {
        match send(&host) {
            Ok(status) => status.to_string(),
            Err(e) => {
                if matches!(e.downcast_ref::<ErrorCode>(), Some(ErrorCode::HttpRequestDenied)) {
                    "403".to_string()
                } else {
                    format!("error: {e}")
                }
            }
        }
    }
}

fn send(host: &str) -> anyhow::Result<u16> {
    let request = OutgoingRequest::new(Fields::new());
    request
        .set_scheme(Some(&Scheme::Http))
        .map_err(|()| anyhow::anyhow!("failed to set scheme"))?;
    request
        .set_authority(Some(host))
        .map_err(|()| anyhow::anyhow!("failed to set authority"))?;
    request
        .set_path_with_query(Some("/"))
        .map_err(|()| anyhow::anyhow!("failed to set path"))?;
    request
        .set_method(&Method::Get)
        .map_err(|()| anyhow::anyhow!("failed to set method"))?;

    let future_response = handle(request, Some(RequestOptions::new()))
        .map_err(|e| anyhow::anyhow!(e))?;
    future_response.subscribe().block();
    let response = future_response
        .get()
        .ok_or_else(|| anyhow::anyhow!("no response available"))?
        .map_err(|()| anyhow::anyhow!("response already taken"))?
        .map_err(|e: ErrorCode| anyhow::Error::from(e))?;
    Ok(response.status())
}

mod export {
    #![allow(unsafe_code)]
    use super::{Component, bindings};
    bindings::export!(Component with_types_in bindings);
}
