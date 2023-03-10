use std::str::FromStr;

use anyhow::Context;
use rand::thread_rng;
use serde::Deserialize;
use serde_json::json;
use tokio::fs;
use wasmbus_rpc::common::{deserialize, serialize};
use wasmbus_rpc::wascap::prelude::{ClaimsBuilder, KeyPair};
use wasmbus_rpc::wascap::wasm::embed_claims;
use wasmbus_rpc::wascap::{caps, jwt};
use wasmcloud::capability::{BuiltinHandler, LogLogging, RandNumbergen, Uuid};
use wasmcloud::{ActorInstanceConfig, ActorModule, ActorResponse, Runtime};
use wasmcloud_interface_httpserver::{HttpRequest, HttpResponse};

#[derive(Deserialize)]
struct Response {
    guid: String,
    random_in_range: u32,
    // If this is truly random, we have nothing to assert here
    #[allow(dead_code)]
    random_32: u32,
}

#[tokio::test]
async fn actor_http_log_rng() -> anyhow::Result<()> {
    const WASM: &str = env!("CARGO_CDYLIB_FILE_ACTOR_HTTP_LOG_RNG");
    let wasm = fs::read(WASM).await.expect("failed to read `{WASM}`");

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
    let wasm = embed_claims(&wasm, &claims, &issuer).expect("failed to embed actor claims");

    let rt = Runtime::builder(BuiltinHandler {
        logging: LogLogging::from(log::logger()),
        numbergen: RandNumbergen::from(thread_rng()),
        external: (),
    })
    .into();
    let actor = ActorModule::new(&rt, wasm).expect("failed to read actor module");

    assert_eq!(actor.claims().subject, module.public_key());

    let mut actor = actor
        .instantiate(ActorInstanceConfig::default())
        .expect("failed to instantiate actor");

    let body = serde_json::to_vec(&json!({
        "min": 42,
        "max": 4242,
    }))
    .expect("failed to encode body to JSON");
    let req = serialize(&HttpRequest {
        body,
        ..Default::default()
    })
    .expect("failed to serialize request");

    let ActorResponse {
        code,
        console_log,
        response,
    } = actor
        .call("HttpServer.HandleRequest", req.as_slice())
        .context("failed to call `HttpServer.HandleRequest`")?;
    assert_eq!(code, 1);
    assert!(console_log.is_empty());
    let HttpResponse {
        status_code,
        header,
        body,
    } = deserialize(&response.expect("response missing"))
        .context("failed to deserialize response")?;
    assert_eq!(status_code, 200);
    assert!(header.is_empty());

    let Response {
        guid,
        random_in_range,
        random_32: _,
    } = serde_json::from_slice(&body).context("failed to decode body as JSON")?;
    assert!(Uuid::from_str(&guid).is_ok());
    assert!((42..=4242).contains(&random_in_range));
    Ok(())
}
