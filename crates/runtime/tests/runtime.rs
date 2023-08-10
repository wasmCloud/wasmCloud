use std::collections::HashMap;
use std::io::Cursor;
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
use tokio::fs;
use tokio::io::{stderr, AsyncRead, AsyncReadExt};
use tracing_subscriber::prelude::*;
use wasmcloud_actor::Uuid;
use wasmcloud_runtime::capability::logging::logging;
use wasmcloud_runtime::capability::provider::MemoryKeyValue;
use wasmcloud_runtime::capability::{self, messaging, IncomingHttp, KeyValueReadWrite, Messaging};
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

struct Handler {
    logging: Arc<Mutex<Vec<(logging::Level, String, String)>>>,
    messaging: Arc<Mutex<Vec<messaging::types::BrokerMessage>>>,
    keyvalue_readwrite: Arc<MemoryKeyValue>,
}

#[async_trait]
impl capability::Logging for Handler {
    async fn log(
        &self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()> {
        self.logging.lock().await.push((level, context, message));
        Ok(())
    }
}

#[async_trait]
impl capability::Messaging for Handler {
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
        self.messaging.lock().await.push(msg);
        Ok(())
    }
}

#[async_trait]
impl capability::Bus for Handler {
    async fn identify_wasmbus_target(
        &self,
        binding: &str,
        namespace: &str,
    ) -> anyhow::Result<capability::TargetEntity> {
        match (binding, namespace) {
            ("messaging", "wasmcloud:messaging") => {
                Ok(capability::TargetEntity::Link(Some("messaging".into())))
            }
            ("keyvalue", "wasmcloud:keyvalue") => {
                Ok(capability::TargetEntity::Link(Some("keyvalue".into())))
            }
            _ => panic!("binding `{binding}` namespace `{namespace}` pair not supported"),
        }
    }

    async fn set_target(
        &self,
        target: Option<capability::TargetEntity>,
        interfaces: Vec<capability::TargetInterface>,
    ) -> anyhow::Result<()> {
        match (target, interfaces.as_slice()) {
            (Some(capability::TargetEntity::Link(Some(name))), [capability::TargetInterface::WasmcloudMessagingConsumer]) if name == "messaging" => Ok(()),
                (Some(capability::TargetEntity::Link(Some(name))), [capability::TargetInterface::WasiKeyvalueReadwrite]) if name == "keyvalue" => Ok(()),
            (target, interfaces) => panic!("`set_target` with target `{target:?}` and interfaces `{interfaces:?}` should not have been called")
        }
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
        panic!("should not have been called")
    }

    async fn call_sync(
        &self,
        target: Option<capability::TargetEntity>,
        operation: String,
        payload: Vec<u8>,
    ) -> anyhow::Result<Vec<u8>> {
        // TODO: Migrate this translation layer to `runtime` crate once we switch to WIT-enabled providers
        match (target, operation.as_str()) {
            (
                Some(capability::TargetEntity::Link(Some(name))),
                "wasmcloud:messaging/Messaging.Publish",
            ) if name == "messaging" => {
                let wasmcloud_compat::messaging::PubMessage {
                    subject,
                    reply_to,
                    body,
                } = rmp_serde::from_slice(&payload).expect("failed to decode payload");
                self.publish(messaging::types::BrokerMessage {
                    subject,
                    reply_to,
                    body: Some(body),
                })
                .await
                .expect("failed to publish message");
                Ok(vec![])
            }
            (
                Some(capability::TargetEntity::Link(Some(name))),
                "wasmcloud:messaging/Messaging.Request",
            ) if name == "messaging" => {
                let wasmcloud_compat::messaging::RequestMessage {
                    subject,
                    body,
                    timeout_ms,
                } = rmp_serde::from_slice(&payload).expect("failed to decode payload");
                let messaging::types::BrokerMessage {
                    subject,
                    body,
                    reply_to,
                } = match subject.as_str() {
                    "test-messaging-request" => self
                        .request(
                            subject,
                            Some(body),
                            Duration::from_millis(timeout_ms.into()),
                        )
                        .await
                        .expect("failed to call `request`"),
                    "test-messaging-request-multi" => self
                        .request_multi(
                            subject,
                            Some(body),
                            Duration::from_millis(timeout_ms.into()),
                            1,
                        )
                        .await
                        .expect("failed to call `request_multi`")
                        .pop()
                        .expect("first element missing"),
                    _ => panic!("invalid subject `{subject}`"),
                };
                let buf = rmp_serde::to_vec_named(&wasmcloud_compat::messaging::ReplyMessage {
                    subject,
                    reply_to,
                    body: body.unwrap_or_default(),
                })
                .expect("failed to encode reply");
                Ok(buf)
            }

            (
                Some(capability::TargetEntity::Link(Some(name))),
                "wasmcloud:keyvalue/KeyValue.Set",
            ) if name == "keyvalue" => {
                let wasmcloud_compat::keyvalue::SetRequest {
                    key,
                    value,
                    expires,
                } = rmp_serde::from_slice(&payload).expect("failed to decode payload");
                assert_eq!(expires, 0);
                self.keyvalue_readwrite
                    .set("", key, Box::new(Cursor::new(value)))
                    .await
                    .expect("failed to call `set`");
                Ok(vec![])
            }

            (
                Some(capability::TargetEntity::Link(Some(name))),
                "wasmcloud:keyvalue/KeyValue.Get",
            ) if name == "keyvalue" => {
                let key = rmp_serde::from_slice(&payload).expect("failed to decode payload");
                let (mut reader, _) = self
                    .keyvalue_readwrite
                    .get("", key)
                    .await
                    .expect("failed to call `get`");
                let mut value = String::new();
                reader
                    .read_to_string(&mut value)
                    .await
                    .expect("failed to read value");
                let buf = rmp_serde::to_vec_named(&wasmcloud_compat::keyvalue::GetResponse {
                    exists: true,
                    value,
                })
                .expect("failed to encode reply");
                Ok(buf)
            }

            (
                Some(capability::TargetEntity::Link(Some(name))),
                "wasmcloud:keyvalue/KeyValue.Contains",
            ) if name == "keyvalue" => {
                let key = rmp_serde::from_slice(&payload).expect("failed to decode payload");
                let ok = self
                    .keyvalue_readwrite
                    .exists("", key)
                    .await
                    .expect("failed to call `exists`");
                let buf = rmp_serde::to_vec_named(&ok).expect("failed to encode reply");
                Ok(buf)
            }

            (
                Some(capability::TargetEntity::Link(Some(name))),
                "wasmcloud:keyvalue/KeyValue.Del",
            ) if name == "keyvalue" => {
                let key = rmp_serde::from_slice(&payload).expect("failed to decode payload");
                self.keyvalue_readwrite
                    .delete("", key)
                    .await
                    .expect("failed to call `delete`");
                let buf = rmp_serde::to_vec_named(&true).expect("failed to encode reply");
                Ok(buf)
            }

            (target, operation) => {
                panic!("`call_sync` with target `{target:?}` and operation `{operation}` should not have been called")
            }
        }
    }
}

fn new_runtime(
    logs: Arc<Mutex<Vec<(logging::Level, String, String)>>>,
    published: Arc<Mutex<Vec<messaging::types::BrokerMessage>>>,
    keyvalue_readwrite: Arc<MemoryKeyValue>,
) -> Runtime {
    let handler = Arc::new(Handler {
        logging: logs,
        messaging: published,
        keyvalue_readwrite: Arc::clone(&keyvalue_readwrite),
    });
    Runtime::builder()
        .bus(Arc::clone(&handler))
        .logging(Arc::clone(&handler))
        .messaging(Arc::clone(&handler))
        .keyvalue_readwrite(Arc::clone(&keyvalue_readwrite))
        .build()
        .expect("failed to construct runtime")
}

async fn run(wasm: impl AsRef<Path>) -> anyhow::Result<Vec<(logging::Level, String, String)>> {
    let wasm = fs::read(wasm).await.context("failed to read Wasm")?;

    let logs = Arc::new(vec![].into());
    let published = Arc::new(vec![].into());
    let keyvalue_readwrite = Arc::new(MemoryKeyValue::from(HashMap::from([(
        "".into(),
        HashMap::from([("foo".into(), b"bar".to_vec())]),
    )])));

    let res = {
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
        let req: Box<dyn AsyncRead + Send + Sync + Unpin> =
            Box::new(Cursor::new(r#"{"min":42,"max":4242}"#));
        let req = http::Request::builder()
            .method("POST")
            .body(req)
            .expect("failed to construct request");
        actor
            .into_incoming_http()
            .await
            .context("failed to instantiate `wasi:http/incoming-handler`")?
            .handle(req)
            .await
            .context("failed to call `wasi:http/incoming-handler.handle`")?
    };
    let (
        http::response::Parts {
            status, headers, ..
        },
        mut body,
    ) = res.into_parts();
    ensure!(status.as_u16() == 200);
    ensure!(headers.is_empty());
    let body = {
        let mut buf = vec![];
        body.read_to_end(&mut buf)
            .await
            .context("failed to read response body")?;
        buf
    };

    let mut published = Arc::try_unwrap(published).unwrap().into_inner().into_iter();
    let mut keyvalue = HashMap::from(Arc::try_unwrap(keyvalue_readwrite).unwrap()).into_iter();
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

    let logs = run(test_actors::RUST_BUILTINS_MODULE_REACTOR_SIGNED).await?;
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

    let logs = run(test_actors::RUST_BUILTINS_COMPAT_REACTOR_PREVIEW2_SIGNED).await?;
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

    let logs = run(test_actors::RUST_BUILTINS_COMPONENT_REACTOR_PREVIEW2_SIGNED).await?;
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
