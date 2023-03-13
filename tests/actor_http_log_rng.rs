use std::str::FromStr;

use anyhow::{bail, ensure, Context};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::json;
use tokio::fs;
use tracing_subscriber::prelude::*;
use wascap::prelude::{ClaimsBuilder, KeyPair};
use wascap::wasm::embed_claims;
use wascap::{caps, jwt};
use wasmbus_rpc::common::{deserialize, serialize};
use wasmcloud::capability::{self, HostHandlerBuilder, Uuid};
use wasmcloud::{ActorComponent, ActorModule, ActorModuleResponse, Runtime};
use wasmcloud_interface_httpserver::{HttpRequest, HttpResponse};
use wit_component::ComponentEncoder;

static LOGGER: Lazy<()> = Lazy::new(|| {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().pretty().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(
                    "info,integration=trace,wasmcloud=trace,cranelift_codegen=warn",
                )
            }),
        )
        .init();
});

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
    namespace: String,
    operation: String,
    payload: Option<Vec<u8>>,
) -> anyhow::Result<anyhow::Result<Option<[u8; 0]>>> {
    bail!(
        "cannot execute `{binding}.{namespace}.{operation}` with payload {payload:?} for actor `{}`",
        claims.subject
    )
}

fn new_runtime() -> Runtime<impl capability::Handler> {
    Runtime::builder(HostHandlerBuilder::new(host_call).build())
        .try_into()
        .expect("failed to construct runtime")
}

fn assert_response(response: Option<impl AsRef<[u8]>>) -> anyhow::Result<()> {
    let response = response.context("response missing")?;

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

fn sign(wasm: impl AsRef<[u8]>) -> anyhow::Result<(Vec<u8>, KeyPair)> {
    let issuer = KeyPair::new_account();
    let module = KeyPair::new_module();

    let claims = ClaimsBuilder::new()
        .issuer(&issuer.public_key())
        .subject(&module.public_key())
        .with_metadata(jwt::Actor {
            name: Some("http_log_rng".into()),
            caps: Some(vec![
                caps::HTTP_SERVER.into(),
                caps::LOGGING.into(),
                caps::NUMBERGEN.into(),
            ]),
            ..Default::default()
        })
        .build();
    let wasm =
        embed_claims(wasm.as_ref(), &claims, &issuer).context("failed to embed actor claims")?;
    Ok((wasm, module))
}

#[tokio::test]
async fn actor_http_log_rng_module() -> anyhow::Result<()> {
    _ = Lazy::force(&LOGGER);

    const WASM: &str = env!("CARGO_CDYLIB_FILE_ACTOR_HTTP_LOG_RNG_MODULE");
    let wasm = fs::read(WASM).await.expect("failed to read `{WASM}`");
    let (wasm, key) = sign(wasm).context("failed to sign module")?;

    let rt = new_runtime();
    let actor = ActorModule::new(&rt, wasm).expect("failed to read actor module");
    assert_eq!(actor.claims().subject, key.public_key());

    let mut actor = actor
        .instantiate()
        .await
        .expect("failed to instantiate actor module");

    let ActorModuleResponse {
        code,
        console_log,
        response,
    } = actor
        .call("HttpServer.HandleRequest", REQUEST.as_slice())
        .await
        .context("failed to call `HttpServer.HandleRequest`")?;
    assert_eq!(code, 1);
    assert!(console_log.is_empty());
    assert_response(response)
}

#[tokio::test]
async fn actor_http_log_rng_component() -> anyhow::Result<()> {
    _ = Lazy::force(&LOGGER);

    const WASM: &str = env!("CARGO_CDYLIB_FILE_ACTOR_HTTP_LOG_RNG_COMPONENT");
    let wasm = wat::parse_file(WASM).context("failed to parse binary")?;
    let wasm = ComponentEncoder::default()
        .validate(true)
        .module(&wasm)
        .context("failed to encode binary")?
        .adapter(
            "wasi_snapshot_preview1",
            include_bytes!("adapters/wasi_snapshot_preview1.reactor.wasm"),
        )
        .context("failed to add WASI adapter")?
        .encode()
        .context("failed to encode a component")?;
    let (wasm, key) = sign(wasm).context("failed to sign component")?;

    let rt = new_runtime();
    let actor = ActorComponent::new(&rt, wasm).expect("failed to read actor component");
    assert_eq!(actor.claims().subject, key.public_key());

    let mut actor = actor
        .instantiate()
        .await
        .expect("failed to instantiate actor component");

    let response = actor
        .call("HttpServer.HandleRequest", Some(REQUEST.as_slice()))
        .await
        .context("failed to call `HttpServer.HandleRequest`")?
        .expect("`HttpServer.HandleRequest` must not fail");
    assert_response(response)
}
