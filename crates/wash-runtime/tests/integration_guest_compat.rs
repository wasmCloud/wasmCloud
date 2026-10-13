#![cfg(feature = "wasm_component_model_implements")]
//! A guest built on the async runtime wit-bindgen shipped before 0.59 serves
//! once the host repairs it at load. Each guest is also run unrepaired and
//! expected to trap, which is what shows it still exercises the bug.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::Subscriber;
use tracing_subscriber::layer::Context as LayerContext;
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::Registry;

use wash_runtime::engine::Engine;
use wash_runtime::engine::compat::{GuestCompat, legacy_cancel_sites};
use wash_runtime::host::http::{DevRouter, Ingress};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::plugin::wasi_blobstore::{InMemoryProvider, MultiplexedAsyncBlobstore};
use wash_runtime::types::{
    Component, LocalResources, Service, Workload, WorkloadStartRequest, WorkloadState,
};
use wash_runtime::wit::WitInterface;

mod common;
use common::{http_incoming_handler_interface, req};

/// Built from source against wit-bindgen 0.58.
const HTTP_LEGACY_ASYNC_P3_WASM: &[u8] = include_bytes!("wasm/http_legacy_async_p3.wasm");
/// A guest on the workspace's current wit-bindgen that awaits the same way.
const HTTP_SLEEPER_WASM: &[u8] = include_bytes!("wasm/http_sleeper.wasm");

const HOST: &str = "legacy";
/// Part of the message a trapped legacy guest's owner is logged, and of no other.
const ADVICE: &str = "wit-bindgen older than 0.59";

/// How a guest's bytes are deployed.
enum Role {
    Component { digest: Option<String> },
    Service { max_restarts: u64 },
}

/// Start `wasm` as an HTTP workload on a host whose engine uses `mode`.
async fn serve(
    mode: GuestCompat,
    wasm: bytes::Bytes,
    role: Role,
    extra_interfaces: Vec<WitInterface>,
) -> Result<(SocketAddr, impl HostApi)> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init();
    let engine = Engine::builder().with_guest_compat(mode).build()?;
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(
            MultiplexedAsyncBlobstore::new().with_provider(Arc::new(InMemoryProvider)),
        ))?
        .build()?
        .start()
        .await
        .context("failed to start host")?;

    let mut host_interfaces = vec![http_incoming_handler_interface(HOST, None)];
    host_interfaces.extend(extra_interfaces);
    let (service, components) = match role {
        Role::Component { digest } => (
            None,
            vec![Component {
                name: "legacy".to_string(),
                digest,
                bytes: wasm,
                ..Default::default()
            }],
        ),
        Role::Service { max_restarts } => (
            Some(Service {
                digest: None,
                bytes: wasm,
                local_resources: LocalResources::default(),
                max_restarts,
            }),
            vec![],
        ),
    };
    let started = host
        .workload_start(WorkloadStartRequest {
            workload_id: uuid::Uuid::new_v4().to_string(),
            workload: Workload {
                namespace: "test".to_string(),
                name: HOST.to_string(),
                annotations: HashMap::new(),
                service,
                components,
                host_interfaces,
                volumes: vec![],
            },
        })
        .await
        .context("workload_start call failed")?;
    assert_eq!(
        started.workload_status.workload_state,
        WorkloadState::Running,
        "{}",
        started.workload_status.message
    );
    Ok((addr, host))
}

fn legacy_fixture() -> bytes::Bytes {
    bytes::Bytes::from_static(HTTP_LEGACY_ASYNC_P3_WASM)
}

/// Records the message of every event logged on the thread it is the default
/// subscriber for.
#[derive(Clone, Default)]
struct Messages(Arc<Mutex<Vec<String>>>);

impl Messages {
    fn capture() -> (Self, tracing::subscriber::DefaultGuard) {
        let messages = Self::default();
        let guard = tracing::subscriber::set_default(Registry::default().with(messages.clone()));
        (messages, guard)
    }

    fn containing(&self, needle: &str) -> usize {
        let messages = self.0.lock().unwrap();
        messages.iter().filter(|m| m.contains(needle)).count()
    }
}

impl<S: Subscriber> tracing_subscriber::Layer<S> for Messages {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: LayerContext<'_, S>) {
        struct Message(String);
        impl tracing::field::Visit for Message {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0 = format!("{value:?}");
                }
            }
        }
        let mut message = Message(String::new());
        event.record(&mut message);
        self.0.lock().unwrap().push(message.0);
    }
}

#[tokio::test]
async fn a_legacy_guest_serves_once_repaired() -> Result<()> {
    // With and without a digest: the two are compiled by different branches.
    for digest in [None, Some("sha256:legacy".to_string())] {
        let role = Role::Component { digest };
        let (addr, _host) = serve(GuestCompat::Auto, legacy_fixture(), role, vec![]).await?;
        let client = reqwest::Client::new();
        // More than one, so a warm instance is woken again after its first call.
        for _ in 0..3 {
            let (status, body) = req(&client, &addr, HOST, "/").await?;
            assert_eq!((status.as_u16(), body.as_str()), (200, "awaited"));
        }
    }
    Ok(())
}

#[tokio::test]
async fn the_same_guest_traps_when_compiled_as_it_arrived() -> Result<()> {
    for mode in [GuestCompat::Detect, GuestCompat::Off] {
        let role = Role::Component { digest: None };
        let (addr, _host) = serve(mode, legacy_fixture(), role, vec![]).await?;
        let (status, _) = req(&reqwest::Client::new(), &addr, HOST, "/").await?;
        assert_eq!(
            status.as_u16(),
            500,
            "{mode:?}: the fixture no longer exercises the trap, so the test above proves nothing",
        );
    }
    Ok(())
}

#[tokio::test]
async fn an_unrepaired_component_is_told_to_rebuild_once() -> Result<()> {
    let (messages, _guard) = Messages::capture();
    let role = Role::Component { digest: None };
    let (addr, _host) = serve(GuestCompat::Off, legacy_fixture(), role, vec![]).await?;
    let client = reqwest::Client::new();
    for _ in 0..3 {
        let (status, _) = req(&client, &addr, HOST, "/").await?;
        assert_eq!(status.as_u16(), 500);
    }
    assert_eq!(messages.containing(ADVICE), 1);
    Ok(())
}

#[tokio::test]
async fn an_unrepaired_service_is_told_to_rebuild_once_across_restarts() -> Result<()> {
    let (messages, _guard) = Messages::capture();
    let role = Role::Service { max_restarts: 5 };
    let (addr, _host) = serve(GuestCompat::Off, legacy_fixture(), role, vec![]).await?;
    let client = reqwest::Client::new();
    // Each request traps the incarnation that serves it. A request that lands
    // while the service restarts fails some other way, so keep asking.
    let faulted = || messages.containing("trigger service faulted");
    for _ in 0..100 {
        if faulted() >= 2 {
            break;
        }
        let _ = req(&client, &addr, HOST, "/").await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(faulted() >= 2, "the service faulted {} times", faulted());
    assert_eq!(messages.containing(ADVICE), 1);
    Ok(())
}

#[test]
fn only_the_legacy_guest_has_sites_to_repair() {
    let legacy = legacy_cancel_sites(HTTP_LEGACY_ASYNC_P3_WASM).expect("the fixture parses");
    assert!(!legacy.is_empty(), "built on wit-bindgen 0.58");
    let current = legacy_cancel_sites(HTTP_SLEEPER_WASM).expect("the fixture parses");
    assert!(current.is_empty(), "built on the workspace's wit-bindgen");
}

/// The same repair, on a component a registry already held.
#[cfg(feature = "oci")]
mod published {
    use super::*;

    /// `ghcr.io/wasmcloud/components/oci-registry:0.1.0`, by digest. Built on
    /// wit-bindgen 0.58 and published before wasmtime enforced the trap.
    const PUBLISHED_LEGACY_REGISTRY: &str = "ghcr.io/wasmcloud/components/oci-registry@sha256:0d02f54a06f19d52655a2663a419e06ea59e3aa0e20bf0e2ba95f611dd9f20cc";

    fn blobstore_interface() -> WitInterface {
        WitInterface {
            namespace: "wasmcloud".to_string(),
            package: "blobstore".to_string(),
            interfaces: ["blobstore", "container", "types"]
                .map(str::to_string)
                .into_iter()
                .collect(),
            version: Some(semver::Version::new(0, 1, 0)),
            config: HashMap::new(),
            name: None,
        }
    }

    /// A blob and its digest, as an OCI client names it.
    const BLOB: &[u8] = b"published before the trap was enforced";
    const BLOB_DIGEST: &str =
        "sha256:906b53bdc8191b17951d25d4e3cacbb81853a8e042a38ca27a1d888ebe08d63d";

    /// Push [`BLOB`] to the registry in one request. The registry streams the body
    /// into `wasmcloud:blobstore`, so the call suspends on the host.
    async fn push_blob(addr: SocketAddr) -> Result<reqwest::StatusCode> {
        let pushed = reqwest::Client::new()
            .post(format!(
                "http://{addr}/v2/compat/blobs/uploads/?digest={BLOB_DIGEST}"
            ))
            .header("HOST", HOST)
            .body(BLOB)
            .send()
            .await?;
        Ok(pushed.status())
    }

    #[tokio::test]
    #[ignore = "pulls a published component from ghcr.io (network)"]
    async fn a_published_legacy_component_serves_once_repaired() -> Result<()> {
        let (wasm, digest) = wash_runtime::oci::pull_component(
            PUBLISHED_LEGACY_REGISTRY,
            wash_runtime::oci::OciConfig::default(),
            wash_runtime::oci::OciPullPolicy::IfNotPresent,
        )
        .await
        .context("failed to pull the published component")?;
        let wasm = bytes::Bytes::from(wasm);
        assert!(
            !legacy_cancel_sites(&wasm)?.is_empty(),
            "the published component no longer carries the legacy runtime"
        );

        let role = Role::Component { digest: None };
        let (addr, _host) = serve(
            GuestCompat::Off,
            wasm.clone(),
            role,
            vec![blobstore_interface()],
        )
        .await?;
        assert_eq!(
            push_blob(addr).await?.as_u16(),
            500,
            "the published component no longer exercises the trap",
        );

        let role = Role::Component {
            digest: Some(digest),
        };
        let (addr, _host) =
            serve(GuestCompat::Auto, wasm, role, vec![blobstore_interface()]).await?;
        assert_eq!(push_blob(addr).await?.as_u16(), 201);
        let (status, pulled) = req(
            &reqwest::Client::new(),
            &addr,
            HOST,
            &format!("/v2/compat/blobs/{BLOB_DIGEST}"),
        )
        .await?;
        assert_eq!((status.as_u16(), pulled.as_bytes()), (200, BLOB));
        Ok(())
    }
}
