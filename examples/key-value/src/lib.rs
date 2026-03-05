mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::{
    exports::wasi::http::incoming_handler::Guest,
    wasi::{
        http::types::{
            Fields, IncomingBody, IncomingRequest, Method, OutgoingBody, OutgoingResponse,
            ResponseOutparam,
        },
        keyvalue::store::open,
    },
};

struct Component;

/// The keyvalue backend to use.
///
/// This constant is used as the bucket identifier passed to `open()`. The host
/// runtime selects the actual backend based on `.wash/config.yaml`:
///
/// | `BACKEND`      | Required config                              |
/// |----------------|----------------------------------------------|
/// | `"in_memory"`  | none (default)                               |
/// | `"filesystem"` | `wasi_keyvalue_path: /tmp/keyvalue-store`    |
/// | `"nats"`       | `wasi_keyvalue_nats_url: nats://...`         |
/// | `"redis"`      | `wasi_keyvalue_redis_url: redis://...`       |
///
/// Change this constant and uncomment the matching section in `.wash/config.yaml`
/// to switch backends.
const BACKEND: &str = "nats";

impl Guest for Component {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        let (status, body) = match request.method() {
            Method::Post => handle_post(request),
            Method::Get => handle_get(request),
            _ => (405u16, "Method Not Allowed\n".to_string()),
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

/// Handle `POST /` with a JSON body `{"key": "...", "value": "..."}`.
/// Stores the key-value pair in the configured backend under the `BACKEND` bucket.
fn handle_post(request: IncomingRequest) -> (u16, String) {
    let body_bytes = match read_body(request) {
        Ok(b) => b,
        Err(e) => return (400, format!("Failed to read body: {e}\n")),
    };

    let payload: KvPayload = match serde_json::from_slice(&body_bytes) {
        Ok(v) => v,
        Err(e) => return (400, format!("Invalid JSON (expected {{\"key\":\"...\",\"value\":\"...\"}}): {e}\n")),
    };

    let bucket = match open(BACKEND) {
        Ok(b) => b,
        Err(e) => return (500, format!("Failed to open keyvalue bucket: {e:?}\n")),
    };

    match bucket.set(&payload.key, payload.value.as_bytes()) {
        Ok(_) => (200, format!("[{BACKEND}] Stored key '{}'\n", payload.key)),
        Err(e) => (500, format!("[{BACKEND}] Failed to store key: {e:?}\n")),
    }
}

/// Handle `GET /?key=<key>`.
/// Returns the stored value if the key exists, or 404 if not.
fn handle_get(request: IncomingRequest) -> (u16, String) {
    let path_and_query = request.path_with_query().unwrap_or_default();

    let key = match parse_query_param(&path_and_query, "key") {
        Some(k) => k,
        None => return (400, "Missing required query parameter: key\n".to_string()),
    };

    let bucket = match open(BACKEND) {
        Ok(b) => b,
        Err(e) => return (500, format!("Failed to open keyvalue bucket: {e:?}\n")),
    };

    match bucket.get(&key) {
        Ok(Some(bytes)) => (200, format!("[{BACKEND}] {}\n", String::from_utf8_lossy(&bytes))),
        Ok(None) => (404, format!("[{BACKEND}] Key '{key}' not found\n")),
        Err(e) => (500, format!("[{BACKEND}] Failed to get key: {e:?}\n")),
    }
}

#[derive(serde::Deserialize)]
struct KvPayload {
    key: String,
    value: String,
}

fn read_body(request: IncomingRequest) -> Result<Vec<u8>, String> {
    let body = request
        .consume()
        .map_err(|_| "failed to consume request body".to_string())?;
    let stream = body
        .stream()
        .map_err(|_| "failed to get body stream".to_string())?;

    let mut data = Vec::new();
    loop {
        match stream.read(65536) {
            Ok(chunk) if chunk.is_empty() => break,
            Ok(chunk) => data.extend_from_slice(&chunk),
            Err(_) => break,
        }
    }
    drop(stream);
    IncomingBody::finish(body);
    Ok(data)
}

fn parse_query_param(path_and_query: &str, param: &str) -> Option<String> {
    let query = path_and_query.splitn(2, '?').nth(1)?;
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        if parts.next()? == param {
            return parts.next().map(|v| v.to_string());
        }
    }
    None
}

bindings::export!(Component with_types_in bindings);
