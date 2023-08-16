use std::collections::HashMap;
use std::net::{Ipv6Addr, SocketAddr};
use std::path::Path;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, ensure, Context};
use async_trait::async_trait;
use futures::lock::Mutex;
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::json;
use tokio::fs;
use tokio::io::{stderr, AsyncReadExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::spawn;
use tracing_subscriber::prelude::*;
use wasmcloud_actor::{HttpRequest, HttpResponse, Uuid};
use wasmcloud_runtime::capability;
use wasmcloud_runtime::capability::logging::logging;
use wasmcloud_runtime::capability::messaging;
use wasmcloud_runtime::capability::provider::MemoryKeyValue;
use wasmcloud_runtime::{Actor, Runtime};

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

fn init() {
    _ = Lazy::force(&LOGGER);
}

static REQUEST: Lazy<Vec<u8>> = Lazy::new(|| {
    let body = serde_json::to_vec(&json!({
        "min": 42,
        "max": 4242,
    }))
    .expect("failed to encode body to JSON");
    rmp_serde::to_vec(&HttpRequest {
        method: "POST".into(),
        path: "/".into(),
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

struct Messaging(Arc<Mutex<Vec<messaging::types::BrokerMessage>>>);

#[async_trait]
impl capability::Messaging for Messaging {
    async fn request(
        &self,
        subject: String,
        body: Option<Vec<u8>>,
        timeout: Duration,
    ) -> anyhow::Result<messaging::types::BrokerMessage> {
        assert_eq!(subject, "test-messaging-request");
        assert_eq!(body.as_deref(), Some(b"foo".as_slice()));
        assert_eq!(timeout, Duration::from_millis(1000));
        Ok(messaging::types::BrokerMessage {
            subject,
            body: Some("bar".into()),
            reply_to: None,
        })
    }

    async fn request_multi(
        &self,
        subject: String,
        body: Option<Vec<u8>>,
        timeout: Duration,
        max_results: u32,
    ) -> anyhow::Result<Vec<messaging::types::BrokerMessage>> {
        assert_eq!(subject, "test-messaging-request-multi");
        assert_eq!(body.as_deref(), Some(b"foo".as_slice()));
        assert_eq!(timeout, Duration::from_millis(1000));
        assert_eq!(max_results, 1);
        Ok(vec![messaging::types::BrokerMessage {
            subject,
            body: Some("bar".into()),
            reply_to: None,
        }])
    }

    async fn publish(&self, msg: messaging::types::BrokerMessage) -> anyhow::Result<()> {
        self.0.lock().await.push(msg);
        Ok(())
    }
}

struct Bus;

#[async_trait]
impl capability::Bus for Bus {
    async fn identify_wasmbus_target(
        &self,
        _binding: &str,
        _namespace: &str,
    ) -> anyhow::Result<capability::TargetEntity> {
        panic!("should not be called")
    }

    async fn set_target(
        &self,
        _target: Option<capability::TargetEntity>,
        _interfaces: Vec<capability::TargetInterface>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn call(
        &self,
        _target: Option<capability::TargetEntity>,
        _operation: String,
    ) -> anyhow::Result<(
        Pin<Box<dyn futures::Future<Output = anyhow::Result<(), String>> + Send>>,
        Box<dyn tokio::io::AsyncWrite + Sync + Send + Unpin>,
        Box<dyn tokio::io::AsyncRead + Sync + Send + Unpin>,
    )> {
        panic!("should not be called")
    }
}

fn new_runtime(
    logs: Arc<Mutex<Vec<(logging::Level, String, String)>>>,
    published: Arc<Mutex<Vec<messaging::types::BrokerMessage>>>,
    keyvalue_readwrite: Arc<MemoryKeyValue>,
) -> Runtime {
    Runtime::builder()
        .bus(Arc::new(Bus))
        .logging(Arc::new(Logging(logs)))
        .messaging(Arc::new(Messaging(published)))
        .keyvalue_readwrite(Arc::clone(&keyvalue_readwrite))
        .build()
        .expect("failed to construct runtime")
}

async fn run(
    wasm: impl AsRef<Path>,
    interfaces: bool,
) -> anyhow::Result<Vec<(logging::Level, String, String)>> {
    let wasm = fs::read(wasm).await.context("failed to read Wasm")?;

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
    let published = Arc::new(vec![].into());
    let keyvalue_readwrite = Arc::new(MemoryKeyValue::from(HashMap::from([(
        "".into(),
        HashMap::from([("foo".into(), b"bar".to_vec())]),
    )])));
    {
        let rt = new_runtime(
            Arc::clone(&logs),
            Arc::clone(&published),
            Arc::clone(&keyvalue_readwrite),
        );
        let actor = Actor::new(&rt, wasm).expect("failed to construct actor");
        actor.claims().expect("claims missing");
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

    let mut published = Arc::try_unwrap(published).unwrap().into_inner().into_iter();
    let mut keyvalue = HashMap::from(Arc::try_unwrap(keyvalue_readwrite).unwrap()).into_iter();
    if interfaces {
        let published = match (published.next(), published.next()) {
            (
                Some(messaging::types::BrokerMessage {
                    subject,
                    reply_to,
                    body,
                }),
                None,
            ) => {
                ensure!(subject == "test-messaging-publish");
                ensure!(reply_to.as_deref() == Some("noreply"));
                body.context("body missing")?
            }
            (None, None) => bail!("no messages published"),
            _ => bail!("too many messages published"),
        };
        ensure!(body == published);

        let set = match (keyvalue.next(), keyvalue.next()) {
            (Some((bucket, kv)), None) => {
                ensure!(bucket == "");
                let mut kv = kv.into_iter();
                match (kv.next(), kv.next()) {
                    (Some((k, v)), None) => {
                        ensure!(k == "result");
                        v
                    }
                    _ => bail!("too many entries present in keyvalue map bucket"),
                }
            }
            _ => bail!("too many buckets present in keyvalue map"),
        };
        ensure!(
            body == set,
            "invalid keyvalue map `result` value:\ngot: {}\nexpected: {}",
            String::from_utf8_lossy(&set),
            String::from_utf8_lossy(&body),
        );
    }

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

    let logs = run(test_actors::RUST_BUILTINS_MODULE_REACTOR_SIGNED, false).await?;
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

    let logs = run(
        test_actors::RUST_BUILTINS_COMPAT_REACTOR_PREVIEW2_SIGNED,
        false,
    )
    .await?;
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

    let logs = run(
        test_actors::RUST_BUILTINS_COMPONENT_REACTOR_PREVIEW2_SIGNED,
        true,
    )
    .await?;
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
