mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::{
    exports::wasi::http::incoming_handler::Guest,
    wasi::http::types::{
        Fields, IncomingBody, IncomingRequest, Method, OutgoingBody, OutgoingResponse,
        ResponseOutparam,
    },
    wasmcloud::couchbase::document,
};

/// The Couchbase bucket this component reads and writes.
///
/// Configured via the `bucket` key in `.wash/config.yaml`:
/// ```yaml
/// dev:
///   couchbase_url: http://Administrator:password@localhost:8091
///   interfaces:
///     wasmcloud:couchbase:
///       bucket: demo
/// ```
struct Component;

impl Guest for Component {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        let path = request.path_with_query().unwrap_or_default();
        let (status, body) = match request.method() {
            Method::Get => handle_get(&path),
            Method::Post => handle_post(request, &path),
            Method::Delete => handle_delete(&path),
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

// ── Route handlers ────────────────────────────────────────────────────────────

/// `GET /?key=<key>` — fetch a document by key.
///
/// Returns the document content as JSON, or 404 if not found.
fn handle_get(path: &str) -> (u16, String) {
    let key = match parse_query_param(path, "key") {
        Some(k) => k,
        None => return (400, "Missing required query parameter: key\n".to_string()),
    };

    match document::get(&key) {
        Ok(doc) => (
            200,
            format!(
                "{{\"key\":{},\"content\":{},\"cas\":{}}}\n",
                serde_json::to_string(&doc.key).unwrap_or_default(),
                doc.content,
                doc.cas
            ),
        ),
        Err(document::DocumentError::NotFound) => (404, format!("Document '{}' not found\n", key)),
        Err(e) => (500, format!("Error: {e:?}\n")),
    }
}

/// `POST /<key>` with JSON body — upsert a document.
///
/// The path segment after `/` is used as the document key.
/// The request body must be valid JSON and becomes the document content.
///
/// Optional query parameter `expiry` (seconds, default 0 = no expiry).
fn handle_post(request: IncomingRequest, path: &str) -> (u16, String) {
    let key = path_key(path);
    if key.is_empty() {
        return (
            400,
            "Specify the document key in the URL path: POST /<key>\n".to_string(),
        );
    }

    let expiry: u32 = parse_query_param(path, "expiry")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let body_bytes = match read_body(request) {
        Ok(b) => b,
        Err(e) => return (400, format!("Failed to read request body: {e}\n")),
    };

    // Validate JSON before sending to Couchbase
    let content: serde_json::Value = match serde_json::from_slice(&body_bytes) {
        Ok(v) => v,
        Err(e) => return (400, format!("Body must be valid JSON: {e}\n")),
    };

    match document::upsert(&key, &content.to_string(), expiry) {
        Ok(cas) => (
            200,
            format!(
                "{{\"key\":{},\"cas\":{}}}\n",
                serde_json::to_string(&key).unwrap_or_default(),
                cas
            ),
        ),
        Err(e) => (500, format!("Upsert failed: {e:?}\n")),
    }
}

/// `DELETE /<key>` — remove a document.
///
/// The path segment after `/` is used as the document key.
fn handle_delete(path: &str) -> (u16, String) {
    let key = path_key(path);
    if key.is_empty() {
        return (
            400,
            "Specify the document key in the URL path: DELETE /<key>\n".to_string(),
        );
    }

    match document::remove(&key, 0) {
        Ok(()) => (200, format!("Deleted '{}'\n", key)),
        Err(document::DocumentError::NotFound) => (404, format!("Document '{}' not found\n", key)),
        Err(e) => (500, format!("Delete failed: {e:?}\n")),
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract the first path segment after the leading `/` as the document key.
///
/// `"/my-doc?expiry=60"` → `"my-doc"`
fn path_key(path: &str) -> String {
    path.trim_start_matches('/')
        .split('?')
        .next()
        .unwrap_or("")
        .to_string()
}

fn parse_query_param(path: &str, param: &str) -> Option<String> {
    let query = path.split('?').nth(1)?;
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        if parts.next()? == param {
            return parts.next().map(|v| v.to_string());
        }
    }
    None
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

bindings::export!(Component with_types_in bindings);
