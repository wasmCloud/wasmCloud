mod common;
use common::*;

use std::str::FromStr;

use anyhow::{bail, ensure, Context};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::json;
use tokio::fs;
use wasmcloud::capability::numbergen::Uuid;
use wasmcloud::capability::{HandlerFunc, HostInvocation};
use wasmcloud::{Actor, Runtime};
use wasmcloud_interface_httpserver::{HttpRequest, HttpResponse};

static REQUEST: Lazy<Vec<u8>> = Lazy::new(|| {
    let body = serde_json::to_vec(&json!({
        "min": 42,
        "max": 4242,
    }))
    .expect("failed to encode body to JSON");
    serialize(&HttpRequest {
        body,
        ..Default::default()
    })
    .expect("failed to serialize request")
});

async fn host_call(
    claims: jwt::Claims<jwt::Actor>,
    binding: String,
    invocation: HostInvocation,
) -> anyhow::Result<Option<[u8; 0]>> {
    bail!(
        "cannot execute `{invocation:?}` within binding `{binding}` for actor `{}`",
        claims.subject
    )
}

fn new_runtime() -> Runtime {
    Runtime::from_host_handler(HandlerFunc::from(host_call)).expect("failed to construct runtime")
}

async fn run(wasm: impl AsRef<[u8]>) -> anyhow::Result<()> {
    let (wasm, key) = sign(
        wasm,
        "http_log_rng",
        [caps::HTTP_SERVER, caps::LOGGING, caps::NUMBERGEN],
    )
    .context("failed to sign Wasm")?;

    let rt = new_runtime();
    let actor = Actor::new(&rt, wasm).expect("failed to construct actor");
    assert_eq!(actor.claims().subject, key.public_key());

    let response = actor
        .call("HttpServer.HandleRequest", Some(REQUEST.as_slice()))
        .await
        .context("failed to call `HttpServer.HandleRequest`")?
        .expect("`HttpServer.HandleRequest` must not fail")
        .context("response missing")?;

    #[derive(Deserialize)]
    struct Response {
        guid: String,
        random_in_range: u32,
        // If this is truly random, we have nothing to assert here
        #[allow(dead_code)]
        random_32: u32,
    }

    let HttpResponse {
        status_code,
        header,
        body,
    } = deserialize(response.as_ref()).context("failed to deserialize response")?;
    ensure!(status_code == 200);
    ensure!(header.is_empty());

    let Response {
        guid,
        random_in_range,
        random_32: _,
    } = serde_json::from_slice(&body).context("failed to decode body as JSON")?;
    ensure!(Uuid::from_str(&guid).is_ok());
    ensure!(
        (42..=4242).contains(&random_in_range),
        "{random_in_range} should have been within range from 42 to 4242 inclusive"
    );
    Ok(())
}

#[tokio::test]
async fn actor_http_log_rng_module() -> anyhow::Result<()> {
    init();

    const WASM: &str = env!("CARGO_CDYLIB_FILE_ACTOR_HTTP_LOG_RNG_MODULE");
    let wasm = fs::read(WASM).await.context("failed to read binary")?;
    run(wasm).await
}

#[tokio::test]
async fn actor_http_log_rng_component() -> anyhow::Result<()> {
    init();

    const WASM: &str = env!("CARGO_CDYLIB_FILE_ACTOR_HTTP_LOG_RNG_COMPONENT");
    let wat = wat::parse_file(WASM).context("failed to parse binary")?;
    let wasm = encode_component(&wat, true)?;
    run(wasm).await
}
