use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, ensure, Context};
use async_trait::async_trait;
use futures::lock::Mutex;
use http_body_util::BodyExt as _;
use once_cell::sync::Lazy;
use serde::Deserialize;
use tokio::fs;
use tokio::io::stderr;
use tokio::sync::oneshot;
use tracing_subscriber::prelude::*;
use wasmcloud_actor::Uuid;
use wasmcloud_runtime::capability::logging::logging;
use wasmcloud_runtime::capability::provider::{
    MemoryBlobstore, MemoryKeyValue, MemoryKeyValueEntry,
};
use wasmcloud_runtime::capability::{
    self, guest_config, messaging, IncomingHttp, LatticeInterfaceTarget,
};
use wasmcloud_runtime::{Component, Runtime};
use wasmtime_wasi_http::body::HyperIncomingBody;

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

const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(600);

fn init() {
    _ = Lazy::force(&LOGGER);
}

struct Handler {
    #[allow(unused)] // TODO: Verify resulting contents and remove
    blobstore: Arc<MemoryBlobstore>,
    logging: Arc<Mutex<Vec<(logging::Level, String, String)>>>,
    messaging: Arc<Mutex<Vec<messaging::types::BrokerMessage>>>,
    outgoing_http: Arc<Mutex<Vec<wasmtime_wasi_http::types::OutgoingRequest>>>,
    config: HashMap<String, Vec<u8>>,
}

#[async_trait]
impl capability::Bus for Handler {
    async fn identify_interface_target(
        &self,
        interface: &capability::CallTargetInterface,
    ) -> Option<capability::TargetEntity> {
        match interface {
            capability::CallTargetInterface {
                namespace,
                package,
                interface: interface_name,
                ..
            } if namespace == "test-actors"
                && package == "foobar"
                && interface_name == "foobar" =>
            {
                Some(capability::TargetEntity::Lattice(LatticeInterfaceTarget {
                    id: "foobar-component-command-preview2".to_string(),
                    interface: interface.clone(),
                    link_name: "default".to_string(),
                }))
            }
            _ => panic!("interface `{interface:?}` not supported"),
        }
    }

    async fn set_link_name(
        &self,
        link_name: String,
        interfaces: Vec<capability::CallTargetInterface>,
    ) -> anyhow::Result<()> {
        match (link_name.as_ref(), interfaces.as_slice()) {
            ("messaging", [cti]) if cti.namespace == "wasmcloud" && cti.package == "messaging" && cti.interface == "consumer" => {},
            ("keyvalue", [cti]) if cti.namespace == "wasi" && cti.package == "keyvalue" && cti.interface == "atomic" => {},
            ("keyvalue", [cti]) if cti.namespace == "wasi" && cti.package == "keyvalue" && cti.interface == "eventual" => {},
            ("blobstore", [cti]) if cti.namespace == "wasi" && cti.package == "blobstore" && cti.interface == "blobstore" => {},
            ("httpclient", [cti]) if cti.namespace == "wasi" && cti.package == "http" && cti.interface == "outgoing-handler" => {},
            ("unknown/alias" | "foobar-component-command-preview2", [cti]) if cti.namespace == "test-actors" && cti.package == "foobar" && cti.interface ==
 "foobar" => {},
            (link_name, interfaces) => panic!("`set_link_name` with link name `{link_name:?}` and interfaces `{interfaces:?}` should not have been called")
        }
        Ok(())
    }

    async fn get(
        &self,
        key: &str,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, guest_config::ConfigError>> {
        Ok(Ok(self.config.get(key).cloned()))
    }

    async fn get_all(
        &self,
    ) -> anyhow::Result<Result<Vec<(String, Vec<u8>)>, guest_config::ConfigError>> {
        Ok(Ok(self.config.clone().into_iter().collect()))
    }

    async fn call(
        &self,
        target: capability::TargetEntity,
        instance: &str,
        name: &str,
        params: Vec<wrpc_transport::Value>,
    ) -> anyhow::Result<Vec<wrpc_transport::Value>> {
        match (target, instance, name) {
            (capability::TargetEntity::Lattice(LatticeInterfaceTarget { id: target_id, .. }), "test-actors:foobar/foobar", "foobar") if target_id == "foobar-component-command-preview2" => {
                let mut params = params.into_iter();
                match (params.next(), params.next()) {
                    (Some(wrpc_transport::Value::String(s)), None) => {
                        assert_eq!(s, "foo");
                        Ok(vec![wrpc_transport::Value::String("foobar".into())])
                    },
                    _ => bail!("invalid parameters received"),
                }
            },
            (target, instance, name) => panic!("`call` with target `{target:?}`, instance `{instance}` and name `{name}` should not have been called")
        }
    }
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
        body: Vec<u8>,
        timeout: Duration,
    ) -> anyhow::Result<messaging::types::BrokerMessage> {
        assert_eq!(subject, "test-messaging-request");
        assert_eq!(body, b"foo".as_slice());
        assert_eq!(timeout, Duration::from_millis(1000));
        Ok(messaging::types::BrokerMessage {
            subject,
            body: "bar".into(),
            reply_to: None,
        })
    }

    async fn publish(&self, msg: messaging::types::BrokerMessage) -> anyhow::Result<()> {
        self.messaging.lock().await.push(msg);
        Ok(())
    }
}

#[async_trait]
impl capability::OutgoingHttp for Handler {
    async fn handle(
        &self,
        request: wasmtime_wasi_http::types::OutgoingRequest,
    ) -> anyhow::Result<
        Result<
            http::Response<HyperIncomingBody>,
            wasmtime_wasi_http::bindings::http::types::ErrorCode,
        >,
    > {
        self.outgoing_http.lock().await.push(request);

        let body = http_body_util::Full::new("test".into())
            .map_err(|_| unreachable!())
            .boxed();
        let res = http::Response::builder()
            .status(200)
            .body(body)
            .expect("failed to build response");
        Ok(Ok(res))
    }
}

fn new_runtime(
    blobstore: Arc<MemoryBlobstore>,
    keyvalue: Arc<MemoryKeyValue>,
    logs: Arc<Mutex<Vec<(logging::Level, String, String)>>>,
    published: Arc<Mutex<Vec<messaging::types::BrokerMessage>>>,
    sent: Arc<Mutex<Vec<wasmtime_wasi_http::types::OutgoingRequest>>>,
    config: HashMap<String, Vec<u8>>,
) -> Runtime {
    let handler = Arc::new(Handler {
        blobstore: Arc::clone(&blobstore),
        logging: logs,
        messaging: published,
        outgoing_http: sent,
        config,
    });
    Runtime::builder()
        .bus(Arc::clone(&handler))
        .blobstore(Arc::clone(&blobstore))
        .keyvalue_atomic(Arc::clone(&keyvalue))
        .keyvalue_eventual(Arc::clone(&keyvalue))
        .logging(Arc::clone(&handler))
        .messaging(Arc::clone(&handler))
        .outgoing_http(Arc::clone(&handler))
        .build()
        .expect("failed to construct runtime")
}

struct RunResult {
    logs: Vec<(logging::Level, String, String)>,
    config_value: Vec<u8>,
    all_config: HashMap<String, Vec<u8>>,
}

async fn run(wasm: impl AsRef<Path>) -> anyhow::Result<RunResult> {
    const BODY: &str = r#"{"min":42,"max":4242,"port":42424,"config_key":"test-config-key"}"#;

    let wasm = fs::read(wasm).await.context("failed to read Wasm")?;

    let keyvalue = Arc::new(MemoryKeyValue::from(HashMap::from([(
        "".into(),
        HashMap::from([("foo".into(), MemoryKeyValueEntry::Blob(b"bar".to_vec()))]),
    )])));
    let blobstore = Arc::default();
    let logs = Arc::default();
    let published = Arc::default();
    let sent = Arc::default();
    let config = HashMap::from([
        ("test-config-key".to_string(), b"test-config-value".to_vec()),
        (
            "test-config-key2".to_string(),
            b"test-config-value2".to_vec(),
        ),
    ]);

    let res = {
        let rt = new_runtime(
            Arc::clone(&blobstore),
            Arc::clone(&keyvalue),
            Arc::clone(&logs),
            Arc::clone(&published),
            Arc::clone(&sent),
            config.clone(),
        );
        let actor = Component::new(&rt, wasm).expect("failed to construct actor");
        actor.claims().expect("claims missing");
        let mut actor = actor.instantiate().context("failed to instantiate")?;
        actor
            .stderr(stderr())
            .await
            .context("failed to set stderr")?;
        let body = http_body_util::Full::new(BODY.into())
            .map_err(|_| unreachable!())
            .boxed();
        let req = http::Request::builder()
            .method("POST")
            .uri("/foo?bar=baz")
            .header("accept", "*/*")
            .header("content-length", BODY.len())
            .header("host", "fake:42")
            .header("test-header", "test-value")
            .body(body)
            .expect("failed to construct request");
        let (tx, rx) = oneshot::channel();
        actor
            .into_incoming_http()
            .await
            .context("failed to instantiate `wasi:http/incoming-handler`")?
            .handle(req, tx)
            .await
            .context("failed to call `wasi:http/incoming-handler.handle`")?;
        rx.await.context("response not set")?
    };
    let res = res.context("request failed")?;
    let (
        http::response::Parts {
            status, headers, ..
        },
        body,
    ) = res.into_parts();
    ensure!(status.as_u16() == 200);
    ensure!(headers.is_empty());
    let body = body
        .collect()
        .await
        .context("failed to read response body")?;
    let body = body.to_bytes();

    let mut published = Arc::try_unwrap(published).unwrap().into_inner().into_iter();
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
            body
        }
        (None, None) => bail!("no messages published"),
        _ => bail!("too many messages published"),
    };
    ensure!(body == published);

    let mut sent = Arc::try_unwrap(sent).unwrap().into_inner().into_iter();
    match (sent.next(), sent.next()) {
        (
            Some(wasmtime_wasi_http::types::OutgoingRequest {
                use_tls,
                authority,
                request,
                connect_timeout,
                first_byte_timeout,
                between_bytes_timeout,
            }),
            None,
        ) => {
            ensure!(!use_tls);
            ensure!(authority == format!("localhost:42424"));
            ensure!(connect_timeout == DEFAULT_HTTP_TIMEOUT);
            ensure!(first_byte_timeout == DEFAULT_HTTP_TIMEOUT);
            ensure!(between_bytes_timeout == DEFAULT_HTTP_TIMEOUT);
            ensure!(request.method() == http::Method::PUT);
            ensure!(*request.uri() == *format!("http://localhost:42424/test"));
            let body = request
                .into_body()
                .collect()
                .await
                .context("failed to read request body")?;
            ensure!(body.to_bytes() == "test");
        }
        (None, None) => bail!("no messages published"),
        _ => bail!("too many messages published"),
    };
    ensure!(body == published);

    let mut keyvalue = HashMap::from(Arc::try_unwrap(keyvalue).unwrap()).into_iter();
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
        config_value: Vec<u8>,
        all_config: Vec<(String, Vec<u8>)>,
    }
    let Response {
        get_random_bytes: _,
        get_random_u64: _,
        guid,
        random_32: _,
        random_in_range,
        long_value: _,
        config_value,
        all_config,
    } = serde_json::from_slice(&body).context("failed to decode body as JSON")?;
    ensure!(Uuid::from_str(&guid).is_ok());
    ensure!(
        (42..=4242).contains(&random_in_range),
        "{random_in_range} should have been within range from 42 to 4242 inclusive"
    );
    Ok(RunResult {
        logs: Arc::try_unwrap(logs).unwrap().into_inner(),
        config_value,
        all_config: all_config.into_iter().collect(),
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn builtins() -> anyhow::Result<()> {
    init();

    let RunResult {
        logs,
        config_value,
        all_config,
    } = run(test_actors::RUST_BUILTINS_COMPONENT_REACTOR_PREVIEW2_SIGNED).await?;
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
    ensure!(
        config_value == b"test-config-value",
        "should have returned the correct config value"
    );
    ensure!(
        all_config.into_iter().collect::<HashMap<_, _>>()
            == HashMap::from([
                ("test-config-key".to_string(), b"test-config-value".to_vec()),
                (
                    "test-config-key2".to_string(),
                    b"test-config-value2".to_vec(),
                ),
            ]),
        "should have returned all config values"
    );
    Ok(())
}
