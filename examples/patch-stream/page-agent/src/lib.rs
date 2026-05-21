mod bindings {
    wit_bindgen::generate!({
        world: "page-agent",
        path: "../wit",
        generate_all,
        async: [
            "export:wasmcloud:patch-stream/page-generation@0.1.0#generate-page",
            "import:wasi:clocks/monotonic-clock@0.3.0-rc-2026-03-15#wait-for",
            "import:wasi:http/client@0.3.0-rc-2026-03-15#send",
        ],
    });
}

use bindings::exports::wasmcloud::patch_stream::page_generation::Guest;
use bindings::wasi::cli::environment;
use bindings::wasi::clocks::monotonic_clock;
use bindings::wasi::http::{
    client,
    types::{Fields, Method, Request, RequestOptions, Response, Scheme},
};
use serde::Deserialize;
use serde_json::json;
use wit_bindgen::{StreamReader, StreamWriter};

struct Component;

/// 500ms between demo patches - slow enough for websocket clients to visibly
/// render each frame while debugging host-side buffering.
const TICK_NS: u64 = 500_000_000;

const SYSTEM_PROMPT: &str = r#"You are PageAgent. Produce newline-delimited JSON Patch operations for a tiny page document.
Each output line must be one JSON object with string fields: op, path, value.
The value field must itself be a JSON-encoded string. Do not wrap the response in markdown."#;

impl Guest for Component {
    async fn generate_page(prompt: String) -> StreamReader<u8> {
        let (mut writer, reader) = bindings::wit_stream::new::<u8>();

        wit_bindgen::spawn(async move {
            if let Err(err) = stream_openai_chat(&prompt, &mut writer).await {
                stream_demo_page(&prompt, Some(&err), &mut writer).await;
            }
            // Writer drops at end of scope -> stream closes -> MetaJson closes
            // the websocket / sink once the final line has been forwarded.
        });

        reader
    }
}

async fn stream_openai_chat(prompt: &str, writer: &mut StreamWriter<u8>) -> Result<(), String> {
    let api_key = env("PAGE_AGENT_OPENAI_API_KEY")
        .or_else(|| env("OPENAI_API_KEY"))
        .ok_or_else(|| {
            "OPENAI_API_KEY not configured; using deterministic PageAgent demo".to_string()
        })?;

    let model = env("PAGE_AGENT_OPENAI_MODEL")
        .or_else(|| env("OPENAI_MODEL"))
        .unwrap_or_else(|| "gpt-4o-mini".to_string());
    let host = env("PAGE_AGENT_OPENAI_HOST").unwrap_or_else(|| "api.openai.com".to_string());
    let path = env("PAGE_AGENT_OPENAI_PATH").unwrap_or_else(|| "/v1/chat/completions".to_string());
    let scheme = match env("PAGE_AGENT_OPENAI_SCHEME")
        .unwrap_or_else(|| "https".to_string())
        .as_str()
    {
        "http" => Scheme::Http,
        _ => Scheme::Https,
    };

    let headers = Fields::new();
    append_header(&headers, "content-type", "application/json")?;
    append_header(&headers, "accept", "text/event-stream")?;
    append_header(&headers, "authorization", &format!("Bearer {api_key}"))?;

    let body = json!({
        "model": model,
        "stream": true,
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": prompt },
        ],
    })
    .to_string()
    .into_bytes();

    let (mut body_tx, body_rx) = bindings::wit_stream::new::<u8>();
    let (_trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
    let options = RequestOptions::new();
    let (request, _request_sent) = Request::new(headers, Some(body_rx), trailers_rx, Some(options));
    request
        .set_method(&Method::Post)
        .map_err(|()| "failed to set OpenAI request method".to_string())?;
    request
        .set_scheme(Some(&scheme))
        .map_err(|()| "failed to set OpenAI request scheme".to_string())?;
    request
        .set_authority(Some(&host))
        .map_err(|()| "failed to set OpenAI request authority".to_string())?;
    request
        .set_path_with_query(Some(&path))
        .map_err(|()| "failed to set OpenAI request path".to_string())?;

    wit_bindgen::spawn(async move {
        body_tx.write_all(body).await;
    });

    let response = client::send(request)
        .await
        .map_err(|err| format!("OpenAI request failed before response was available: {err:?}"))?;

    let status = response.get_status_code();
    let (_response_done_tx, response_done_rx) = bindings::wit_future::new(|| Ok(()));
    let (body_rx, _trailers_rx) = Response::consume_body(response, response_done_rx);

    if !(200..300).contains(&status) {
        let body = collect_body(body_rx, 4096).await;
        return Err(format!(
            "OpenAI request returned HTTP {status}: {}",
            String::from_utf8_lossy(&body)
        ));
    }

    stream_chat_completion_sse(body_rx, writer).await
}

async fn stream_chat_completion_sse(
    mut body_rx: StreamReader<u8>,
    writer: &mut StreamWriter<u8>,
) -> Result<(), String> {
    let mut line = Vec::with_capacity(1024);
    while let Some(byte) = body_rx.next().await {
        match byte {
            b'\n' => {
                handle_sse_line(&line, writer).await?;
                line.clear();
            }
            b'\r' => {}
            byte => line.push(byte),
        }
    }

    if !line.is_empty() {
        handle_sse_line(&line, writer).await?;
    }

    Ok(())
}

async fn handle_sse_line(line: &[u8], writer: &mut StreamWriter<u8>) -> Result<(), String> {
    let line = std::str::from_utf8(line).map_err(|err| err.to_string())?;
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(());
    };
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return Ok(());
    }

    let chunk: ChatCompletionChunk =
        serde_json::from_str(data).map_err(|err| format!("invalid OpenAI SSE chunk: {err}"))?;
    for choice in chunk.choices {
        if let Some(content) = choice.delta.content {
            writer.write_all(content.into_bytes()).await;
        }
    }
    Ok(())
}

async fn stream_demo_page(prompt: &str, ai_error: Option<&str>, writer: &mut StreamWriter<u8>) {
    let start_ns = monotonic_clock::now();

    let mut edits = vec![
        patch("add", "/title", json!("Untitled").to_string()),
        patch("add", "/version", json!(0).to_string()),
        patch("add", "/items", json!([]).to_string()),
        patch("add", "/prompt", json!(prompt).to_string()),
        patch(
            "replace",
            "/title",
            json!("AI-assisted streaming demo").to_string(),
        ),
        patch(
            "add",
            "/items/0",
            json!({"id": 1, "name": "receive commander prompt", "done": true}).to_string(),
        ),
        patch(
            "add",
            "/items/1",
            json!({"id": 2, "name": "ask PageAgent to generate page", "done": true}).to_string(),
        ),
        patch(
            "add",
            "/items/2",
            json!({"id": 3, "name": "stream bytes into MetaJson", "done": false}).to_string(),
        ),
        patch("replace", "/version", json!(1).to_string()),
        patch("add", "/tags/0", json!("commander").to_string()),
        patch("add", "/tags/1", json!("page-agent").to_string()),
        patch("add", "/tags/2", json!("meta-json").to_string()),
        patch("replace", "/items/2/done", json!(true).to_string()),
        patch("replace", "/version", json!(2).to_string()),
        patch(
            "add",
            "/meta",
            json!({"emitted_by": "page-agent", "mode": "demo-fallback"}).to_string(),
        ),
    ];

    if let Some(ai_error) = ai_error {
        edits.push(patch("add", "/meta/ai_error", json!(ai_error).to_string()));
    }

    for edit in edits {
        let elapsed_ms = monotonic_clock::now().saturating_sub(start_ns) / 1_000_000;
        let mut line = format!("[t+{:>4}ms] ", elapsed_ms).into_bytes();
        line.extend_from_slice(edit.as_bytes());
        line.push(b'\n');
        writer.write_all(line).await;
        monotonic_clock::wait_for(TICK_NS).await;
    }
}

fn patch(op: &str, path: &str, value: String) -> String {
    json!({
        "op": op,
        "path": path,
        "value": value,
    })
    .to_string()
}

async fn collect_body(mut body_rx: StreamReader<u8>, max: usize) -> Vec<u8> {
    let mut body = Vec::new();
    while let Some(byte) = body_rx.next().await {
        if body.len() >= max {
            break;
        }
        body.push(byte);
    }
    body
}

fn append_header(headers: &Fields, name: &str, value: &str) -> Result<(), String> {
    headers
        .append(&name.to_string(), &value.as_bytes().to_vec())
        .map_err(|err| format!("failed to append header {name}: {err:?}"))
}

fn env(name: &str) -> Option<String> {
    environment::get_environment()
        .into_iter()
        .find_map(|(key, value)| if key == name { Some(value) } else { None })
        .filter(|value| !value.trim().is_empty())
}

#[derive(Deserialize)]
struct ChatCompletionChunk {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    delta: ChatDelta,
}

#[derive(Deserialize)]
struct ChatDelta {
    content: Option<String>,
}

bindings::export!(Component with_types_in bindings);
