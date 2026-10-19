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
    let config = trust.client_config(insecure)?;
    let clients = clients.clone();
    blocking_io(move || {
        clients
            .try_get_with(key, || oci_client(config).map(Arc::new))
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
    /// On Unix, environment PEM paths replace the native roots. Freeze that
    /// complete set so the platform verifier cannot read a different bundle
    /// while this snapshot waits for client construction.
    freeze_native_roots: bool,
}

impl TrustSnapshot {
    pub(super) fn client_config(&self, insecure: bool) -> Result<ClientConfig> {
        let mut config = ClientConfig {
            protocol: if insecure {
                ClientProtocol::Http
            } else {
                ClientProtocol::Https
            },
            ..Default::default()
        };
        if self.freeze_native_roots {
            // oci-client treats an empty tls_certs_only vector as unset and
            // would fall back to rereading the native/environment store.
            if self.roots.is_empty() {
                bail!("no usable OCI trust roots in the environment snapshot");
            }
            config.tls_certs_only = self.roots.clone();
        } else {
            config.extra_root_certificates = self.roots.clone();
        }
        Ok(config)
    }

    pub(super) async fn load() -> Result<Self> {
        let load = || {
            let roots = extra_ca_certificates();
            let mut hash = Sha256::new();
            let mut environment_roots = false;
            // Include paths as well as contents: removing an environment
            // override also changes what the platform verifier loads.
            for name in ["SSL_CERT_FILE", "SSL_CERT_DIR"] {
                let value = std::env::var_os(name);
                environment_roots |= value.is_some();
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
            let freeze_native_roots = environment_roots
                && cfg!(all(
                    unix,
                    not(target_vendor = "apple"),
                    not(target_os = "android"),
                    not(target_arch = "wasm32")
                ));
            if freeze_native_roots {
                // Do not reuse disk entries from clients which could merge
                // a second, differently timed read of the environment roots.
                hash.update(b"frozen-environment-roots-v1");
            }
            Ok(Self {
                id: TrustId(hash.finalize().into()),
                roots,
                freeze_native_roots,
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

    #[test]
    fn environment_roots_are_frozen_before_client_construction() {
        const CHILD: &str = "WASH_TEST_OCI_SNAPSHOT_BUNDLE";
        if let Some(bundle) = std::env::var_os(CHILD) {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(check_root_snapshot(Path::new(&bundle)));
            return;
        }

        // Each child starts with its environment in place, so this does not
        // mutate process-global environment while other tests are running.
        for variable in ["SSL_CERT_FILE", "SSL_CERT_DIR"] {
            let dir = tempfile::tempdir().unwrap();
            let bundle = dir.path().join("roots.pem");
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "oci::client::tests::environment_roots_are_frozen_before_client_construction",
                    "--nocapture",
                ])
                .env(CHILD, &bundle)
                .env_remove("SSL_CERT_FILE")
                .env_remove("SSL_CERT_DIR");
            child.env(
                variable,
                if variable == "SSL_CERT_FILE" {
                    bundle.as_path()
                } else {
                    dir.path()
                },
            );
            let output = child.output().unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                output.status.success() && stdout.contains("1 passed"),
                "{variable} snapshot regression failed:\n{stdout}\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    async fn check_root_snapshot(bundle: &Path) {
        use super::super::{test_ca::Ca, test_registry::Registry};
        use std::sync::atomic::Ordering;

        crate::init_crypto();
        let ca_a = Ca::new("snapshot-a");
        let ca_b = Ca::new("snapshot-b");
        let registry_a = Registry::start_with_tls(Some(ca_a.server_config())).await;
        let registry_b = Registry::start_with_tls(Some(ca_b.server_config())).await;
        let auth = RegistryAuth::Basic("user".into(), "secret".into());
        let reference_a = Reference::try_from(registry_a.reference()).unwrap();
        let reference_b = Reference::try_from(registry_b.reference()).unwrap();

        std::fs::write(bundle, ca_a.pem()).unwrap();
        let snapshot_a = TrustSnapshot::load().await.unwrap();
        // This is the construction barrier: the pull captured A, then waited
        // for a client slot while a projected CA bundle was replaced with B.
        std::fs::write(bundle, ca_b.pem()).unwrap();
        let client_a = pull_client(&reference_a, false, &auth, &snapshot_a)
            .await
            .unwrap();
        let client_b = pull_client(&reference_b, false, &auth, &snapshot_a)
            .await
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(5),
            client_a.fetch_manifest_digest(&reference_a, &auth),
        )
        .await
        .unwrap()
        .unwrap();

        // Prove B is reachable and trusted under its own current snapshot.
        let config =
            OciConfig::new_with_credentials("user", "secret").with_timeout(Duration::from_secs(5));
        pull_component(
            &registry_b.reference(),
            config.clone(),
            OciPullPolicy::Always,
        )
        .await
        .unwrap();

        std::fs::write(bundle, ca_a.pem()).unwrap();
        let restored = TrustSnapshot::load().await.unwrap();
        assert!(restored.id == snapshot_a.id);
        let reused = pull_client(&reference_b, false, &auth, &restored)
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&client_b, &reused));
        registry_b.requests.store(0, Ordering::SeqCst);
        assert!(
            pull_component(&registry_b.reference(), config, OciPullPolicy::Always)
                .await
                .is_err(),
            "a client stored under A must not retain B from construction time"
        );
        assert_eq!(registry_b.requests.load(Ordering::SeqCst), 0);

        if snapshot_a.freeze_native_roots {
            std::fs::write(bundle, "").unwrap();
            let empty = TrustSnapshot::load().await.unwrap();
            std::fs::write(bundle, ca_b.pem()).unwrap();
            assert!(
                empty.client_config(false).is_err(),
                "an empty snapshot must not fall back to newly installed roots"
            );
        }
    }

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
            freeze_native_roots: false,
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
        let _ = server.await;
        assert_eq!(
            authenticated.load(Ordering::SeqCst),
            32,
            "every failure happened after Basic auth populated its token map"
        );
        clients.run_pending_tasks();
        assert!(clients.entry_count() <= 4);
        // Moka removes entries before epoch reclamation drops their values.
        // Keep driving maintenance until those retired clients are reclaimed.
        let live_clients = || {
            previous
                .iter()
                .filter(|client| client.upgrade().is_some())
                .count()
        };
        let reclaimed = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                clients.run_pending_tasks();
                if live_clients() <= 4 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await;
        assert!(
            reclaimed.is_ok(),
            "eviction must release the clients and their internal token maps: {} clients remain",
            live_clients()
        );
    }
}
