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
        dpop::DpopKey,
        egress_credential::{EgressCaller, EgressCredentialSource, LeasedSource, SecretRoute},
        http::{
            DevRouter, HostHandler, Ingress, IngressRoute, OutgoingHandler, RequestIoFuture,
            RouteError, Router, SendFuture,
        },
        token_exchange::{Grant, TokenExchange, TokenRefresh, spawn_token_refresh},
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

/// Answers every request with its own body, recording what was sent and
/// echoing the credential back in response headers. Each pending challenge
/// answers one request with a DPoP nonce challenge instead.
#[derive(Clone, Default)]
struct Echo {
    sent: Arc<Mutex<Vec<Sent>>>,
    proofs: Arc<Mutex<Vec<Option<String>>>>,
    challenges: Arc<AtomicUsize>,
}

impl OutgoingHandler for Echo {
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
        let header = |name: &str| {
            request
                .headers()
                .get(name)
                .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
        };
        let authorization = header("authorization");
        self.proofs.lock().unwrap().push(header("dpop"));
        self.sent
            .lock()
            .unwrap()
            .push((request.uri().to_string(), authorization.clone()));
        let challenged = self
            .challenges
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
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
            let mut response = hyper::Response::builder().status(201);
            if challenged {
                response = response
                    .status(401)
                    .header("www-authenticate", r#"DPoP error="use_dpop_nonce""#)
                    .header("dpop-nonce", "n1");
            }
            if let Some(authorization) = &authorization {
                let token = authorization.split_once(' ').map_or("", |(_, t)| t);
                let escaped: String = token.bytes().map(|b| format!("%{b:02X}")).collect();
                response = response
                    .header("x-echo-authorization", authorization.as_str())
                    .header("x-echo-token", token)
                    .header("x-echo-escaped", escaped);
            }
            let response = response.body(body).expect("static response is well-formed");
            let io: RequestIoFuture = Box::new(async { Ok(()) });
            Ok((response, io))
        })
    }
}

/// An upstream that never said it handles secrets.
#[derive(Clone, Default)]
struct Naive(Echo);

impl OutgoingHandler for Naive {
    fn send_request(
        &self,
        workload_id: &str,
        request: hyper::Request<WasiBody>,
        options: Option<RequestOptions>,
        fut: RequestIoFuture,
    ) -> SendFuture {
        self.0.send_request(workload_id, request, options, fut)
    }
}

/// Records each brokered request it is asked about, refusing them on `veto`.
struct Gate {
    inner: DevRouter,
    veto: bool,
    brokered: Arc<Mutex<Vec<Sent>>>,
    saw_credential: Arc<AtomicUsize>,
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
        if request.headers().contains_key(hyper::header::AUTHORIZATION) {
            self.saw_credential.fetch_add(1, Ordering::SeqCst);
        }
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
    token: Option<Arc<TokenExchange>>,
    uses: Mutex<Vec<String>>,
    callers: Mutex<Vec<BrokerCaller>>,
}

impl Policy {
    fn new(bind_github: bool) -> Self {
        Self {
            bind_github,
            destination: ("api.example.com".to_string(), 443),
            signer: Arc::new(SignerKey::generate_ecdsa_p256().unwrap()),
            token: None,
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
        let presenter = match &self.token {
            Some(exchange) => Presenter::Token(Arc::clone(exchange)),
            None => Presenter::Bearer(token),
        };
        (self.bind_github && label == "github").then(|| {
            Arc::new(
                ClientBinding::new(presenter)
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
            BrokerOutcome::Failed(detail) => format!("failed {detail}"),
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
    saw_credential: Arc<AtomicUsize>,
    policy: Arc<Policy>,
    state: WorkloadState,
    message: String,
    _host: Arc<wash_runtime::host::Host>,
}

/// Where brokered requests go.
enum Upstream {
    Echo,
    /// An [`Echo`] that never said it handles secrets.
    Naive,
    /// The real network path, address gate included.
    Network,
}

async fn start(policy: Policy) -> Result<Running> {
    start_with(policy, false, Upstream::Echo).await
}

async fn start_with(policy: Policy, veto: bool, upstream: Upstream) -> Result<Running> {
    let echo = Echo::default();
    let brokered = Arc::default();
    let saw_credential = Arc::default();
    let gate = Gate {
        inner: DevRouter::default(),
        veto,
        brokered: Arc::clone(&brokered),
        saw_credential: Arc::clone(&saw_credential),
    };
    let builder = Ingress::builder(gate, "127.0.0.1:0".parse()?);
    let (addr, ingress): (_, Arc<dyn HostHandler>) = match upstream {
        Upstream::Echo => {
            let ingress = builder.outgoing_handler(echo.clone()).build().await?;
            (ingress.addr(), Arc::new(ingress))
        }
        Upstream::Naive => {
            let ingress = builder
                .outgoing_handler(Naive(echo.clone()))
                .build()
                .await?;
            (ingress.addr(), Arc::new(ingress))
        }
        Upstream::Network => {
            let ingress = builder.build().await?;
            (ingress.addr(), Arc::new(ingress))
        }
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
        saw_credential,
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
    let mut parts = body.splitn(4, ' ');
    assert_eq!(parts.next(), Some("201"));
    let audit = parts.next().unwrap();
    assert!(uuid::Uuid::parse_str(audit).is_ok(), "audit id {audit:?}");
    let echoed = parts.next().unwrap();
    assert!(!echoed.contains("gh-token"), "{echoed}");
    assert!(
        echoed.contains("x-echo-authorization=[redacted]")
            && echoed.contains("x-echo-token=[redacted]")
            && !echoed.contains("x-echo-escaped"),
        "the credential is scrubbed from the response in every spelling: {echoed}"
    );
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
    assert_eq!(
        running.saw_credential.load(Ordering::SeqCst),
        0,
        "the router is asked before the credential is written in"
    );
    Ok(())
}

/// An upstream that never said it handles secrets is never handed one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_handler_that_does_not_accept_secrets_is_not_supported() -> Result<()> {
    let running = start_with(Policy::new(true), false, Upstream::Naive).await?;
    assert_eq!(running.state, WorkloadState::Running, "{}", running.message);

    let to = [("x-to", "https://api.example.com/v1/repos")];
    let (status, body) = call(running.addr, "/send", &to, "hello").await?;
    assert_eq!(status, 502, "{body}");
    assert!(body.contains("NotSupported"), "{body}");
    assert!(running.echo.sent.lock().unwrap().is_empty());
    assert_eq!(
        running.policy.uses.lock().unwrap().clone(),
        ["github denied not-supported"]
    );
    Ok(())
}

/// A managed component's own `wasi:http` import links, and refuses every
/// request before it reaches the network.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_managed_component_cannot_send_without_the_broker() -> Result<()> {
    let running = start(Policy::new(true)).await?;
    assert_eq!(running.state, WorkloadState::Running, "{}", running.message);

    let to = [("x-to", "https://api.example.com/v1/repos")];
    let (status, body) = call(running.addr, "/direct", &to, "hello").await?;
    assert_eq!(status, 502, "{body}");
    assert!(body.contains("HttpRequestDenied"), "{body}");
    assert!(running.echo.sent.lock().unwrap().is_empty());
    assert!(running.policy.uses.lock().unwrap().is_empty());
    Ok(())
}

/// An HTTPS token endpoint on loopback that issues one DPoP-bound token.
async fn dpop_token_endpoint() -> Result<(String, Arc<rustls::ClientConfig>)> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])?;
    let cert = rustls::pki_types::CertificateDer::from(certified.cert.der().to_vec());
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
        certified.signing_key.serialize_der(),
    ));
    let server = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert.clone()], key)?;
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(stream) = acceptor.accept(stream).await else {
                    return;
                };
                let service = hyper::service::service_fn(|_request| async {
                    let body = serde_json::json!({
                        "access_token": "dpop-token",
                        "token_type": "DPoP",
                        "expires_in": 300,
                    });
                    hyper::Response::builder()
                        .header("content-type", "application/json")
                        .body(Full::new(Bytes::from(body.to_string())))
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert)?;
    let client = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok((format!("https://localhost:{port}/token"), Arc::new(client)))
}

fn proof_nonce(proof: &str) -> Option<String> {
    use base64::Engine as _;
    let payload = proof.split('.').nth(1)?;
    let claims: serde_json::Value = serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .ok()?,
    )
    .ok()?;
    claims.get("nonce")?.as_str().map(str::to_string)
}

async fn refreshed_dpop_exchange() -> Result<(Arc<TokenExchange>, TokenRefresh)> {
    let (endpoint, tls) = dpop_token_endpoint().await?;
    let svid = LeasedSource::default();
    svid.renew(
        Zeroizing::new(b"svid.jwt.sig".to_vec()),
        Duration::from_secs(300),
    );
    let exchange = Arc::new(
        TokenExchange::new(&endpoint, Grant::ClientAssertion, Arc::new(svid), tls)?
            .with_dpop(Arc::new(DpopKey::generate()?)),
    );
    let refresh = spawn_token_refresh(Arc::clone(&exchange))?;
    for _ in 0..250 {
        if exchange.current().is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    exchange.current().context("no DPoP token was issued")?;
    Ok((exchange, refresh))
}

/// A resource server's nonce challenge is answered once with a fresh proof
/// when the request can be sent again, and handed to the guest when it
/// cannot, with the nonce kept for the next request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resource_server_nonce_challenge_is_answered_once() -> Result<()> {
    let (exchange, _refresh) = refreshed_dpop_exchange().await?;
    let mut policy = Policy::new(true);
    policy.token = Some(exchange);
    let running = start(policy).await?;
    assert_eq!(running.state, WorkloadState::Running, "{}", running.message);
    let to = [("x-to", "https://api.example.com/v1/repos")];

    running.echo.challenges.store(1, Ordering::SeqCst);
    let (status, body) = call(running.addr, "/send", &to, "").await?;
    assert_eq!(status, 200, "{body}");
    assert!(body.starts_with("201 "), "the replay succeeds: {body}");
    assert!(!body.contains("dpop-token"), "{body}");
    let proofs = running.echo.proofs.lock().unwrap().clone();
    assert_eq!(proofs.len(), 2, "one challenge, one replay");
    assert_eq!(proof_nonce(proofs[0].as_deref().unwrap()), None);
    assert_eq!(
        proof_nonce(proofs[1].as_deref().unwrap()).as_deref(),
        Some("n1")
    );
    let sent = running.echo.sent.lock().unwrap().clone();
    assert!(
        sent.iter()
            .all(|(_, auth)| auth.as_deref() == Some("DPoP dpop-token")),
        "{sent:?}"
    );

    running.echo.challenges.store(1, Ordering::SeqCst);
    let (status, body) = call(running.addr, "/send", &to, "a body").await?;
    assert_eq!(status, 200, "{body}");
    assert!(
        body.starts_with("401 "),
        "a request with a body is not replayed: {body}"
    );
    let proofs = running.echo.proofs.lock().unwrap().clone();
    assert_eq!(proofs.len(), 3);
    assert_eq!(
        proof_nonce(proofs[2].as_deref().unwrap()).as_deref(),
        Some("n1"),
        "the cached nonce is used"
    );
    assert_eq!(
        running.policy.uses.lock().unwrap().clone(),
        ["github completed", "github completed"]
    );
    assert_eq!(running.brokered.lock().unwrap().len(), 3);
    Ok(())
}

/// The router has the last word on a brokered request, after the broker's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_router_can_refuse_a_brokered_request() -> Result<()> {
    let running = start_with(Policy::new(true), true, Upstream::Echo).await?;
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
    let running = start_with(
        Policy::new(true).to("localhost", port),
        false,
        Upstream::Network,
    )
    .await?;
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
