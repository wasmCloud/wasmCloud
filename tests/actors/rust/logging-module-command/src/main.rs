use std::env::args;
use std::io::stdin;

use anyhow::{anyhow, Context};
use serde::Deserialize;
use wasmcloud_actor::wasmcloud::bus::lattice::TargetEntity;
use wasmcloud_actor::{wasmcloud, HttpResponse, HttpServerRequest};

// TODO: Migrate this to Go

#[derive(Deserialize)]
struct LogRequest {
    level: String,
    context: String,
    message: String,
}

fn main() -> anyhow::Result<()> {
    // TODO: Change this to argv[1] once possible to set in Wasmtime
    assert_eq!(args().last().as_deref(), Some("wasi:logging/logging.log"));
    let LogRequest {
        level,
        context,
        message,
    } = serde_json::from_reader(stdin().lock()).context("failed to read log request")?;
    assert_eq!(level, "info");
    assert!(context.is_empty());
    let message = format!("[{}]{message}", env!("CARGO_PKG_NAME"));
    let req = rmp_serde::to_vec(&HttpServerRequest {
        body: message.as_bytes().into(),
        ..Default::default()
    })
    .context("failed to serialize http request")?;
    let res = wasmcloud::bus::host::call_sync(
        Some(&TargetEntity::Link(Some("default".into()))),
        "http-server/HttpServer.HandleRequest",
        &req,
    )
    .map_err(|e| anyhow!(e).context("failed to call `HttpServer.HandleRequest`"))?;
    let HttpResponse { body, .. } =
        rmp_serde::from_slice(&res).context("failed to deserialize response")?;
    let body = String::from_utf8(body).context("failed to parse response body as string")?;
    assert_eq!(body, format!("[http-compat-command]{message}"));
    // TODO: Publish/store the body
    eprintln!("{body}");
    Ok(())
}
