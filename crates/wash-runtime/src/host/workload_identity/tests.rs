use std::sync::atomic::{AtomicUsize, Ordering};

use base64::Engine as _;
use rcgen::{
    BasicConstraints, CertificateParams, CertificateSigningRequestParams, IsCa, Issuer, KeyPair,
    SanType,
};

use super::*;

const SPIFFE_ID: &str = "spiffe://acme.test/tenant/t1/identity/i1";

struct FakeIssuer {
    ca: Issuer<'static, KeyPair>,
    ca_der: CertificateDer<'static>,
    lifetime: Duration,
    x509_calls: AtomicUsize,
    jwt_calls: AtomicUsize,
    seen: Mutex<Vec<String>>,
    /// When set, each issuance waits for a permit.
    gate: Option<Arc<tokio::sync::Semaphore>>,
    /// The `aud` claim minted, when not the audience asked for.
    aud: Option<serde_json::Value>,
}

impl FakeIssuer {
    fn build(lifetime: Duration) -> Self {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_der = params.self_signed(&key).unwrap().der().clone();
        Self {
            ca: Issuer::new(params, key),
            ca_der,
            lifetime,
            x509_calls: AtomicUsize::new(0),
            jwt_calls: AtomicUsize::new(0),
            seen: Mutex::default(),
            gate: None,
            aud: None,
        }
    }

    fn new(lifetime: Duration) -> Arc<Self> {
        Arc::new(Self::build(lifetime))
    }

    fn gated(lifetime: Duration) -> (Arc<Self>, Arc<tokio::sync::Semaphore>) {
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let issuer = Self {
            gate: Some(Arc::clone(&gate)),
            ..Self::build(lifetime)
        };
        (Arc::new(issuer), gate)
    }

    async fn wait(&self) {
        if let Some(gate) = &self.gate {
            gate.acquire().await.unwrap().forget();
        }
    }

    fn host_tls(&self) -> Arc<rustls::ClientConfig> {
        crate::init_crypto();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(self.ca_der.clone()).unwrap();
        Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    }

    fn server_config(&self) -> rustls::ServerConfig {
        let key = KeyPair::generate().unwrap();
        let cert = CertificateParams::new(vec!["127.0.0.1".to_string()])
            .unwrap()
            .signed_by(&key, &self.ca)
            .unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(self.ca_der.clone()).unwrap();
        let verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(roots))
            .build()
            .unwrap();
        rustls::ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(
                vec![cert.der().clone()],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
            )
            .unwrap()
    }
}

#[async_trait]
impl IdentityIssuer for FakeIssuer {
    async fn issue_x509(
        &self,
        request: &IssueRequest<'_>,
        csr_der: &[u8],
    ) -> Result<X509Svid, IssuerError> {
        self.x509_calls.fetch_add(1, Ordering::SeqCst);
        let mut seen = request.loaded_digests.to_vec();
        seen.extend(request.namespace.map(str::to_string));
        *self.seen.lock().unwrap() = seen;
        self.wait().await;
        let mut csr = CertificateSigningRequestParams::from_der(&csr_der.to_vec().into()).unwrap();
        let now = time::OffsetDateTime::now_utc();
        csr.params.not_before = now - time::Duration::seconds(1);
        csr.params.not_after = now + time::Duration::milliseconds(self.lifetime.as_millis() as i64);
        csr.params.subject_alt_names = vec![SanType::URI(SPIFFE_ID.try_into().unwrap())];
        let cert = csr.signed_by(&self.ca).unwrap();
        Ok(X509Svid {
            chain: vec![cert.der().clone()],
        })
    }

    async fn issue_jwt(
        &self,
        _request: &IssueRequest<'_>,
        audience: &str,
    ) -> Result<JwtSvid, IssuerError> {
        self.jwt_calls.fetch_add(1, Ordering::SeqCst);
        self.wait().await;
        let exp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 300;
        let aud = self.aud.clone().unwrap_or_else(|| audience.into());
        Ok(JwtSvid {
            token: Zeroizing::new(jwt(&serde_json::json!({ "aud": aud, "exp": exp }))),
        })
    }
}

fn jwt(claims: &serde_json::Value) -> String {
    let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    format!(
        "{}.{}.sig",
        enc.encode(br#"{"alg":"ES256"}"#),
        enc.encode(claims.to_string())
    )
}

fn digest(byte: u8) -> String {
    sha256_digest(&[byte])
}

fn grant(destinations: Vec<MtlsDestination>, audiences: &[&str]) -> IdentityGrant {
    IdentityGrant {
        namespace: Some("tenant-a".into()),
        identity_ref: None,
        approved_digests: vec![digest(1)],
        scheduling_lease: SchedulingLease::new(
            b"lease".to_vec(),
            SystemTime::now() + Duration::from_secs(600),
        ),
        mtls_destinations: destinations,
        jwt_audiences: audiences.iter().map(|a| a.to_string()).collect(),
        ingress_hosts: Vec::new(),
    }
}

fn store(issuer: Arc<FakeIssuer>) -> WorkloadCredentials {
    let tls = issuer.host_tls();
    WorkloadCredentials::new(issuer, tls).with_retry(Duration::from_millis(20))
}

fn app() -> [(String, String); 1] {
    [("app".into(), digest(1))]
}

async fn eventually(mut check: impl FnMut() -> bool) {
    for _ in 0..300 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition never held");
}

#[tokio::test]
async fn a_component_outside_the_approved_digests_is_refused() {
    let credentials = store(FakeIssuer::new(Duration::from_secs(60)));
    assert!(
        !credentials.admit("w1", &[]).unwrap(),
        "no grant, no identity"
    );
    credentials.bind("w1", grant(vec![], &["sts"])).unwrap();
    let err = credentials
        .admit("w1", &[("app".into(), digest(2))])
        .unwrap_err();
    assert!(err.to_string().contains("not approved"), "{err}");
    assert!(credentials.admit("w1", &app()).unwrap());
    assert!(
        credentials.bind("w1", grant(vec![], &[])).is_err(),
        "a running workload's grant is not replaced"
    );
}

#[tokio::test]
async fn a_grant_needs_a_lease_and_well_formed_digests() {
    let credentials = store(FakeIssuer::new(Duration::from_secs(60)));
    let mut bad = grant(vec![], &[]);
    bad.approved_digests = vec!["latest".into()];
    assert!(credentials.bind("w1", bad).is_err());
    let mut upper = grant(vec![], &[]);
    upper.approved_digests = vec![digest(1).to_ascii_uppercase().replace("SHA256", "sha256")];
    assert!(credentials.bind("w1", upper).is_err());
    let mut none = grant(vec![], &[]);
    none.approved_digests.clear();
    assert!(credentials.bind("w1", none).is_err());
    let mut unleased = grant(vec![], &[]);
    unleased.scheduling_lease = SchedulingLease::new(Vec::new(), SystemTime::now());
    assert!(credentials.bind("w1", unleased).is_err());
    assert!(!credentials.is_bound("w1"));
    assert!(
        credentials
            .renew_lease("w1", SchedulingLease::new(b"x".to_vec(), SystemTime::now()))
            .is_err(),
        "not bound"
    );
}

#[tokio::test]
async fn the_jwt_svid_is_a_bearer_source_until_release() {
    let issuer = FakeIssuer::new(Duration::from_secs(60));
    let credentials = store(Arc::clone(&issuer));
    credentials
        .bind("w1", grant(vec![], &["sts.amazonaws.com"]))
        .unwrap();
    assert!(credentials.jwt_source("w1", "other").is_none());
    let source = credentials.jwt_source("w1", "sts.amazonaws.com").unwrap();
    assert_eq!(source.current(), Err(CredentialError::Unavailable));
    credentials.admit("w1", &app()).unwrap();
    eventually(|| source.current().is_ok()).await;
    let token = String::from_utf8(source.current().unwrap().to_vec()).unwrap();
    assert_eq!(jwt_claims(&token).unwrap().aud, ["sts.amazonaws.com"]);
    assert_eq!(
        issuer.x509_calls.load(Ordering::SeqCst),
        0,
        "no mTLS destinations"
    );

    credentials.release("w1");
    assert_eq!(source.current(), Err(CredentialError::Unavailable));
    assert!(!credentials.is_bound("w1"));
}

#[tokio::test]
async fn a_jwt_svid_for_more_than_its_audience_is_refused() {
    let issuer = Arc::new(FakeIssuer {
        aud: Some(serde_json::json!(["sts.amazonaws.com", "other"])),
        ..FakeIssuer::build(Duration::from_secs(60))
    });
    let credentials = store(Arc::clone(&issuer));
    credentials
        .bind("w1", grant(vec![], &["sts.amazonaws.com"]))
        .unwrap();
    let source = credentials.jwt_source("w1", "sts.amazonaws.com").unwrap();
    credentials.admit("w1", &app()).unwrap();
    eventually(|| issuer.jwt_calls.load(Ordering::SeqCst) >= 2).await;
    assert_eq!(source.current(), Err(CredentialError::Unavailable));
}

#[tokio::test]
async fn release_drops_an_issuance_still_in_flight() {
    let (issuer, gate) = FakeIssuer::gated(Duration::from_secs(60));
    let credentials = store(Arc::clone(&issuer));
    let destination = MtlsDestination::new("127.0.0.1", 8443).unwrap();
    credentials
        .bind("w1", grant(vec![destination], &["aud"]))
        .unwrap();
    let source = credentials.jwt_source("w1", "aud").unwrap();
    credentials.admit("w1", &app()).unwrap();
    eventually(|| {
        issuer.x509_calls.load(Ordering::SeqCst) == 1
            && issuer.jwt_calls.load(Ordering::SeqCst) == 1
    })
    .await;

    credentials.release("w1");
    gate.add_permits(10);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(source.current(), Err(CredentialError::Unavailable));
    assert_eq!(credentials.spiffe_id("w1"), None);
    assert_eq!(
        issuer.x509_calls.load(Ordering::SeqCst),
        1,
        "renewal stopped"
    );
    assert_eq!(
        issuer.jwt_calls.load(Ordering::SeqCst),
        1,
        "renewal stopped"
    );

    credentials
        .bind("w1", grant(vec![], &["aud"]))
        .expect("a released id can be bound again");
}

#[tokio::test]
async fn one_credential_waiting_on_the_issuer_does_not_hold_up_another() {
    struct SlowX509(Arc<FakeIssuer>);
    #[async_trait]
    impl IdentityIssuer for SlowX509 {
        async fn issue_x509(
            &self,
            _request: &IssueRequest<'_>,
            _csr_der: &[u8],
        ) -> Result<X509Svid, IssuerError> {
            std::future::pending().await
        }
        async fn issue_jwt(
            &self,
            request: &IssueRequest<'_>,
            audience: &str,
        ) -> Result<JwtSvid, IssuerError> {
            self.0.issue_jwt(request, audience).await
        }
    }
    let issuer = FakeIssuer::new(Duration::from_secs(60));
    let tls = issuer.host_tls();
    let credentials = WorkloadCredentials::new(Arc::new(SlowX509(issuer)), tls);
    let destination = MtlsDestination::new("127.0.0.1", 8443).unwrap();
    credentials
        .bind("w1", grant(vec![destination], &["aud"]))
        .unwrap();
    let source = credentials.jwt_source("w1", "aud").unwrap();
    credentials.admit("w1", &app()).unwrap();
    eventually(|| source.current().is_ok()).await;
}

#[tokio::test]
async fn an_expired_scheduling_lease_asks_the_issuer_nothing() {
    let issuer = FakeIssuer::new(Duration::from_secs(60));
    let credentials = store(Arc::clone(&issuer));
    let mut expiring = grant(vec![], &["aud"]);
    expiring.scheduling_lease = SchedulingLease::new(
        b"old".to_vec(),
        SystemTime::now() + Duration::from_millis(200),
    );
    credentials.bind("w1", expiring).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    credentials.admit("w1", &app()).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(issuer.jwt_calls.load(Ordering::SeqCst), 0);

    credentials
        .renew_lease(
            "w1",
            SchedulingLease::new(b"new".to_vec(), SystemTime::now() + Duration::from_secs(60)),
        )
        .unwrap();
    eventually(|| issuer.jwt_calls.load(Ordering::SeqCst) > 0).await;
}

#[tokio::test]
async fn the_x509_svid_is_renewed_half_way_through_its_life() {
    let issuer = FakeIssuer::new(Duration::from_millis(2500));
    let credentials = store(Arc::clone(&issuer));
    let destination = MtlsDestination::new("127.0.0.1", 8443).unwrap();
    credentials
        .bind("w1", grant(vec![destination], &[]))
        .unwrap();
    credentials.admit("w1", &app()).unwrap();
    eventually(|| credentials.spiffe_id("w1").is_some()).await;
    assert_eq!(credentials.spiffe_id("w1").as_deref(), Some(SPIFFE_ID));
    assert_eq!(
        *issuer.seen.lock().unwrap(),
        vec![digest(1), "tenant-a".to_string()]
    );
    tokio::time::sleep(Duration::from_millis(1800)).await;
    assert!(issuer.x509_calls.load(Ordering::SeqCst) >= 2);
}

#[test]
fn renewal_is_bounded() {
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    assert_eq!(
        renew_in(at, at + Duration::from_secs(600)),
        Duration::from_secs(300)
    );
    assert_eq!(renew_in(at, at), MIN_RENEWAL);
    assert_eq!(renew_in(at, at - Duration::from_secs(5)), MIN_RENEWAL);
    assert_eq!(
        renew_in(at, at + Duration::from_secs(365 * 24 * 3600)),
        MAX_LIFETIME / 2
    );
}

/// Serve TLS on `listener`, reporting each accepted client's SPIFFE ID.
fn serve(
    listener: tokio::net::TcpListener,
    config: rustls::ServerConfig,
) -> tokio::sync::mpsc::UnboundedReceiver<String> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let (seen, observed) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            let seen = seen.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let peer = tls.get_ref().1.peer_certificates().unwrap()[0].clone();
                let _ = seen.send(leaf_spiffe_id(&[peer]).unwrap());
                // Keep-alive, so a pooled connection is reused until it closes.
                loop {
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        let Ok(byte) = tls.read_u8().await else {
                            return;
                        };
                        request.push(byte);
                    }
                    if tls
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            });
        }
    });
    observed
}

#[tokio::test]
async fn the_x509_svid_is_presented_only_to_granted_destinations() {
    let issuer = FakeIssuer::new(Duration::from_secs(60));
    let credentials = store(Arc::clone(&issuer));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let destination = MtlsDestination::new("127.0.0.1", port).unwrap();
    credentials
        .bind("w1", grant(vec![destination], &[]))
        .unwrap();
    credentials.admit("w1", &app()).unwrap();
    eventually(|| credentials.spiffe_id("w1").is_some()).await;

    let inner: Arc<dyn ClientTlsConfigResolver> =
        Arc::new(crate::host::http_client::default_client_tls_config());
    let resolver = credentials.tls_resolver(inner);
    assert!(
        resolver
            .config_for_destination("w1", "127.0.0.1", port + 1)
            .is_none()
    );
    assert!(
        resolver
            .config_for_destination("w2", "127.0.0.1", port)
            .is_none()
    );
    let config = resolver
        .config_for_destination("w1", "127.0.0.1", port)
        .unwrap();

    let mut observed = serve(listener, issuer.server_config());
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let connector = tokio_rustls::TlsConnector::from(config);
    let _tls = connector
        .connect("127.0.0.1".try_into().unwrap(), stream)
        .await
        .unwrap();
    assert_eq!(observed.recv().await.unwrap(), SPIFFE_ID);

    credentials.revoke("w1");
    let refusing = resolver
        .config_for_destination("w1", "127.0.0.1", port)
        .expect("a revoked workload's destination still refuses");
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    assert!(
        tokio_rustls::TlsConnector::from(refusing)
            .connect("127.0.0.1".try_into().unwrap(), stream)
            .await
            .is_err()
    );

    credentials.release("w1");
    assert!(
        resolver
            .config_for_destination("w1", "127.0.0.1", port)
            .is_none()
    );
}

/// Once the SVID lapses with no renewal, the destination is refused again
/// and the pooled connection that presented it is closed.
#[tokio::test]
async fn an_expired_x509_svid_is_never_presented() {
    let (issuer, gate) = FakeIssuer::gated(Duration::from_millis(1500));
    let credentials = store(Arc::clone(&issuer));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut observed = serve(listener, issuer.server_config());
    credentials
        .bind(
            "w1",
            grant(vec![MtlsDestination::new("127.0.0.1", port).unwrap()], &[]),
        )
        .unwrap();
    gate.add_permits(1);
    credentials.admit("w1", &app()).unwrap();
    eventually(|| credentials.spiffe_id("w1").is_some()).await;

    let resolver = credentials.tls_resolver(Arc::new(issuer.host_tls()));
    let closes = |resolver: &Arc<dyn ClientTlsConfigResolver>| {
        resolver
            .destination_tls("w1", "127.0.0.1", port)
            .unwrap()
            .closes
            .is_some()
    };
    assert!(closes(&resolver));
    let clients = crate::host::http_client::WorkloadClients::with_tls_config_resolver(
        Arc::clone(&resolver),
        crate::host::quota::QuotaRegistry::new(Default::default(), None),
    );
    let send = || {
        let request = hyper::Request::builder()
            .uri(format!("https://127.0.0.1:{port}/"))
            .body(wasmtime_wasi_http::WasiBody::default())
            .unwrap();
        let client = clients.client("w1");
        async move {
            tokio::time::timeout(Duration::from_secs(5), client.send_request(request, None))
                .await
                .unwrap()
        }
    };
    let (response, _io) = send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(observed.recv().await.unwrap(), SPIFFE_ID);

    // The renewal waits on the gate, so the SVID lapses.
    tokio::time::sleep(Duration::from_millis(1700)).await;
    assert!(!closes(&resolver), "only the refusing config is left");
    assert!(send().await.is_err(), "neither reused nor reissued");
}

/// A connection's close is fixed when it is made, at its SVID's expiry,
/// however long the pool waits before first polling it.
#[tokio::test]
async fn a_connections_close_is_fixed_when_it_is_made() {
    let issuer = FakeIssuer::new(Duration::from_secs(3));
    let credentials = store(Arc::clone(&issuer));
    credentials
        .bind(
            "w1",
            grant(vec![MtlsDestination::new("127.0.0.1", 8443).unwrap()], &[]),
        )
        .unwrap();
    credentials.admit("w1", &app()).unwrap();
    eventually(|| credentials.spiffe_id("w1").is_some()).await;

    let resolver = credentials.tls_resolver(Arc::new(issuer.host_tls()));
    let closes = resolver
        .destination_tls("w1", "127.0.0.1", 8443)
        .unwrap()
        .closes
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    tokio::time::timeout(Duration::from_millis(1800), closes)
        .await
        .expect("closes at the SVID's expiry, not a lifetime after the first poll");
}

/// A revoke closes the connections that presented the SVID, idle or not.
#[tokio::test]
async fn revoking_an_identity_closes_its_pooled_connections() {
    let issuer = FakeIssuer::new(Duration::from_secs(60));
    let credentials = store(Arc::clone(&issuer));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut observed = serve(listener, issuer.server_config());
    credentials
        .bind(
            "w1",
            grant(vec![MtlsDestination::new("127.0.0.1", port).unwrap()], &[]),
        )
        .unwrap();
    credentials.admit("w1", &app()).unwrap();
    eventually(|| credentials.spiffe_id("w1").is_some()).await;
    let clients = crate::host::http_client::WorkloadClients::with_tls_config_resolver(
        credentials.tls_resolver(Arc::new(issuer.host_tls())),
        crate::host::quota::QuotaRegistry::new(Default::default(), None),
    );
    let send = || {
        let request = hyper::Request::builder()
            .uri(format!("https://127.0.0.1:{port}/"))
            .body(wasmtime_wasi_http::WasiBody::default())
            .unwrap();
        let client = clients.client("w1");
        async move {
            tokio::time::timeout(Duration::from_secs(5), client.send_request(request, None))
                .await
                .unwrap()
        }
    };
    let (response, _io) = send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(observed.recv().await.unwrap(), SPIFFE_ID);
    drop(response);
    let (reused, _io) = send().await.unwrap();
    assert_eq!(reused.status(), 200);
    assert!(observed.try_recv().is_err(), "the connection was reused");
    drop(reused);

    credentials.revoke("w1");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(send().await.is_err(), "the pooled connection was closed");
}

#[tokio::test]
async fn a_scheduling_lease_only_moves_forward() {
    let issuer = FakeIssuer::new(Duration::from_secs(60));
    let credentials = store(issuer);
    let now = SystemTime::now();
    let mut expired = grant(vec![], &["aud"]);
    expired.scheduling_lease = SchedulingLease::new(b"old".to_vec(), now);
    assert!(credentials.bind("w1", expired).is_err());

    credentials.bind("w1", grant(vec![], &["aud"])).unwrap();
    let lease = |token: &[u8], at: SystemTime| SchedulingLease::new(token.to_vec(), at);
    assert!(
        credentials
            .renew_lease("w1", lease(b"", now + Duration::from_secs(900)))
            .is_err()
    );
    assert!(credentials.renew_lease("w1", lease(b"past", now)).is_err());
    assert!(
        credentials
            .renew_lease("w1", lease(b"older", now + Duration::from_secs(60)))
            .is_err(),
        "expires before the lease it replaces"
    );
    credentials
        .renew_lease("w1", lease(b"newer", now + Duration::from_secs(900)))
        .unwrap();
    assert!(
        credentials
            .renew_lease("w2", lease(b"x", now + Duration::from_secs(900)))
            .is_err()
    );
}

#[tokio::test]
async fn a_revoked_identity_cannot_be_rebound_until_released() {
    let issuer = FakeIssuer::new(Duration::from_secs(60));
    let credentials = store(Arc::clone(&issuer));
    credentials.bind("w1", grant(vec![], &["aud"])).unwrap();
    let source = credentials.jwt_source("w1", "aud").unwrap();
    credentials.admit("w1", &app()).unwrap();
    eventually(|| source.current().is_ok()).await;

    assert!(!source.is_revoked());
    credentials.revoke("w1");
    assert_eq!(source.current(), Err(CredentialError::Unavailable));
    assert!(source.is_revoked(), "so an STS refresh built on it stops");
    assert!(credentials.jwt_source("w1", "aud").is_none());
    assert!(credentials.bind("w1", grant(vec![], &["aud"])).is_err());
    assert!(credentials.admit("w1", &app()).is_err());
    let lease = SchedulingLease::new(b"x".to_vec(), SystemTime::now() + Duration::from_secs(900));
    assert!(credentials.renew_lease("w1", lease).is_err());

    credentials.release("w1");
    credentials.bind("w1", grant(vec![], &["aud"])).unwrap();
}

#[tokio::test]
async fn release_all_drops_every_identity_and_frees_the_issuer() {
    let issuer = FakeIssuer::new(Duration::from_secs(60));
    let credentials = store(Arc::clone(&issuer));
    let issued = |id: &str| {
        credentials
            .jwt_source(id, "aud")
            .is_some_and(|source| source.current().is_ok())
    };
    for id in ["w1", "w2"] {
        credentials.bind(id, grant(vec![], &["aud"])).unwrap();
        credentials.admit(id, &app()).unwrap();
    }
    eventually(|| issued("w1") && issued("w2")).await;
    credentials.release_all();
    assert!(!credentials.is_bound("w1") && !credentials.is_bound("w2"));

    // Renewal tasks hold the store weakly, so dropping it frees the issuer.
    credentials.bind("w3", grant(vec![], &["aud"])).unwrap();
    credentials.admit("w3", &app()).unwrap();
    eventually(|| issued("w3")).await;
    drop(credentials);
    eventually(|| Arc::strong_count(&issuer) == 1).await;
}

/// A pooled connection made before the SVID arrived would keep presenting
/// nothing after it did, so none is made.
#[tokio::test]
async fn the_pool_never_keeps_a_connection_made_before_the_svid() {
    let (issuer, gate) = FakeIssuer::gated(Duration::from_secs(60));
    let credentials = store(Arc::clone(&issuer));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut observed = serve(listener, issuer.server_config());
    credentials
        .bind(
            "w1",
            grant(vec![MtlsDestination::new("127.0.0.1", port).unwrap()], &[]),
        )
        .unwrap();
    credentials.admit("w1", &app()).unwrap();

    let clients = crate::host::http_client::WorkloadClients::with_tls_config_resolver(
        credentials.tls_resolver(Arc::new(issuer.host_tls())),
        crate::host::quota::QuotaRegistry::new(Default::default(), None),
    );
    let send = || {
        let request = hyper::Request::builder()
            .uri(format!("https://127.0.0.1:{port}/"))
            .body(wasmtime_wasi_http::WasiBody::default())
            .unwrap();
        let client = clients.client("w1");
        async move {
            tokio::time::timeout(Duration::from_secs(5), client.send_request(request, None))
                .await
                .unwrap()
        }
    };
    assert!(send().await.is_err(), "refused until the SVID is issued");

    gate.add_permits(1);
    eventually(|| credentials.spiffe_id("w1").is_some()).await;
    let (response, _io) = send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(observed.recv().await.unwrap(), SPIFFE_ID);
}

#[test]
fn an_svid_names_exactly_one_spiffe_id() {
    let key = KeyPair::generate().unwrap();
    let plain = CertificateParams::new(vec!["example.com".to_string()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    assert!(leaf_spiffe_id(&[plain.der().clone()]).is_err());
    assert!(leaf_spiffe_id(&[]).is_err());
}

#[test]
fn a_jwt_without_exp_is_refused() {
    assert!(jwt_claims(&jwt(&serde_json::json!({ "aud": "a" }))).is_err());
    assert!(jwt_claims("not.a.jwt.at.all").is_err());
    let claims = jwt_claims(&jwt(&serde_json::json!({ "aud": ["a", "b"], "exp": 5 }))).unwrap();
    assert!(claims.aud == ["a", "b"] && claims.exp == 5);
    assert!(jwt_claims(&jwt(&serde_json::json!({ "aud": ["a", 7], "exp": 5 }))).is_err());
}

#[test]
fn an_mtls_destination_is_exact() {
    assert!(MtlsDestination::new("*.example.com", 443).is_err());
    assert!(MtlsDestination::new("example.com:443", 443).is_err());
    assert!(MtlsDestination::new("example.com", 0).is_err());
    let d = MtlsDestination::new("Example.com", 443).unwrap();
    assert!(d.matches("example.COM.", 443) && !d.matches("example.com", 8443));
    for v6 in ["::1", "[::1]", "0:0:0:0:0:0:0:1"] {
        let d = MtlsDestination::new(v6, 443).unwrap();
        assert!(d.matches("[::1]", 443), "{v6}");
        assert!(!d.matches("[::2]", 443) && !d.matches("::1", 8443), "{v6}");
    }
    assert!(MtlsDestination::new("[example.com]", 443).is_err());
}

#[test]
fn an_issue_request_never_prints_its_lease() {
    let digests = vec![digest(1)];
    let request = IssueRequest {
        workload_id: "w1",
        namespace: None,
        identity_ref: None,
        loaded_digests: &digests,
        scheduling_lease: b"secret-lease",
    };
    let printed = format!("{request:?}");
    assert!(
        printed.contains("w1") && !printed.contains("115"),
        "{printed}"
    );
}

#[tokio::test]
async fn a_granted_mtls_destination_is_network_only() {
    use crate::host::http::OutgoingHandler as _;
    let credentials = store(FakeIssuer::new(Duration::from_secs(60)));
    credentials
        .bind(
            "w1",
            grant(
                vec![MtlsDestination::new("db.internal", 8443).unwrap()],
                &[],
            ),
        )
        .unwrap();
    let handler = crate::host::http::DefaultOutgoingHandler::default();
    let granted: hyper::Uri = "https://db.internal:8443/".parse().unwrap();
    assert!(
        !handler.network_only("w1", &granted),
        "nothing installed yet"
    );

    handler.install_workload_credentials(credentials);
    assert!(handler.network_only("w1", &granted));
    assert!(!handler.network_only("w2", &granted));
    for other in ["http://db.internal:8443/", "https://db.internal/"] {
        assert!(
            !handler.network_only("w1", &other.parse().unwrap()),
            "{other}"
        );
    }
}

mod federation {
    use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _};
    use rcgen::ExtendedKeyUsagePurpose;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;
    use crate::host::trust_bundle::{SpiffePeer, TrustBundle, TrustDomain};

    const PEER_SERVER: &str = "spiffe://peer.test/wi/server";
    const CALLER: &str = "spiffe://acme.test/wi/caller";

    fn ca() -> (Issuer<'static, KeyPair>, CertificateDer<'static>) {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let der = params.self_signed(&key).unwrap().der().clone();
        (Issuer::new(params, key), der)
    }

    fn svid(
        ca: &Issuer<'static, KeyPair>,
        id: &str,
        usage: ExtendedKeyUsagePurpose,
    ) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.subject_alt_names = vec![SanType::URI(id.try_into().unwrap())];
        params.extended_key_usages = vec![usage];
        let cert = params.signed_by(&key, ca).unwrap();
        (
            vec![cert.der().clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        )
    }

    fn bundle(domain: &str, ca: &CertificateDer<'_>, jwt: Option<&EcdsaKeyPair>) -> TrustBundle {
        let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let mut keys = vec![serde_json::json!({
            "kty": "EC", "use": "x509-svid",
            "x5c": [base64::engine::general_purpose::STANDARD.encode(ca)],
        })];
        if let Some(key) = jwt {
            let point = key.public_key().as_ref();
            keys.push(serde_json::json!({
                "kty": "EC", "use": "jwt-svid", "kid": "k1", "crv": "P-256",
                "x": enc.encode(&point[1..33]), "y": enc.encode(&point[33..]),
            }));
        }
        let json = serde_json::json!({ "keys": keys }).to_string();
        TrustBundle::from_spiffe_json(TrustDomain::new(domain).unwrap(), json.as_bytes()).unwrap()
    }

    fn later() -> SystemTime {
        SystemTime::now() + Duration::from_secs(60)
    }

    /// A peer-domain TLS server that requires a client certificate from the
    /// local domain and answers each request with the client's SPIFFE ID.
    async fn peer_server(
        local_ca: &CertificateDer<'static>,
        peer_ca: &Issuer<'static, KeyPair>,
        id: &str,
    ) -> u16 {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(local_ca.clone()).unwrap();
        let verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(roots))
            .build()
            .unwrap();
        let (chain, key) = svid(peer_ca, id, ExtendedKeyUsagePurpose::ServerAuth);
        let config = rustls::ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(chain, key)
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (tcp, _) = listener.accept().await.unwrap();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(tcp).await else {
                        return;
                    };
                    let mut byte = [0u8; 1];
                    let _ = tls.read(&mut byte).await;
                    let _ = tls.write_all(b"ok").await;
                });
            }
        });
        port
    }

    async fn connect(config: Arc<rustls::ClientConfig>, port: u16) -> bool {
        let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let Ok(mut tls) = tokio_rustls::TlsConnector::from(config)
            .connect("127.0.0.1".try_into().unwrap(), stream)
            .await
        else {
            return false;
        };
        tls.write_all(b"x").await.unwrap();
        let mut reply = [0u8; 2];
        tls.read_exact(&mut reply).await.is_ok()
    }

    #[tokio::test]
    async fn a_spiffe_server_is_verified_against_its_federated_bundle() {
        let issuer = FakeIssuer::new(Duration::from_secs(3600));
        let credentials = store(Arc::clone(&issuer));
        let (peer_ca, peer_ca_der) = ca();
        let port = peer_server(&issuer.ca_der, &peer_ca, PEER_SERVER).await;
        let to = |peer: &str| {
            MtlsDestination::new("127.0.0.1", port)
                .unwrap()
                .with_server(SpiffePeer::parse(peer).unwrap())
        };
        credentials
            .bind("domain", grant(vec![to("spiffe://peer.test")], &[]))
            .unwrap();
        credentials
            .bind("exact", grant(vec![to("spiffe://peer.test/wi/other")], &[]))
            .unwrap();
        for workload in ["domain", "exact"] {
            credentials.admit(workload, &app()).unwrap();
        }
        eventually(|| {
            credentials.spiffe_id("domain").is_some() && credentials.spiffe_id("exact").is_some()
        })
        .await;
        let resolver = credentials.tls_resolver(Arc::new(issuer.host_tls()));
        let config = |workload: &str| {
            resolver
                .config_for_destination(workload, "127.0.0.1", port)
                .unwrap()
        };

        assert!(
            !connect(config("domain"), port).await,
            "no peer bundle, no connection"
        );
        credentials
            .trust_bundles()
            .replace([bundle("peer.test", &peer_ca_der, None)], later())
            .unwrap();
        assert!(connect(config("domain"), port).await);
        assert!(
            !connect(config("exact"), port).await,
            "the server is not the ID the destination names"
        );

        let (other_ca, other_der) = ca();
        drop(other_ca);
        credentials
            .trust_bundles()
            .replace([bundle("peer.test", &other_der, None)], later())
            .unwrap();
        assert!(
            !connect(config("domain"), port).await,
            "verified against the peer's bundle, not the host's roots"
        );
    }

    #[tokio::test]
    async fn removing_a_federation_closes_its_connections() {
        let issuer = FakeIssuer::new(Duration::from_secs(3600));
        let credentials = store(Arc::clone(&issuer));
        let (_, peer_ca_der) = ca();
        credentials
            .trust_bundles()
            .replace([bundle("peer.test", &peer_ca_der, None)], later())
            .unwrap();
        let destination = MtlsDestination::new("peer.example", 8443)
            .unwrap()
            .with_server(SpiffePeer::parse("spiffe://peer.test").unwrap());
        credentials
            .bind("w1", grant(vec![destination], &[]))
            .unwrap();
        credentials.admit("w1", &app()).unwrap();
        eventually(|| credentials.spiffe_id("w1").is_some()).await;
        let resolver = credentials.tls_resolver(Arc::new(issuer.host_tls()));
        let closes = || {
            resolver
                .destination_tls("w1", "peer.example", 8443)
                .unwrap()
                .closes
                .unwrap()
        };

        let first = closes();
        credentials
            .trust_bundles()
            .replace([bundle("peer.test", &peer_ca_der, None)], later())
            .unwrap();
        let mut first = std::pin::pin!(first);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            futures::poll!(first.as_mut()).is_pending(),
            "a refresh keeps the connection"
        );
        credentials.trust_bundles().replace([], later()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), first)
            .await
            .expect("removing the federation closes the connection");

        credentials
            .trust_bundles()
            .replace([bundle("peer.test", &peer_ca_der, None)], later())
            .unwrap();
        let capped = closes();
        tokio::time::pause();
        let mut capped = std::pin::pin!(capped);
        tokio::time::advance(SPIFFE_CONNECTION_REUSE - Duration::from_secs(1)).await;
        assert!(futures::poll!(capped.as_mut()).is_pending());
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(
            futures::poll!(capped.as_mut()).is_ready(),
            "reused for at most five minutes"
        );
    }

    #[tokio::test]
    async fn an_ingress_host_presents_one_workloads_svid() {
        let issuer = FakeIssuer::new(Duration::from_secs(3600));
        let credentials = store(Arc::clone(&issuer));
        let serving = |hosts: &[&str]| IdentityGrant {
            ingress_hosts: hosts.iter().map(|h| h.to_string()).collect(),
            ..grant(vec![], &[])
        };
        assert!(credentials.bind("w0", serving(&["[::1]"])).is_err());
        credentials.bind("w1", serving(&["Svc.Test."])).unwrap();
        let err = credentials.bind("w2", serving(&["svc.test"])).unwrap_err();
        assert!(err.to_string().contains("already presents"), "{err}");
        assert!(
            credentials.ingress_svid("svc.test").is_none(),
            "nothing to present before issuance"
        );
        credentials.admit("w1", &app()).unwrap();
        eventually(|| credentials.ingress_svid("SVC.test").is_some()).await;
        let w1 = credentials.spiffe_id("w1").unwrap();
        assert_eq!(credentials.ingress_svid("svc.test").unwrap().0, w1);
        assert!(credentials.ingress_svid("other.test").is_none());
        credentials.revoke("w1");
        assert!(credentials.ingress_svid("svc.test").is_none());
        // The revoked entry stays until released, and never shadows the next.
        credentials.bind("w2", serving(&["svc.test"])).unwrap();
        credentials.admit("w2", &app()).unwrap();
        eventually(|| credentials.ingress_svid("svc.test").is_some()).await;
        for _ in 0..16 {
            assert!(credentials.ingress_svid("svc.test").is_some());
        }

        let replica = |id: &str| IdentityGrant {
            identity_ref: Some(id.into()),
            ..serving(&["api.test"])
        };
        credentials.bind("r1", replica("api")).unwrap();
        credentials.bind("r2", replica("api")).unwrap();
        let err = credentials.bind("r3", replica("other")).unwrap_err();
        assert!(err.to_string().contains("already presents"), "{err}");
    }

    /// The ingress side of a federated call: the workload's SVID is served by
    /// SNI, a client SVID is verified, and a request routed elsewhere is
    /// refused.
    #[tokio::test]
    async fn ingress_serves_the_workloads_svid_and_verifies_the_caller() {
        use crate::host::ingress_identity::{PEER_ID_HEADER, PEER_SVID_HEADER, accept, admit};
        let issuer = FakeIssuer::new(Duration::from_secs(3600));
        let credentials = store(Arc::clone(&issuer));
        credentials
            .bind(
                "w1",
                IdentityGrant {
                    ingress_hosts: vec!["svc.test".into()],
                    ..grant(vec![], &[])
                },
            )
            .unwrap();
        credentials.admit("w1", &app()).unwrap();
        eventually(|| credentials.ingress_svid("svc.test").is_some()).await;
        let jwt_key = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING).unwrap();
        credentials
            .trust_bundles()
            .replace(
                [bundle("acme.test", &issuer.ca_der, Some(&jwt_key))],
                later(),
            )
            .unwrap();

        let default = {
            let key = KeyPair::generate().unwrap();
            let cert = CertificateParams::new(vec!["default.test".into()])
                .unwrap()
                .self_signed(&key)
                .unwrap();
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(
                    vec![cert.der().clone()],
                    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
                )
                .unwrap()
        };
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(default));
        let slot: crate::host::ingress_identity::IdentitySlot = Arc::default();
        let _ = slot.set(credentials.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (accepted, mut connections) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let (tcp, _) = listener.accept().await.unwrap();
                let (acceptor, slot, accepted) = (acceptor.clone(), slot.clone(), accepted.clone());
                tokio::spawn(async move {
                    let outcome = accept(&acceptor, &slot, tcp).await;
                    let ok = outcome.is_ok();
                    let (tls, connection) = match outcome {
                        Ok((tls, connection)) => (Some(tls), connection),
                        Err(_) => (None, None),
                    };
                    let _ = accepted.send((ok, connection));
                    if let Some(mut tls) = tls {
                        let mut byte = [0u8; 1];
                        let _ = tls.read(&mut byte).await;
                    }
                });
            }
        });

        let client = |cert: Option<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)>| {
            let builder = rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(
                    credentials
                        .trust_bundles()
                        .server_verifier(SpiffePeer::parse(SPIFFE_ID).unwrap()),
                );
            Arc::new(match cert {
                Some((chain, key)) => builder.with_client_auth_cert(chain, key).unwrap(),
                None => builder.with_no_client_auth(),
            })
        };
        let dial = |config: Arc<rustls::ClientConfig>, name: &'static str| async move {
            let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            let mut tls = tokio_rustls::TlsConnector::from(config)
                .connect(name.try_into().unwrap(), stream)
                .await
                .ok()?;
            tls.write_all(b"x").await.ok()?;
            Some(tls)
        };

        let caller = svid(&issuer.ca, CALLER, ExtendedKeyUsagePurpose::ClientAuth);
        let _held = dial(client(Some(caller)), "svc.test")
            .await
            .expect("the client verified the workload's SVID");
        let (ok, connection) = connections.recv().await.unwrap();
        let x509 = connection.filter(|_| ok).unwrap();
        let mut headers = hyper::HeaderMap::new();
        headers.insert(PEER_ID_HEADER, "spiffe://forged.test/a".parse().unwrap());
        crate::host::ingress_identity::strip(&mut headers);
        assert_eq!(admit(&x509, "w2", &mut headers), Err(421));
        admit(&x509, "w1", &mut headers).unwrap();
        assert_eq!(headers[PEER_ID_HEADER], CALLER);
        assert_eq!(headers[PEER_SVID_HEADER], "x509");

        let (stranger_ca, _) = ca();
        let stranger = svid(&stranger_ca, CALLER, ExtendedKeyUsagePurpose::ClientAuth);
        assert!(
            dial(client(Some(stranger)), "svc.test").await.is_none() || {
                let (ok, _) = connections.recv().await.unwrap();
                !ok
            }
        );

        let _anonymous = dial(client(None), "svc.test")
            .await
            .expect("a client may present no certificate");
        let (_, connection) = loop {
            let next = connections.recv().await.unwrap();
            if next.0 {
                break next;
            }
        };
        let connection = connection.unwrap();
        let token = {
            let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
            let exp = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + 60;
            let signing = format!(
                "{}.{}",
                enc.encode(r#"{"alg":"ES256","kid":"k1"}"#),
                enc.encode(
                    serde_json::json!({ "sub": CALLER, "aud": SPIFFE_ID, "exp": exp }).to_string()
                )
            );
            let signature = jwt_key
                .sign(&aws_lc_rs::rand::SystemRandom::new(), signing.as_bytes())
                .unwrap();
            format!("{signing}.{}", enc.encode(signature.as_ref()))
        };
        let mut headers = hyper::HeaderMap::new();
        admit(&connection, "w1", &mut headers).unwrap();
        assert!(!headers.contains_key(PEER_ID_HEADER), "no proof, no peer");
        headers.insert(
            hyper::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        admit(&connection, "w1", &mut headers).unwrap();
        assert_eq!(headers[PEER_ID_HEADER], CALLER);
        assert_eq!(headers[PEER_SVID_HEADER], "jwt");

        credentials.trust_bundles().clear();
        let mut headers = hyper::HeaderMap::new();
        headers.insert(
            hyper::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        admit(&connection, "w1", &mut headers).unwrap();
        assert!(
            !headers.contains_key(PEER_ID_HEADER),
            "a dropped bundle verifies nothing"
        );
        let mut headers = hyper::HeaderMap::new();
        admit(&x509, "w1", &mut headers).unwrap();
        assert!(
            !headers.contains_key(PEER_ID_HEADER),
            "a connection verified before the bundle was dropped carries no peer after"
        );
    }
}
