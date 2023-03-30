mod common;

use common::*;

use std::str::FromStr;
use std::sync::Arc;

use anyhow::{ensure, Context};
use async_trait::async_trait;
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::json;
use test_actors::encode_component;
use tokio::sync::Mutex;
use wasmcloud_actor::Uuid;
use wasmcloud_host::capability::logging;
use wasmcloud_host::{Actor, Runtime};
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

struct Logging(Arc<Mutex<Vec<(logging::Level, String, String)>>>);

#[async_trait]
impl logging::Host for Logging {
    async fn log(
        &mut self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()> {
        match level {
            logging::Level::Trace => assert_eq!(message, "trace"),
            logging::Level::Debug => assert_eq!(message, "debug"),
            logging::Level::Info => assert_eq!(message, "info"),
            logging::Level::Warn => assert_eq!(message, "warn"),
            logging::Level::Error => assert_eq!(message, "error"),
        }
        self.0.lock().await.push((level, context, message));
        Ok(())
    }
}

fn new_runtime(logs: Arc<Mutex<Vec<(logging::Level, String, String)>>>) -> Runtime {
    Runtime::builder()
        .logging(Logging(logs))
        .build()
        .expect("failed to construct runtime")
}

async fn run(wasm: impl AsRef<[u8]>) -> anyhow::Result<Vec<(logging::Level, String, String)>> {
    let (wasm, key) = sign(
        wasm,
        "http_log_rng",
        [caps::HTTP_SERVER, caps::LOGGING, caps::NUMBERGEN],
    )
    .context("failed to sign Wasm")?;

    let logs = Arc::new(vec![].into());
    let response = {
        let rt = new_runtime(Arc::clone(&logs));
        let actor = Actor::new(&rt, wasm).expect("failed to construct actor");
        assert_eq!(actor.claims().subject, key.public_key());
        actor
            .configure()
            .inherit_stdout()
            .inherit_stderr()
            .call("HttpServer.HandleRequest", Some(REQUEST.as_slice()))
            .await
            .context("failed to call `HttpServer.HandleRequest`")?
            .expect("`HttpServer.HandleRequest` must not fail")
            .context("response missing")?
    };

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
    Ok(Arc::try_unwrap(logs).unwrap().into_inner())
}

#[tokio::test]
async fn actor_http_log_rng_module() -> anyhow::Result<()> {
    init();

    let logs = run(test_actors::RUST_HTTP_LOG_RNG_MODULE).await?;
    assert_eq!(
        logs,
        vec![
            (logging::Level::Debug, "".into(), "debug".into()),
            (logging::Level::Info, "".into(), "info".into()),
            (logging::Level::Warn, "".into(), "warn".into()),
            (logging::Level::Error, "".into(), "error".into()),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn actor_http_log_rng_component() -> anyhow::Result<()> {
    init();
    let wasm = encode_component(test_actors::RUST_HTTP_LOG_RNG_COMPONENT, true)?;
    let logs = run(wasm).await?;
    assert_eq!(
        logs,
        vec![
            (
                logging::Level::Trace,
                "trace-context".into(),
                "trace".into()
            ),
            (
                logging::Level::Debug,
                "debug-context".into(),
                "debug".into()
            ),
            (logging::Level::Info, "info-context".into(), "info".into()),
            (logging::Level::Warn, "warn-context".into(), "warn".into()),
            (
                logging::Level::Error,
                "error-context".into(),
                "error".into()
            ),
        ]
    );
    Ok(())
}
