use super::*;

/// Clients reused across pulls, keyed by what they are built from and by the
/// credentials they carry: `oci-client` keeps the first credentials it sees
/// for a registry, so a client shared across credentials would pull with
/// whichever came first.
///
/// Environment roots are snapshotted for each pull. The snapshot identifies
/// both the client and the completed-result cache, so rotation or installation
/// of extra roots cannot reuse a result from a different trust configuration.
pub(super) async fn pull_client(
    insecure: bool,
    auth: &RegistryAuth,
    trust: &TrustSnapshot,
) -> Result<Client> {
    static CLIENTS: OnceLock<moka::sync::Cache<(bool, AuthId, TrustId), Client>> = OnceLock::new();
    let clients = CLIENTS.get_or_init(|| {
        moka::sync::Cache::builder()
            .max_capacity(256)
            .time_to_live(CLIENT_REUSE)
            .build()
    });
    let key = (insecure, AuthId::of(auth), trust.id);
    if let Some(client) = clients.get(&key) {
        return Ok(client);
    }
    let roots = trust.roots.clone();
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
            })
            .map_err(|err| anyhow::Error::new(crate::engine::SharedError(err)))
    })
    .await
}

const CLIENT_REUSE: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct TrustId([u8; 32]);

pub(super) struct TrustSnapshot {
    pub(super) id: TrustId,
    roots: Vec<Certificate>,
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
