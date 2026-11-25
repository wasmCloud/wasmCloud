#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::sync::{Arc, Mutex};
use std::{collections::HashMap, time::Duration};
use tokio::time::timeout;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use wasmtime_wasi_http::{RequestOptions, WasiBody};

use wash_runtime::{
    engine::{Engine, workload::ResolvedWorkload},
    host::{
        HostApi, HostBuilder,
        allowed_hosts::AllowedHost,
        egress_credential::{
            CredentialError, EgressCaller, EgressCredentialSource, EgressSecret, SecretForm,
        },
        http::{
            DynamicRouter, Ingress, IngressRoute, OutgoingHandler, RequestIoFuture, RouteError,
            Router, SendFuture,
        },
    },
    plugin::{wasi_config::DynamicConfig, wasi_logging::TracingLogger},
    types::{Component, LocalResources, Workload, WorkloadOwner, WorkloadStartRequest},
    wit::WitInterface,
};
use zeroize::Zeroizing;

const HTTP_ALLOWED_HOSTS_WASM: &[u8] = include_bytes!("wasm/http_allowed_hosts.wasm");

/// A sent request's URI and its `authorization` values.
type Sent = (String, Vec<String>);

#[derive(Clone, Default)]
struct RecordingOutgoing {
    sent: Arc<Mutex<Vec<Sent>>>,
}

impl OutgoingHandler for RecordingOutgoing {
    fn accepts_secrets(&self) -> bool {
        true
    }

    fn send_request(
        &self,
        _workload_id: &str,
        request: hyper::Request<WasiBody>,
        _options: Option<RequestOptions>,
        _fut: RequestIoFuture,
    ) -> SendFuture {
        let authorization = request
            .headers()
            .get_all(hyper::header::AUTHORIZATION)
            .iter()
            .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
            .collect();
        self.sent
            .lock()
            .unwrap()
            .push((request.uri().to_string(), authorization));
        Box::new(async move {
            let _ = request.into_body().collect().await;
            let body: WasiBody = Empty::<Bytes>::new()
                .map_err(|never| match never {})
                .boxed_unsync();
            let resp = hyper::Response::builder()
                .status(hyper::StatusCode::OK)
                .body(body)
                .expect("static response is well-formed");
            let io: RequestIoFuture = Box::new(async { Ok(()) });
            Ok((resp, io))
        })
    }
}

struct Fixed;

impl EgressCredentialSource for Fixed {
    fn current(&self) -> Result<Zeroizing<Vec<u8>>, CredentialError> {
        Ok(Zeroizing::new(b"host-secret-0123456789".to_vec()))
    }
}

const PLACEHOLDER: &str = "wash_ph_9fKq2Lm7Xv4Rt8Zb3Nd6Wc1Y";

/// Gives every component its `api.example.com` token by placeholder.
struct CredentialingRouter {
    inner: DynamicRouter,
    callers: Arc<Mutex<Vec<Caller>>>,
}

#[async_trait::async_trait]
impl Router for CredentialingRouter {
    async fn on_workload_resolved(
        &self,
        resolved_handle: &ResolvedWorkload,
        component_id: &str,
    ) -> anyhow::Result<()> {
        self.inner
            .on_workload_resolved(resolved_handle, component_id)
            .await
    }

    async fn on_workload_unbind(&self, workload_id: &str) -> anyhow::Result<()> {
        self.inner.on_workload_unbind(workload_id).await
    }

    async fn on_service_http_resolved(
        &self,
        workload_id: &str,
        routes: &[IngressRoute],
    ) -> anyhow::Result<()> {
        self.inner
            .on_service_http_resolved(workload_id, routes)
            .await
    }

    fn allow_outgoing_request(
        &self,
        workload_id: &str,
        request: &hyper::Request<WasiBody>,
        options: Option<RequestOptions>,
        allowed_hosts: &[AllowedHost],
    ) -> anyhow::Result<()> {
        self.inner
            .allow_outgoing_request(workload_id, request, options, allowed_hosts)
    }

    fn egress_secrets(&self, caller: EgressCaller<'_>) -> anyhow::Result<Arc<[EgressSecret]>> {
        self.callers.lock().unwrap().push((
            caller.component_id.map(str::to_string),
            caller.owner.cloned(),
        ));
        anyhow::ensure!(
            caller.namespace == "test" && caller.owner == Some(&deployment_owner()),
            "egress names its workload: {caller:?}"
        );
        let secret = EgressSecret::new(PLACEHOLDER, Arc::new(Fixed), SecretForm::Bearer)?
            .for_destinations(["https://api.example.com".parse()?])?;
        Ok(Arc::from([secret]))
    }

    fn route_incoming_request(
        &self,
        req: &hyper::Request<hyper::body::Incoming>,
    ) -> Result<String, RouteError> {
        self.inner.route_incoming_request(req)
    }

    fn route_local_egress(
        &self,
        uri: &hyper::Uri,
        can_serve: &mut dyn FnMut(&str) -> bool,
    ) -> Option<String> {
        self.inner.route_local_egress(uri, can_serve)
    }
}

fn deployment_owner() -> WorkloadOwner {
    WorkloadOwner::new(
        "WorkloadDeployment",
        "http-egress-credential",
        "deployment-uid",
    )
    .unwrap()
}

fn workload() -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            owner: Some(deployment_owner()),
            namespace: "test".to_string(),
            name: "http-egress-credential-7d9f-x2k".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "http-allowed-hosts.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_ALLOWED_HOSTS_WASM),
                local_resources: LocalResources {
                    memory_limit_mb: 128,
                    cpu_limit: 1,
                    config: HashMap::new(),
                    environment: HashMap::from([(
                        "API_TOKEN".to_string(),
                        PLACEHOLDER.to_string(),
                    )]),
                    volume_mounts: vec![],
                    allowed_hosts: vec![AllowedHost::Any].into(),
                    allowed_ip_name_lookups: Default::default(),
                    allowed_host_loopback_ports: Default::default(),
                },
                pool_size: 1,
                max_invocations: 100,
                max_concurrency: 1,
                ..Default::default()
            }],
            host_interfaces: vec![WitInterface {
                namespace: "wasi".to_string(),
                package: "http".to_string(),
                interfaces: ["incoming-handler".to_string()].into_iter().collect(),
                version: Some(semver::Version::parse("0.2.2").unwrap()),
                config: {
                    let mut config = wash_runtime::wit::Config::new();
                    config.insert("host".to_string(), "test".to_string());
                    config
                },
                name: None,
            }],
            volumes: vec![],
        },
    }
}

async fn get(addr: std::net::SocketAddr, path: &str) -> Result<(u16, String)> {
    get_with(addr, path, &[]).await
}

async fn get_with(
    addr: std::net::SocketAddr,
    path: &str,
    headers: &[(&str, &str)],
) -> Result<(u16, String)> {
    let mut request = reqwest::Client::new()
        .get(format!("http://{addr}{path}"))
        .header("HOST", "test");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = timeout(Duration::from_secs(10), request.send())
        .await
        .context("request timed out")??;
    Ok((response.status().as_u16(), response.text().await?))
}

/// A caller the router saw: its component and the owner it was named by.
type Caller = (Option<String>, Option<WorkloadOwner>);

struct Started {
    addr: std::net::SocketAddr,
    outgoing: RecordingOutgoing,
    callers: Arc<Mutex<Vec<Caller>>>,
    _host: Arc<wash_runtime::host::Host>,
}

async fn start(request: WorkloadStartRequest) -> Result<Started> {
    let outgoing = RecordingOutgoing::default();
    let callers = Arc::<Mutex<Vec<Caller>>>::default();
    let router = CredentialingRouter {
        inner: DynamicRouter::default(),
        callers: Arc::clone(&callers),
    };
    let ingress = Ingress::builder(router, "127.0.0.1:0".parse()?)
        .outgoing_handler(outgoing.clone())
        .build()
        .await?;
    let addr = ingress.addr();
    let host = HostBuilder::new()
        .with_engine(Engine::builder().build()?)
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(TracingLogger::default()))?
        .with_plugin(Arc::new(DynamicConfig::default()))?
        .build()?
        .start()
        .await?;
    host.workload_start(request).await?;
    Ok(Started {
        addr,
        outgoing,
        callers,
        _host: host,
    })
}

/// A real guest sends the placeholder from its environment with an ordinary
/// HTTP client, and the request leaves with the host's secret in its place.
/// The same placeholder sent to a host the secret is not for is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_placeholder_leaves_as_the_host_secret() -> Result<()> {
    let Started {
        addr,
        outgoing,
        callers,
        _host,
    } = start(workload()).await?;

    let (status, body) = get(addr, "/token").await?;
    assert_eq!(status, 200, "{body}");
    let (status, body) = get(addr, "/token-elsewhere").await?;
    assert_eq!(status, 403, "{body}");
    let (status, body) = get_with(addr, "/token", &[("x-forward-me", PLACEHOLDER)]).await?;
    assert_eq!(status, 400, "an inbound placeholder is refused: {body}");
    let (status, body) = get(addr, &format!("/token?k={PLACEHOLDER}")).await?;
    assert_eq!(status, 400, "an inbound placeholder is refused: {body}");

    let sent = outgoing.sent.lock().unwrap().clone();
    assert_eq!(
        sent,
        [(
            "https://api.example.com/v1".to_string(),
            vec!["Bearer host-secret-0123456789".to_string()]
        )]
    );
    let callers = callers.lock().unwrap().clone();
    assert_eq!(callers.len(), 2);
    assert!(
        callers
            .iter()
            .all(|(id, _)| id.as_deref().is_some_and(|id| !id.is_empty())),
        "{callers:?}"
    );
    Ok(())
}

/// The router sees the owner the host was started with and nothing else: a
/// workload that names the deployment and carries its annotations, but was
/// given no owner, is not the deployment's and gets none of its secrets.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_claimed_name_is_not_an_owner() -> Result<()> {
    let mut request = workload();
    request.workload.owner = None;
    request.workload.name = "http-egress-credential".to_string();
    request.workload.annotations = HashMap::from([
        (
            "runtime.wasmcloud.dev/workload-deployment".to_string(),
            "http-egress-credential".to_string(),
        ),
        (
            "app.kubernetes.io/name".to_string(),
            "http-egress-credential".to_string(),
        ),
    ]);
    let started = start(request).await?;

    let (status, body) = get(started.addr, "/token").await?;
    assert_ne!(status, 200, "{body}");
    assert!(started.outgoing.sent.lock().unwrap().is_empty());
    let callers = started.callers.lock().unwrap().clone();
    assert!(
        matches!(callers.as_slice(), [(_, None)]),
        "the router was named an owner: {callers:?}"
    );
    Ok(())
}
