mod bindings {
    wit_bindgen::generate!({
        path: "../wit",
        world: "web",
        generate_all,
        async: [
            "import:wasmcloud:wasi3-ai-app/producer@0.1.0#produce",
            "import:wasmcloud:wasi3-ai-app/summarizer@0.1.0#summarize",
            "export:wasi:http/handler@0.3.0#handle",
        ],
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasmcloud::wasi3_ai_app::producer;
use bindings::wasmcloud::wasi3_ai_app::summarizer;
use serde_json::Value;
use tracing::info;
use wit_bindgen::spawn_local;

static UI_HTML: &str = include_str!("../ui.html");

struct Component;

impl Handler for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        tracing_subscriber::fmt()
            .with_env_filter("info")
            .with_writer(std::io::stderr) // or stdout
            .init();

        //request.handle().
        // Get the full path + query string (e.g., "/task?id=123")
        let path_with_query = request
            .get_path_with_query()
            .unwrap_or_else(|| "/".to_string());

        // Extract the path portion – everything before the '?' character
        let path = path_with_query.split('?').next().unwrap_or("/");
        info!(path = path, "Frontend request");
        // Route based on the path
        match path {
            "/" => build_response(200, &[("content-type", "text/html")], UI_HTML.into()).await,
            "/start" => handle_transcription(request).await,
            "/summarize" => handle_summarize(request).await,
            _ => {
                build_response(
                    404,
                    &[("content-type", "text/html")],
                    "<h1>404 Not Found</h1>".to_string(),
                )
                .await
            }
        }
    }
}

async fn handle_summarize(request: Request) -> Result<Response, ErrorCode> {
    let payload = read_request_body(request).await?;

    let mut upstream = summarizer::summarize(payload).await;

    let headers = Fields::new();

    let (mut tx, rx) = bindings::wit_stream::new::<u8>();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());

    spawn_local(async move {
        while let Some(byte) = upstream.next().await {
            tx.write_all(vec![byte]).await;
        }
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });

    let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
    Ok(response)
}

/// Reads the entire request body into a Vec<u8>.
async fn read_request_body(req: Request) -> Result<String, ErrorCode> {
    // create a future for the body consumption (we don't need it for trailers)
    let (abort_tx, abort_rx) = bindings::wit_future::new(|| Ok(()));
    let (body_stream, _trailers_future) = Request::consume_body(req, abort_rx);
    let body: Vec<u8> = body_stream.collect().await;

    drop(abort_tx);

    let body_text = String::from_utf8(body).unwrap_or_else(|_| "Invalid UTF-8".into());

    let json: Value = serde_json::from_str(&body_text)
        .map_err(|e| ErrorCode::InternalError(Some(e.to_string())))?;
    let payload = json["payload"].to_string();
    
    info!(payload = payload, "Frontend request /summarize post");
    Ok(payload)
}

async fn handle_transcription(_request: Request) -> Result<Response, ErrorCode> {
    let mut upstream = producer::produce().await;

    let headers = Fields::new();

    let (mut tx, rx) = bindings::wit_stream::new::<u8>();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());

    spawn_local(async move {
        while let Some(byte) = upstream.next().await {
            tx.write_all(vec![byte]).await;
        }
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });

    let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
    Ok(response)
}

/// Build an HTTP response with the given status, headers, and body.
/// Headers are provided as a slice of (name, value) pairs.
/// The body is a string; it will be streamed as `text/plain` by default,
/// but you can override with the `content-type` header.
async fn build_response(
    status: u16,
    headers: &[(&str, &str)],
    body: String,
) -> Result<Response, ErrorCode> {
    // 1. Create headers
    let headers_fields = Fields::new();
    for (name, value) in headers {
        headers_fields
            .set(name, &[value.as_bytes().to_vec()])
            .map_err(|_| ErrorCode::InternalError(None))?;
    }
    // Default Content-Type if not set and body is non-empty
    if !body.is_empty()
        && !headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("content-type"))
    {
        headers_fields
            .set("content-type", &[b"text/plain; charset=utf-8".to_vec()])
            .map_err(|_| ErrorCode::InternalError(None))?;
    }

    // 2. Create a stream for the body
    let (mut tx, rx) = bindings::wit_stream::new::<u8>();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());

    // spawn_local a task to write the body
    spawn_local(async move {
        let body_bytes = body.as_bytes().to_vec();
        tx.write_all(body_bytes).await;
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });

    // 3. Create the Response resource
    let (response, _send_future) = Response::new(headers_fields, Some(rx), trailers_rx);

    // 4. Set status (if not 200)
    if status != 200 {
        response
            .set_status_code(status)
            .map_err(|_| ErrorCode::InternalError(None))?;
    }

    Ok(response)
}

bindings::export!(Component with_types_in bindings);
