use super::*;

/// Clients reused across pulls, keyed by what they are built from and by the
/// credentials they carry: `oci-client` keeps the first credentials it sees
/// for a registry, so a client shared across credentials would pull with
/// whichever came first.
///
/// Each client serves one registry/repository. The dependency's token and
/// authentication maps have no eviction, so sharing a client across arbitrary
/// repositories would bypass the bound on retained clients, even on failures.
///
/// Environment roots are snapshotted for each pull. The snapshot identifies
/// both the client and the completed-result cache, so rotation or installation
/// of extra roots cannot reuse a result from a different trust configuration.
pub(super) async fn pull_client(
    reference: &Reference,
    insecure: bool,
    auth: &RegistryAuth,
    trust: &TrustSnapshot,
) -> Result<Arc<Client>> {
    static CLIENTS: OnceLock<ClientCache> = OnceLock::new();
    let clients = CLIENTS.get_or_init(|| client_cache(256));
    pull_client_from_cache(clients, reference, insecure, auth, trust).await
}

type ClientCache = moka::sync::Cache<ClientKey, Arc<Client>>;

#[derive(Clone, PartialEq, Eq, Hash)]
struct ClientKey {
    registry: String,
    repository: String,
    insecure: bool,
    auth: AuthId,
    trust: TrustId,
}

fn client_cache(capacity: u64) -> ClientCache {
    moka::sync::Cache::builder()
        .max_capacity(capacity)
        .time_to_live(CLIENT_REUSE)
        .build()
}

async fn pull_client_from_cache(
    clients: &ClientCache,
    reference: &Reference,
    insecure: bool,
    auth: &RegistryAuth,
    trust: &TrustSnapshot,
) -> Result<Arc<Client>> {
    let key = ClientKey {
        registry: reference.resolve_registry().to_owned(),
        repository: reference.repository().to_owned(),
        insecure,
        auth: AuthId::of(auth),
        trust: trust.id,
    };
    if let Some(client) = clients.get(&key) {
        return Ok(client);
    }
    let roots = trust.roots.clone();
    let clients = clients.clone();
    blocking_io(move || {
        clients
            .try_get_with(key, || {
                oci_client(ClientConfig {
                    protocol: if insecure {
                        ClientProtocol::Http
                    } else {
                        ClientProtocol::Https
                    },
                    extra_root_certificates: roots,
                    ..Default::default()
                })
                .map(Arc::new)
            })
            .map_err(|err| anyhow::Error::new(crate::engine::SharedError(err)))
    })
    .await
}

const CLIENT_REUSE: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct TrustId(pub(super) [u8; 32]);

pub(super) struct TrustSnapshot {
    pub(super) id: TrustId,
    pub(super) roots: Vec<Certificate>,
}

impl TrustSnapshot {
    pub(super) async fn load() -> Result<Self> {
        let load = || {
            let roots = extra_ca_certificates();
            let mut hash = Sha256::new();
            // Include paths as well as contents: removing an environment
            // override also changes what the platform verifier loads.
            for name in ["SSL_CERT_FILE", "SSL_CERT_DIR"] {
                let value = std::env::var_os(name);
                hash.update([u8::from(value.is_some())]);
                let bytes = value
                    .as_deref()
                    .map_or(&[][..], std::ffi::OsStr::as_encoded_bytes);
                hash.update((bytes.len() as u64).to_le_bytes());
                hash.update(bytes);
            }
            hash.update([u8::from(EXTRA_CA_CERTIFICATES.get().is_some())]);
            for root in &roots {
                hash.update([match root.encoding {
                    CertificateEncoding::Der => 0,
                    CertificateEncoding::Pem => 1,
                }]);
                hash.update((root.data.len() as u64).to_le_bytes());
                hash.update(&root.data);
            }
            Ok(Self {
                id: TrustId(hash.finalize().into()),
                roots,
            })
        };
        if std::env::var_os("SSL_CERT_FILE").is_some() || std::env::var_os("SSL_CERT_DIR").is_some()
        {
            blocking_io(load).await
        } else {
            load()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn client_reuse_is_confined_to_a_repository_and_security_scope() {
        crate::init_crypto();
        let clients = client_cache(16);
        let trust = TrustSnapshot::load().await.unwrap();
        let auth = RegistryAuth::Basic("user".into(), "secret".into());
        let reference = Reference::try_from("registry.invalid/ns/image:v1").unwrap();
        let first = pull_client_from_cache(&clients, &reference, false, &auth, &trust)
            .await
            .unwrap();
        let another_tag = Reference::try_from("registry.invalid/ns/image:v2").unwrap();
        let reused = pull_client_from_cache(&clients, &another_tag, false, &auth, &trust)
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&first, &reused), "tags share one token scope");
        for different in ["other.invalid/ns/image:v1", "registry.invalid/ns/other:v1"] {
            let different = Reference::try_from(different).unwrap();
            let client = pull_client_from_cache(&clients, &different, false, &auth, &trust)
                .await
                .unwrap();
            assert!(!Arc::ptr_eq(&first, &client));
        }
        let other_auth = RegistryAuth::Basic("user".into(), "other-secret".into());
        let other_trust = TrustSnapshot {
            id: TrustId([0; 32]),
            roots: vec![],
        };
        for (insecure, auth, trust) in [
            (true, &auth, &trust),
            (false, &other_auth, &trust),
            (false, &auth, &other_trust),
        ] {
            let client = pull_client_from_cache(&clients, &reference, insecure, auth, trust)
                .await
                .unwrap();
            assert!(!Arc::ptr_eq(&first, &client));
        }
    }

    #[tokio::test]
    async fn failed_repositories_cannot_accumulate_in_one_retained_client() {
        use bytes::Bytes;
        use http_body_util::Full;
        use hyper::{Request, Response, StatusCode, body::Incoming, service::service_fn};
        use hyper_util::rt::TokioIo;
        use std::convert::Infallible;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::net::TcpListener;

        crate::init_crypto();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let authenticated = Arc::new(AtomicUsize::new(0));
        let requests = authenticated.clone();
        let server = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let requests = requests.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<Incoming>| {
                        let mut response = Response::builder().header("connection", "close");
                        if req.uri().path() == "/v2/" {
                            response = response
                                .status(StatusCode::UNAUTHORIZED)
                                .header("www-authenticate", "Basic realm=\"test\"");
                        } else {
                            if req
                                .headers()
                                .get("authorization")
                                .and_then(|v| v.to_str().ok())
                                == Some("Basic dXNlcjpzZWNyZXQ=")
                            {
                                requests.fetch_add(1, Ordering::SeqCst);
                            }
                            response = response.status(StatusCode::NOT_FOUND).header(
                                "docker-content-digest",
                                format!("sha256:{}", "ab".repeat(32)),
                            );
                        }
                        std::future::ready(Ok::<_, Infallible>(
                            response.body(Full::new(Bytes::new())).unwrap(),
                        ))
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });

        let clients = client_cache(4);
        let trust = TrustSnapshot::load().await.unwrap();
        let auth = RegistryAuth::Basic("user".into(), "secret".into());
        let mut previous = Vec::new();
        for index in 0..32 {
            let reference =
                Reference::try_from(format!("{addr}/missing/image-{index}:v1")).unwrap();
            let client = pull_client_from_cache(&clients, &reference, true, &auth, &trust)
                .await
                .unwrap();
            let weak = Arc::downgrade(&client);
            assert!(
                previous
                    .iter()
                    .all(|old: &std::sync::Weak<Client>| !old.ptr_eq(&weak)),
                "a new repository must not grow another repository's token map"
            );
            assert!(
                tokio::time::timeout(
                    Duration::from_secs(5),
                    client.fetch_manifest_digest(&reference, &auth)
                )
                .await
                .unwrap()
                .is_err()
            );
            previous.push(weak);
        }
        server.abort();
        assert_eq!(
            authenticated.load(Ordering::SeqCst),
            32,
            "every failure happened after Basic auth populated its token map"
        );
        clients.run_pending_tasks();
        assert!(clients.entry_count() <= 4);
        assert!(
            previous
                .iter()
                .filter(|client| client.upgrade().is_some())
                .count()
                <= 4,
            "eviction must release the clients and their internal token maps"
        );
    }
}
