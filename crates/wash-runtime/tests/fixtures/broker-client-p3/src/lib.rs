//! Each route performs one broker operation and answers with its outcome:
//!
//! - `POST /send` sends the request body to `x-to` through `github`, adding
//!   `x-authorization` as `authorization` when given. 200 carries the upstream
//!   status, audit id, `x-echo-*` response headers and body; 502 carries the
//!   broker error.
//! - `POST /direct` sends the same request through `wasi:http/client`.
//! - `POST /sign` signs the body with `release`, in hex. `GET /key-info`
//!   returns its algorithm and hex public key.
//! - `POST /verify` checks the body against the hex `x-tag` with `webhook`.

mod bindings {
    wit_bindgen::generate!({ world: "broker-client-p3", generate_all });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response, Scheme};
use bindings::{github, release, webhook};

struct Component;

impl Handler for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let path = request.get_path_with_query().unwrap_or_default();
        let headers = request.get_headers();
        let header = |name: &str| {
            headers
                .get(name)
                .into_iter()
                .next()
                .map(|v| String::from_utf8_lossy(&v).into_owned())
        };
        let (to, authorization, tag) = (header("x-to"), header("x-authorization"), header("x-tag"));
        let (_, consumed) = bindings::wit_future::new(|| Ok(()));
        let (body, trailers) = Request::consume_body(request, consumed);

        Ok(match path.as_str() {
            "/send" | "/direct" => {
                let direct = path == "/direct";
                match send(
                    direct,
                    to.unwrap_or_default(),
                    authorization,
                    body,
                    trailers,
                )
                .await
                {
                    Ok(text) => respond(200, text),
                    Err(e) => respond(502, e),
                }
            }
            "/sign" => match release::sign(body).await {
                Ok(signature) => respond(200, hex(&signature)),
                Err(e) => respond(502, format!("{e:?}")),
            },
            "/key-info" => {
                let info = release::get_key_info().await;
                let key = hex(&info.public_key.unwrap_or_default());
                respond(200, format!("{:?} {key}", info.algorithm))
            }
            "/verify" => match webhook::verify(body, unhex(&tag.unwrap_or_default())).await {
                Ok(valid) => respond(200, valid.to_string()),
                Err(e) => respond(502, format!("{e:?}")),
            },
            _ => respond(404, "not found".into()),
        })
    }
}

async fn send(
    direct: bool,
    to: String,
    authorization: Option<String>,
    body: wit_bindgen::StreamReader<u8>,
    trailers: wit_bindgen::FutureReader<
        Result<Option<bindings::wasi::http::types::Fields>, ErrorCode>,
    >,
) -> Result<String, String> {
    let (scheme, rest) = to.split_once("://").ok_or("x-to is not a URL")?;
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let headers = Fields::new();
    if let Some(value) = authorization {
        headers
            .append("authorization", value.as_bytes())
            .map_err(|e| format!("{e:?}"))?;
    }
    let (request, _sent) = Request::new(headers, Some(body), trailers, None);
    let scheme = match scheme {
        "https" => Scheme::Https,
        "http" => Scheme::Http,
        other => Scheme::Other(other.into()),
    };
    let set = |r: Result<(), ()>| r.map_err(|()| "invalid request".to_string());
    set(request.set_scheme(Some(&scheme)))?;
    set(request.set_authority(Some(authority)))?;
    set(request.set_path_with_query(Some(if path.is_empty() { "/" } else { path })))?;

    let response = if direct {
        bindings::wasi::http::client::send(request)
            .await
            .map_err(|e| format!("{e:?}"))?
    } else {
        github::send(request).await.map_err(|e| format!("{e:?}"))?
    };
    let status = response.get_status_code();
    let headers = response.get_headers();
    let audit = headers
        .get("wasmcloud-audit-id")
        .into_iter()
        .next()
        .map(|v| String::from_utf8_lossy(&v).into_owned())
        .unwrap_or_default();
    let echoed: Vec<String> = headers
        .copy_all()
        .into_iter()
        .filter(|(name, _)| name.starts_with("x-echo-"))
        .map(|(name, value)| {
            format!(
                "{name}={}",
                String::from_utf8_lossy(&value).replace(' ', "+")
            )
        })
        .collect();
    let echoed = if echoed.is_empty() {
        "-".to_string()
    } else {
        echoed.join(",")
    };
    let (_, consumed) = bindings::wit_future::new(|| Ok(()));
    let (mut stream, _trailers) = Response::consume_body(response, consumed);
    let mut received = Vec::new();
    loop {
        let (result, chunk) = stream.read(Vec::with_capacity(16 * 1024)).await;
        received.extend(chunk);
        if !matches!(result, wit_bindgen::StreamResult::Complete(_)) {
            break;
        }
    }
    Ok(format!(
        "{status} {audit} {echoed} {}",
        String::from_utf8_lossy(&received)
    ))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .filter_map(|i| u8::from_str_radix(text.get(2 * i..2 * i + 2)?, 16).ok())
        .collect()
}

fn respond(status: u16, body: String) -> Response {
    let (mut tx, rx) = bindings::wit_stream::new();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
    wit_bindgen::spawn_local(async move {
        tx.write_all(body.into_bytes()).await;
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });
    let (response, _result) = Response::new(Fields::new(), Some(rx), trailers_rx);
    let _ = response.set_status_code(status);
    response
}

bindings::export!(Component with_types_in bindings);
