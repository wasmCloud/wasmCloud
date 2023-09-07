use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::path::Path;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::Ordering;
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
use wasmcloud_runtime::capability::provider::{
    MemoryBlobstore, MemoryKeyValue, MemoryKeyValueEntry,
};
use wasmcloud_runtime::capability::{
    self, messaging, IncomingHttp, KeyValueAtomic, KeyValueReadWrite, Messaging,
};
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
    #[allow(unused)] // TODO: Verify resulting contents and remove
    blobstore: Arc<MemoryBlobstore>,
    keyvalue_atomic: Arc<MemoryKeyValue>,
    keyvalue_readwrite: Arc<MemoryKeyValue>,
    logging: Arc<Mutex<Vec<(logging::Level, String, String)>>>,
    messaging: Arc<Mutex<Vec<messaging::types::BrokerMessage>>>,
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
            ("", "foobar-component-command-preview2") => Ok(capability::TargetEntity::Actor(
                capability::ActorIdentifier::Alias("foobar-component-command-preview2".into()),
            )),
            ("", "unknown/alias") => Ok(capability::TargetEntity::Actor(
                capability::ActorIdentifier::Alias("unknown/alias".into()),
            )),
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
                (Some(capability::TargetEntity::Link(Some(name))), [capability::TargetInterface::WasiKeyvalueAtomic | capability::TargetInterface::WasiKeyvalueReadwrite]) if name == "keyvalue" => Ok(()),
                (Some(capability::TargetEntity::Link(Some(name))), [capability::TargetInterface::WasiBlobstoreBlobstore]) if name == "blobstore" => Ok(()),
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

            (
                Some(capability::TargetEntity::Link(Some(name))),
                "wasmcloud:keyvalue/KeyValue.Increment",
            ) if name == "keyvalue" => {
                let wasmcloud_compat::keyvalue::IncrementRequest { key, value } =
                    rmp_serde::from_slice(&payload).expect("failed to decode payload");
                let value = value.try_into().expect("value does not fit in `u64`");
                let new = self
                    .keyvalue_atomic
                    .increment("", key, value)
                    .await
                    .expect("failed to call `increment`");
                let new: i32 = new.try_into().expect("response does not fit in `u64`");
                let buf = rmp_serde::to_vec_named(&new).expect("failed to encode reply");
                Ok(buf)
            }

            (
                Some(capability::TargetEntity::Actor(capability::ActorIdentifier::Alias(name))),
                "test-actors:foobar/actor.foobar" // component invocation
                | "foobar-component-command-preview2/actor.foobar"  // valid module invocation
                | "unknown/alias/actor.foobar", // invalid module invocation
            ) if name == "foobar-component-command-preview2" || name == "unknown/alias" => {
                assert_eq!(payload, br#"{"arg":"foo"}"#);
                if name == "unknown/alias" {
                    bail!("unknown actor call alias")
                } else {
                    Ok(r#""foobar""#.into())
                }
            }

            (target, operation) => {
                panic!("`call_sync` with target `{target:?}` and operation `{operation}` should not have been called")
            }
        }
    }
}

fn new_runtime(
    blobstore: Arc<MemoryBlobstore>,
    keyvalue: Arc<MemoryKeyValue>,
    logs: Arc<Mutex<Vec<(logging::Level, String, String)>>>,
    published: Arc<Mutex<Vec<messaging::types::BrokerMessage>>>,
) -> Runtime {
    let handler = Arc::new(Handler {
        blobstore: Arc::clone(&blobstore),
        keyvalue_atomic: Arc::clone(&keyvalue),
        keyvalue_readwrite: Arc::clone(&keyvalue),
        logging: logs,
        messaging: published,
    });
    Runtime::builder()
        .bus(Arc::clone(&handler))
        .blobstore(Arc::clone(&blobstore))
        .keyvalue_atomic(Arc::clone(&keyvalue))
        .keyvalue_readwrite(Arc::clone(&keyvalue))
        .logging(Arc::clone(&handler))
        .messaging(Arc::clone(&handler))
        .build()
        .expect("failed to construct runtime")
}

async fn run(wasm: impl AsRef<Path>) -> anyhow::Result<Vec<(logging::Level, String, String)>> {
    let wasm = fs::read(wasm).await.context("failed to read Wasm")?;

    let keyvalue = Arc::new(MemoryKeyValue::from(HashMap::from([(
        "".into(),
        HashMap::from([("foo".into(), MemoryKeyValueEntry::Blob(b"bar".to_vec()))]),
    )])));
    let blobstore = Arc::default();
    let logs = Arc::default();
    let published = Arc::default();

    let res = {
        let rt = new_runtime(
            Arc::clone(&blobstore),
            Arc::clone(&keyvalue),
            Arc::clone(&logs),
            Arc::clone(&published),
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
            .uri("/foo?bar=baz")
            .header("accept", "*/*")
            .header("content-length", "21")
            .header("host", "fake:42")
            .header("test-header", "test-value")
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
    let mut keyvalue = HashMap::from(Arc::try_unwrap(keyvalue).unwrap()).into_iter();
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
            let mut kv = kv.into_iter().collect::<BTreeMap<_, _>>().into_iter();
            match (kv.next(), kv.next(), kv.next()) {
                (
                    Some((counter_key, MemoryKeyValueEntry::Atomic(counter_value))),
                    Some((result_key, MemoryKeyValueEntry::Blob(result_value))),
                    None,
                ) => {
                    ensure!(counter_key == "counter");
                    ensure!(counter_value.load(Ordering::Relaxed) == 42);
                    ensure!(result_key == "result");
                    result_value
                }
                (a, b, c) => bail!("invalid keyvalue map bucket entries ({a:?}, {b:?}, {c:?})"),
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
        #[allow(dead_code)]
        long_value: String,
    }
    let Response {
        get_random_bytes: _,
        get_random_u64: _,
        guid,
        random_32: _,
        random_in_range,
        long_value: _,
    } = serde_json::from_slice(&body).context("failed to decode body as JSON")?;
    ensure!(Uuid::from_str(&guid).is_ok());
    ensure!(
        (42..=4242).contains(&random_in_range),
        "{random_in_range} should have been within range from 42 to 4242 inclusive"
    );
    Ok(Arc::try_unwrap(logs).unwrap().into_inner())
}

#[tokio::test(flavor = "multi_thread")]
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

#[tokio::test(flavor = "multi_thread")]
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

#[tokio::test(flavor = "multi_thread")]
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
