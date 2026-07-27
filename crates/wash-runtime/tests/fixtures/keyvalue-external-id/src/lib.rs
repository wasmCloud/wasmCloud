//! Real-guest fixture for `external-id` keyvalue routing.
//!
//! Imports `wasi:keyvalue/store` twice under the labels `users` and `catalog`,
//! each annotated with the platform name of the resource it expects. On each
//! HTTP request it writes a distinct value to the same key through each import
//! and reads both back. The host binds each import from its external-id alone —
//! nothing platform-side mentions `users` or `catalog` — so the values must
//! still land in separate backends. Responds `isolated` on success, `leak: …`
//! otherwise.

mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::http::incoming_handler::Guest;
use bindings::wasi::http::types::{
    Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam,
};

struct Component;

const KEY: &str = "k";

impl Guest for Component {
    fn handle(_request: IncomingRequest, response_out: ResponseOutparam) {
        let body = match run() {
            Ok(s) => s,
            Err(e) => format!("error: {e}"),
        };

        let response = OutgoingResponse::new(Fields::new());
        response.set_status_code(200).unwrap();
        let out_body = response.body().unwrap();
        ResponseOutparam::set(response_out, Ok(response));

        let stream = out_body.write().unwrap();
        stream.blocking_write_and_flush(body.as_bytes()).unwrap();
        drop(stream);
        OutgoingBody::finish(out_body, None).unwrap();
    }
}

fn run() -> Result<String, String> {
    let users = bindings::users::open("bucket").map_err(|e| format!("open users: {e:?}"))?;
    let catalog = bindings::catalog::open("bucket").map_err(|e| format!("open catalog: {e:?}"))?;

    users
        .set(KEY, b"from-users")
        .map_err(|e| format!("set users: {e:?}"))?;
    catalog
        .set(KEY, b"from-catalog")
        .map_err(|e| format!("set catalog: {e:?}"))?;

    let u = users.get(KEY).map_err(|e| format!("get users: {e:?}"))?;
    let c = catalog.get(KEY).map_err(|e| format!("get catalog: {e:?}"))?;

    if u.as_deref() == Some(b"from-users".as_slice())
        && c.as_deref() == Some(b"from-catalog".as_slice())
    {
        Ok("isolated".to_string())
    } else {
        Ok(format!("leak: users={u:?} catalog={c:?}"))
    }
}

bindings::export!(Component with_types_in bindings);
