mod bindings {
    wit_bindgen::generate!({
        world: "commander",
        path: "../wit",
        generate_all,
        async: [
            "import:wasmcloud:patch-stream/page-generation@0.1.0#generate-page",
            "import:wasmcloud:patch-stream/sink@0.1.0#send-stream",
            "export:wasi:http/handler@0.3.0-rc-2026-03-15#handle",
        ],
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasmcloud::patch_stream::{page_generation, sink};

struct Component;

const DEFAULT_PROMPT: &str =
    "Generate a small task-list page as newline-delimited JSON Patch operations.";

impl Handler for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let headers = Fields::new();
        let _ = headers.append(
            &"content-type".to_string(),
            &b"text/plain; charset=utf-8".to_vec(),
        );

        // Kick off the PageAgent and hand the resulting stream off to
        // meta-json. We don't keep a reader for ourselves — the
        // commander's job is to dispatch, meta-json is responsible
        // for persisting / logging the result.
        let prompt = prompt_from_path(request.get_path_with_query().as_deref());
        let page_rx = page_generation::generate_page(prompt).await;
        let result = sink::send_stream(page_rx).await;

        let (_trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
        let (response, _result) = Response::new(headers, None, trailers_rx);
        let status = match result {
            Ok(()) => 200,
            Err(()) => 502,
        };
        response
            .set_status_code(status)
            .map_err(|()| ErrorCode::InternalError(Some("set_status failed".into())))?;
        Ok(response)
    }
}

fn prompt_from_path(path_with_query: Option<&str>) -> String {
    let Some(path_with_query) = path_with_query else {
        return DEFAULT_PROMPT.to_string();
    };
    let Some((_, query)) = path_with_query.split_once('?') else {
        return DEFAULT_PROMPT.to_string();
    };

    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find_map(|(key, value)| {
            if key == "prompt" {
                Some(percent_decode(value))
            } else {
                None
            }
        })
        .filter(|prompt| !prompt.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_PROMPT.to_string())
}

fn percent_decode(input: &str) -> String {
    let mut out = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = from_hex(bytes[i + 1]);
                let lo = from_hex(bytes[i + 2]);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push((hi << 4) | lo);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

bindings::export!(Component with_types_in bindings);
