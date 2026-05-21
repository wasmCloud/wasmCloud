use anyhow::Context as _;

use serde::Deserialize;
use serde_json::Value;
use wstd::future::FutureExt as _;
use wstd::http::{Body, Request, Response, StatusCode};
use wstd::io::{AsyncRead, AsyncWrite};
use wstd::time::Duration;

static UI_HTML: &str = include_str!("../ui.html");

#[wstd::http_server]
async fn main(req: Request<Body>) -> anyhow::Result<Response<Body>> {
    match (req.method().as_str(), req.uri().path()) {
        ("GET", "/") => home().await,
        ("GET", "/todos") => list_todos(&req).await,
        ("GET", "/tags") => list_tags().await,
        ("POST", "/todos") => create_todo(req).await,
        ("POST", "/todos/done") => mark_done(req).await,
        ("POST", "/todos/delete") => delete_todo(req).await,
        _ => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body("Not found\n".into())
            .map_err(Into::into),
    }
}

async fn home() -> anyhow::Result<Response<Body>> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/html")
        .body(UI_HTML.into())
        .map_err(Into::into)
}

async fn list_todos(req: &Request<Body>) -> anyhow::Result<Response<Body>> {
    // Optional `?tag=<name>` query string filters the list server-side.
    let tag = req.uri().query().and_then(|q| {
        q.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            (k == "tag").then(|| urlencoding_decode(v))
        })
    });
    let payload = match tag {
        Some(t) => serde_json::json!({ "op": "list", "tag": t }),
        None => serde_json::json!({ "op": "list" }),
    };
    json_reply(call_service(payload).await?)
}

async fn list_tags() -> anyhow::Result<Response<Body>> {
    json_reply(call_service(serde_json::json!({ "op": "tags" })).await?)
}

#[derive(Deserialize)]
struct CreateBody {
    description: String,
    #[serde(default)]
    tags: Vec<String>,
}

async fn create_todo(mut req: Request<Body>) -> anyhow::Result<Response<Body>> {
    let body: CreateBody = req
        .body_mut()
        .json()
        .await
        .context("failed to parse body")?;
    json_reply(
        call_service(serde_json::json!({
            "op": "create",
            "description": body.description,
            "tags": body.tags,
        }))
        .await?,
    )
}

/// Minimal `%XX` decoder for query-string values. The UI only ever sends ASCII
/// tag names (alphanumeric + a few separators), so we don't pull in a
/// full URL parser.
fn urlencoding_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
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

#[derive(Deserialize)]
struct IdBody {
    id: i64,
}

async fn mark_done(mut req: Request<Body>) -> anyhow::Result<Response<Body>> {
    let body: IdBody = req
        .body_mut()
        .json()
        .await
        .context("failed to parse body")?;
    json_reply(call_service(serde_json::json!({ "op": "done", "id": body.id })).await?)
}

async fn delete_todo(mut req: Request<Body>) -> anyhow::Result<Response<Body>> {
    let body: IdBody = req
        .body_mut()
        .json()
        .await
        .context("failed to parse body")?;
    json_reply(call_service(serde_json::json!({ "op": "delete", "id": body.id })).await?)
}

fn json_reply(value: Value) -> anyhow::Result<Response<Body>> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(&value)?.into())
        .map_err(Into::into)
}

async fn call_service(command: Value) -> anyhow::Result<Value> {
    let mut line = serde_json::to_vec(&command)?;
    line.push(b'\n');

    let client = wstd::net::TcpStream::connect("127.0.0.1:7777")
        .await
        .context("failed to connect to db service")?;

    let raw = async {
        let (mut reader, mut writer) = client.split();
        writer.write_all(&line).await?;
        writer.flush().await?;

        let mut resp_buf = Vec::new();
        let mut buf = [0u8; 1024];

        loop {
            let n = reader.read(&mut buf).await?;
            if n == 0 {
                return Err(std::io::Error::other(
                    "connection closed before response was complete",
                ));
            }
            for &byte in &buf[..n] {
                if byte == b'\n' {
                    return Ok(resp_buf);
                }
                resp_buf.push(byte);
            }
        }
    }
    .timeout(Duration::from_secs(10))
    .await
    .context("db service timed out")?
    .context("failed to communicate with db service")?;

    serde_json::from_slice::<Value>(&raw).context("invalid JSON from db service")
}
