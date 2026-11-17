#![cfg(feature = "wasm_component_model_implements")]
//! A real P3 guest reaching out only through `wasmcloud:broker` imports, one
//! per credential, with every key and token held by the host.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use tokio::time::timeout;
use wasmtime_wasi_http::{RequestOptions, WasiBody};
use zeroize::Zeroizing;

use wash_runtime::{
    engine::{Engine, workload::ResolvedWorkload},
    host::{
        HostApi, HostBuilder,
        allowed_hosts::AllowedHost,
        egress_credential::{EgressCaller, EgressCredentialSource, LeasedSource, SecretRoute},
        http::{
            DevRouter, HostHandler, Ingress, IngressRoute, OutgoingHandler, RequestIoFuture,
            RouteError, Router, SendFuture,
        },
    },
    plugin::wasmcloud_broker::{
        BrokerCaller, BrokerOutcome, BrokerPolicy, BrokerUse, ClientBinding, Presenter, SignerKey,
        VerifierKey, WasmcloudBroker,
    },
    types::{Component, LocalResources, Workload, WorkloadStartRequest, WorkloadState},
    wit::WitInterface,
};

mod common;
use common::http_incoming_handler_interface;

const BROKER_CLIENT_P3_WASM: &[u8] = include_bytes!("wasm/broker_client_p3.wasm");
const WEBHOOK_SECRET: &[u8] = b"whsec_test";

/// A request URI and one header of it.
type Sent = (String, Option<String>);

/// Answers every request with its own body, recording what was sent.
#[derive(Clone, Default)]
struct Echo {
    sent: Arc<Mutex<Vec<Sent>>>,
}

impl OutgoingHandler for Echo {
    fn send_request(
        &self,
        _workload_id: &str,
        request: hyper::Request<WasiBody>,
        _options: Option<RequestOptions>,
        _fut: RequestIoFuture,
    ) -> SendFuture {
        let authorization = request
            .headers()
            .get(hyper::header::AUTHORIZATION)
            .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
        self.sent
            .lock()
            .unwrap()
            .push((request.uri().to_string(), authorization));
        Box::new(async move {
            let body = request
                .into_body()
                .collect()
                .await
                .map_err(|e| wasmtime_wasi_http::Error::InternalError(Some(e.to_string())))?
                .to_bytes();
            let body: WasiBody = Full::new(body)
                .map_err(|never| match never {})
                .boxed_unsync();
            let response = hyper::Response::builder()
                .status(201)
                .body(body)
                .expect("static response is well-formed");
            let io: RequestIoFuture = Box::new(async { Ok(()) });
            Ok((response, io))
        })
    }
}

/// Records each brokered request it is asked about, refusing them on `veto`.
struct Gate {
    inner: DevRouter,
    veto: bool,
    brokered: Arc<Mutex<Vec<Sent>>>,
}

#[async_trait::async_trait]
impl Router for Gate {
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

    fn allow_brokered_request(
        &self,
        caller: EgressCaller<'_>,
        request: &hyper::Request<WasiBody>,
    ) -> anyhow::Result<()> {
        self.brokered.lock().unwrap().push((
            request.uri().to_string(),
            caller.component_id.map(str::to_string),
        ));
        anyhow::ensure!(!self.veto, "brokered egress is switched off");
        Ok(())
    }

    fn route_incoming_request(
        &self,
        req: &hyper::Request<hyper::body::Incoming>,
    ) -> Result<String, RouteError> {
        self.inner.route_incoming_request(req)
    }
}

struct Policy {
    bind_github: bool,
    destination: (String, u16),
    signer: Arc<SignerKey>,
    uses: Mutex<Vec<String>>,
    callers: Mutex<Vec<BrokerCaller>>,
}

impl Policy {
    fn new(bind_github: bool) -> Self {
        Self {
            bind_github,
            destination: ("api.example.com".to_string(), 443),
            signer: Arc::new(SignerKey::generate_ecdsa_p256().unwrap()),
            uses: Mutex::default(),
            callers: Mutex::default(),
        }
    }

    fn to(mut self, host: &str, port: u16) -> Self {
        self.destination = (host.to_string(), port);
        self
    }
}

impl BrokerPolicy for Policy {
    fn client(&self, caller: &BrokerCaller, label: &str) -> Option<Arc<ClientBinding>> {
        self.callers.lock().unwrap().push(caller.clone());
        let token = LeasedSource::default();
        token.renew(
            Zeroizing::new(b"gh-token".to_vec()),
            Duration::from_secs(300),
        );
        let token: Arc<dyn EgressCredentialSource> = Arc::new(token);
        (self.bind_github && label == "github").then(|| {
            Arc::new(
                ClientBinding::new(Presenter::Bearer(token))
                    .to(&self.destination.0, self.destination.1)
                    .on(SecretRoute::new(None, "/v1").unwrap()),
            )
        })
    }

    fn signer(&self, _caller: &BrokerCaller, label: &str) -> Option<Arc<SignerKey>> {
        (label == "release").then(|| Arc::clone(&self.signer))
    }

    fn verifier(&self, _caller: &BrokerCaller, label: &str) -> Option<Arc<VerifierKey>> {
        (label == "webhook").then(|| Arc::new(VerifierKey::hmac_sha256(WEBHOOK_SECRET)))
    }

    fn on_use(&self, event: &BrokerUse<'_>) {
        let outcome = match &event.outcome {
            BrokerOutcome::Completed => "completed".to_string(),
            BrokerOutcome::Denied(reason) => format!("denied {reason}"),
            BrokerOutcome::Indeterminate(detail) => format!("indeterminate {detail}"),
        };
        self.uses
            .lock()
            .unwrap()
            .push(format!("{} {outcome}", event.label));
    }
}

fn broker_import(interface: &str, label: &str) -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "broker".to_string(),
        interfaces: [interface.to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.1.0").unwrap()),
        config: Default::default(),
        name: Some(label.to_string()),
    }
}

fn workload() -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "broker-client".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "broker-client-p3".to_string(),
                digest: None,
                bytes: Bytes::from_static(BROKER_CLIENT_P3_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
                max_concurrency: 1,
                ..Default::default()
            }],
            host_interfaces: vec![
                http_incoming_handler_interface("broker", None),
                broker_import("client", "github"),
                broker_import("signer", "release"),
                broker_import("verifier", "webhook"),
            ],
            volumes: vec![],
        },
    }
}

struct Running {
    addr: std::net::SocketAddr,
    echo: Echo,
    brokered: Arc<Mutex<Vec<Sent>>>,
    policy: Arc<Policy>,
    state: WorkloadState,
    message: String,
    _host: Arc<wash_runtime::host::Host>,
}

async fn start(policy: Policy) -> Result<Running> {
    start_with(policy, false, true).await
}

/// `echo` false sends over the real network path, address gate included.
async fn start_with(policy: Policy, veto: bool, echo_upstream: bool) -> Result<Running> {
    let echo = Echo::default();
    let brokered = Arc::default();
    let gate = Gate {
        inner: DevRouter::default(),
        veto,
        brokered: Arc::clone(&brokered),
    };
    let builder = Ingress::builder(gate, "127.0.0.1:0".parse()?);
    let (addr, ingress): (_, Arc<dyn HostHandler>) = if echo_upstream {
        let ingress = builder.outgoing_handler(echo.clone()).build().await?;
        (ingress.addr(), Arc::new(ingress))
    } else {
        let ingress = builder.build().await?;
        (ingress.addr(), Arc::new(ingress))
    };
    let policy = Arc::new(policy);
    let host = HostBuilder::new()
        .with_engine(Engine::builder().build()?)
        .with_http_handler(ingress)
        .with_plugin(Arc::new(WasmcloudBroker::new(
            Arc::clone(&policy) as Arc<dyn BrokerPolicy>
        )))?
        .build()?
        .start()
        .await
        .context("failed to start host")?;
    let response = host.workload_start(workload()).await?;
    Ok(Running {
        addr,
        echo,
        brokered,
        policy,
        state: response.workload_status.workload_state,
        message: response.workload_status.message,
        _host: host,
    })
}

async fn call(
    addr: std::net::SocketAddr,
    path: &str,
    headers: &[(&str, &str)],
    body: impl Into<reqwest::Body>,
) -> Result<(u16, String)> {
    let mut request = reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .header("HOST", "broker")
        .body(body);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = timeout(Duration::from_secs(20), request.send())
        .await
        .context("request timed out")??;
    Ok((response.status().as_u16(), response.text().await?))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

/// The guest's request leaves with the bound token though the guest never
/// held it, streams its body both ways, and comes back with an audit id. The
/// workload declares no allowed hosts: the binding is the only grant.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_guest_sends_with_a_credential_it_never_holds() -> Result<()> {
    let running = start(Policy::new(true)).await?;
    assert_eq!(running.state, WorkloadState::Running, "{}", running.message);

    let large = "x".repeat(512 * 1024);
    let to = [("x-to", "https://api.example.com/v1/repos")];
    let (status, body) = call(running.addr, "/send", &to, large.clone()).await?;
    assert_eq!(status, 200, "{body}");
    let mut parts = body.splitn(3, ' ');
    assert_eq!(parts.next(), Some("201"));
    let audit = parts.next().unwrap();
    assert!(uuid::Uuid::parse_str(audit).is_ok(), "audit id {audit:?}");
    assert_eq!(parts.next().map(str::len), Some(large.len()));

    let sent = running.echo.sent.lock().unwrap().clone();
    assert_eq!(
        sent,
        [(
            "https://api.example.com/v1/repos".to_string(),
            Some("Bearer gh-token".to_string())
        )]
    );
    let callers = running.policy.callers.lock().unwrap().clone();
    assert!(
        callers
            .iter()
            .all(|c| c.component_name.as_deref() == Some("broker-client-p3")
                && &*c.workload_name == "broker-client"
                && &*c.namespace == "test")
    );
    assert_eq!(
        running.policy.uses.lock().unwrap().clone(),
        ["github completed"]
    );
    let brokered = running.brokered.lock().unwrap().clone();
    assert_eq!(brokered.len(), 1);
    assert_eq!(brokered[0].0, "https://api.example.com/v1/repos");
    assert!(
        brokered[0].1.is_some(),
        "the router sees the calling component"
    );
    Ok(())
}

/// The router has the last word on a brokered request, after the broker's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_router_can_refuse_a_brokered_request() -> Result<()> {
    let running = start_with(Policy::new(true), true, true).await?;
    assert_eq!(running.state, WorkloadState::Running, "{}", running.message);

    let to = [("x-to", "https://api.example.com/v1/repos")];
    let (status, body) = call(running.addr, "/send", &to, "hello").await?;
    assert_eq!(status, 502, "{body}");
    assert!(body.contains("NotEntitled"), "{body}");
    assert!(running.echo.sent.lock().unwrap().is_empty());
    assert_eq!(running.brokered.lock().unwrap().len(), 1);
    Ok(())
}

/// A granted hostname is still subject to the address gate once resolved:
/// `localhost` is bound, but loopback is not reachable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_granted_hostname_that_resolves_to_loopback_is_refused() -> Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&accepted);
    tokio::spawn(async move {
        while listener.accept().await.is_ok() {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });
    let running = start_with(Policy::new(true).to("localhost", port), false, false).await?;
    assert_eq!(running.state, WorkloadState::Running, "{}", running.message);

    let to = format!("https://localhost:{port}/v1/repos");
    let (status, body) = call(running.addr, "/send", &[("x-to", &to)], "hello").await?;
    assert_eq!(status, 502, "{body}");
    assert!(body.contains("ResolvedAddressNotAllowed"), "{body}");
    assert_eq!(accepted.load(Ordering::SeqCst), 0);
    assert_eq!(running.brokered.lock().unwrap().len(), 1);
    Ok(())
}

/// Each refusal names its reason and nothing reaches the network.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_outside_the_binding_are_refused_before_sending() -> Result<()> {
    let running = start(Policy::new(true)).await?;
    assert_eq!(running.state, WorkloadState::Running, "{}", running.message);

    for (headers, reason) in [
        (
            vec![("x-to", "https://evil.example.com/v1/repos")],
            "DestinationNotAllowed",
        ),
        (
            vec![("x-to", "http://api.example.com/v1/repos")],
            "DestinationNotAllowed",
        ),
        (
            vec![("x-to", "https://api.example.com/admin")],
            "OperationNotAllowed",
        ),
        (
            vec![
                ("x-to", "https://api.example.com/v1/repos"),
                ("x-authorization", "Bearer guessed"),
            ],
            "ReservedHeader",
        ),
    ] {
        let (status, body) = call(running.addr, "/send", &headers, "").await?;
        assert_eq!(status, 502, "{body}");
        assert!(body.contains(reason), "{reason}: {body}");
    }
    assert!(running.echo.sent.lock().unwrap().is_empty());
    let uses = running.policy.uses.lock().unwrap().clone();
    assert_eq!(
        uses,
        [
            "github denied destination-not-allowed",
            "github denied destination-not-allowed",
            "github denied operation-not-allowed",
            "github denied reserved-header",
        ]
    );
    Ok(())
}

/// Signing and verifying stream the message into keys the guest cannot read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_guest_signs_and_verifies_with_host_held_keys() -> Result<()> {
    use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};

    let running = start(Policy::new(true)).await?;
    assert_eq!(running.state, WorkloadState::Running, "{}", running.message);

    let manifest = "release manifest ".repeat(10_000);
    let (status, signature) = call(running.addr, "/sign", &[], manifest.clone()).await?;
    assert_eq!(status, 200, "{signature}");
    let (status, info) = call(running.addr, "/key-info", &[], "").await?;
    assert_eq!(status, 200, "{info}");
    let (algorithm, public) = info.split_once(' ').unwrap();
    assert!(algorithm.contains("EcdsaP256Sha256"), "{algorithm}");
    assert_eq!(unhex(public), running.policy.signer.public_key().unwrap());
    UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, unhex(public))
        .verify(manifest.as_bytes(), &unhex(&signature))
        .expect("the signature verifies against the published key");

    let payload = r#"{"event":"push"}"#;
    let key = aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA256, WEBHOOK_SECRET);
    let tag = hex(aws_lc_rs::hmac::sign(&key, payload.as_bytes()).as_ref());
    let (status, body) = call(running.addr, "/verify", &[("x-tag", &tag)], payload).await?;
    assert_eq!((status, body.as_str()), (200, "true"));
    let (status, body) = call(running.addr, "/verify", &[("x-tag", &tag)], "tampered").await?;
    assert_eq!((status, body.as_str()), (200, "false"));
    Ok(())
}

/// A label with no grant behind it fails the workload rather than a call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unbound_label_fails_the_workload() -> Result<()> {
    let running = start(Policy::new(false)).await?;
    assert_ne!(running.state, WorkloadState::Running);
    assert!(
        running.message.contains("github"),
        "the refusal names the label: {}",
        running.message
    );
    Ok(())
}
