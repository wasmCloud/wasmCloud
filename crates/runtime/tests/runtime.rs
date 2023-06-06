mod common;

use common::*;

use std::net::{Ipv6Addr, SocketAddr};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{ensure, Context};
use async_trait::async_trait;
use futures::lock::Mutex;
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::json;
use tokio::fs;
use tokio::io::{stderr, AsyncReadExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::spawn;
use wasmcloud_actor::{HttpRequest, HttpResponse, Uuid};
use wasmcloud_runtime::capability::{self, logging};
use wasmcloud_runtime::{Actor, Runtime};

static REQUEST: Lazy<Vec<u8>> = Lazy::new(|| {
    let body = serde_json::to_vec(&json!({
        "min": 42,
        "max": 4242,
    }))
    .expect("failed to encode body to JSON");
    rmp_serde::to_vec(&HttpRequest {
        body,
        ..Default::default()
    })
    .expect("failed to serialize request")
});

struct Logging(Arc<Mutex<Vec<(logging::Level, String, String)>>>);

#[async_trait]
impl capability::Logging for Logging {
    async fn log(
        &self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()> {
        self.0.lock().await.push((level, context, message));
        Ok(())
    }
}

fn new_runtime(logs: Arc<Mutex<Vec<(logging::Level, String, String)>>>) -> Runtime {
    Runtime::builder()
        .logging(Arc::new(Logging(logs)))
        .build()
        .expect("failed to construct runtime")
}

async fn run(wasm: impl AsRef<Path>) -> anyhow::Result<Vec<(logging::Level, String, String)>> {
    let wasm = fs::read(wasm).await.context("failed to read Wasm")?;
    let (wasm, key) = sign(
        wasm,
        "http_log_rng",
        [caps::HTTP_SERVER, caps::LOGGING, caps::NUMBERGEN],
    )
    .context("failed to sign Wasm")?;

    let socket = TcpListener::bind(SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)))
        .await
        .context("failed to bind on a socket")?;
    let addr = socket
        .local_addr()
        .context("failed to query local socket address")?;
    let response = spawn(async move {
        let (mut stream, _) = socket.accept().await.context("failed to accept")?;
        let mut buf = vec![];
        stream
            .read_to_end(&mut buf)
            .await
            .context("failed to read from stream")?;
        rmp_serde::from_slice(&buf).context("failed to deserialize response")
    });
    let output = TcpStream::connect(addr)
        .await
        .context("failed to connect to socket")?;
    let logs = Arc::new(vec![].into());
    {
        let rt = new_runtime(Arc::clone(&logs));
        let actor = Actor::new(&rt, wasm).expect("failed to construct actor");
        let claims = actor.claims().expect("claims missing");
        assert_eq!(claims.subject, key.public_key());
        let mut actor = actor.instantiate().await.context("failed to instantiate")?;
        actor
            .stderr(stderr())
            .await
            .context("failed to set stderr")?;
        actor
            .call("HttpServer.HandleRequest", REQUEST.as_slice(), output)
            .await
            .context("failed to call `HttpServer.HandleRequest`")?
            .expect("`HttpServer.HandleRequest` must not fail");
    }
    let HttpResponse {
        status_code,
        header,
        body,
    } = response.await??;

    ensure!(status_code == 200);
    ensure!(header.is_empty());

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    // NOTE: If values are truly random, we have nothing to assert for some of these fields
    struct Response {
        #[allow(dead_code)]
        get_random_bytes: [u8; 8],
        #[allow(dead_code)]
        get_random_u64: u64,
        guid: String,
        random_in_range: u32,
        #[allow(dead_code)]
        random_32: u32,
    }
    let Response {
        get_random_bytes: _,
        get_random_u64: _,
        guid,
        random_32: _,
        random_in_range,
    } = serde_json::from_slice(&body).context("failed to decode body as JSON")?;
    ensure!(Uuid::from_str(&guid).is_ok());
    ensure!(
        (42..=4242).contains(&random_in_range),
        "{random_in_range} should have been within range from 42 to 4242 inclusive"
    );
    Ok(Arc::try_unwrap(logs).unwrap().into_inner())
}

#[tokio::test]
async fn builtins_module() -> anyhow::Result<()> {
    init();

    let logs = run(test_actors::RUST_BUILTINS_MODULE_REACTOR).await?;
    assert_eq!(
        logs,
        vec![
            (
                logging::Level::Trace,
                "".into(),
                "context: trace-context; trace".into()
            ),
            (
                logging::Level::Debug,
                "".into(),
                "context: debug-context; debug".into()
            ),
            (
                logging::Level::Info,
                "".into(),
                "context: info-context; info".into()
            ),
            (
                logging::Level::Warn,
                "".into(),
                "context: warn-context; warn".into()
            ),
            (
                logging::Level::Error,
                "".into(),
                "context: error-context; error".into()
            ),
            (
                logging::Level::Trace,
                "".into(),
                "context: trace-context; trace".into()
            ),
            (
                logging::Level::Debug,
                "".into(),
                "context: debug-context; debug".into()
            ),
            (
                logging::Level::Info,
                "".into(),
                "context: info-context; info".into()
            ),
            (
                logging::Level::Warn,
                "".into(),
                "context: warn-context; warn".into()
            ),
            (
                logging::Level::Error,
                "".into(),
                "context: error-context; error".into()
            ),
            (logging::Level::Trace, "".into(), "trace".into()),
            (logging::Level::Debug, "".into(), "debug".into()),
            (logging::Level::Info, "".into(), "info".into()),
            (logging::Level::Warn, "".into(), "warn".into()),
            (logging::Level::Error, "".into(), "error".into()),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn builtins_compat() -> anyhow::Result<()> {
    init();

    let logs = run(test_actors::RUST_BUILTINS_COMPAT_REACTOR_PREVIEW2).await?;
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
            (logging::Level::Trace, "".into(), "trace".into()),
            (logging::Level::Debug, "".into(), "debug".into()),
            (logging::Level::Info, "".into(), "info".into()),
            (logging::Level::Warn, "".into(), "warn".into()),
            (logging::Level::Error, "".into(), "error".into()),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn builtins_component() -> anyhow::Result<()> {
    init();

    let logs = run(test_actors::RUST_BUILTINS_COMPONENT_REACTOR_PREVIEW2).await?;
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
            (logging::Level::Trace, "".into(), "trace".into()),
            (logging::Level::Debug, "".into(), "debug".into()),
            (logging::Level::Info, "".into(), "info".into()),
            (logging::Level::Warn, "".into(), "warn".into()),
            (logging::Level::Error, "".into(), "error".into()),
        ]
    );
    Ok(())
}
