use std::path::{Path, PathBuf};
use std::sync::Arc;

use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject as _};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use super::*;

/// A private CA, a server certificate it issued for `cluster.internal`, and a
/// client certificate it issued, all written as PEM under one directory.
pub(crate) struct Pki {
    pub(crate) dir: tempfile::TempDir,
    pub(crate) server_chain: Vec<CertificateDer<'static>>,
    pub(crate) server_key: PrivateKeyDer<'static>,
    pub(crate) ca_der: CertificateDer<'static>,
}

impl Pki {
    pub(crate) fn new() -> Self {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();

        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_key = KeyPair::generate().unwrap();
        let ca_cert = ca_params.self_signed(&ca_key).unwrap();
        let issuer = Issuer::from_params(&ca_params, &ca_key);

        let server_key = KeyPair::generate().unwrap();
        let server_cert = CertificateParams::new(vec!["cluster.internal".to_string()])
            .unwrap()
            .signed_by(&server_key, &issuer)
            .unwrap();
        let client_key = KeyPair::generate().unwrap();
        let client_cert = CertificateParams::new(vec!["plugin".to_string()])
            .unwrap()
            .signed_by(&client_key, &issuer)
            .unwrap();

        std::fs::write(dir.path().join("ca.crt"), ca_cert.pem()).unwrap();
        std::fs::write(dir.path().join("client.crt"), client_cert.pem()).unwrap();
        std::fs::write(dir.path().join("client.key"), client_key.serialize_pem()).unwrap();
        std::fs::write(dir.path().join("empty.crt"), "").unwrap();

        Self {
            server_chain: vec![server_cert.der().clone()],
            server_key: PrivateKeyDer::from_pem_slice(server_key.serialize_pem().as_bytes())
                .unwrap(),
            ca_der: ca_cert.der().clone(),
            dir,
        }
    }

    pub(crate) fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// A server for `cluster.internal` that requires a client certificate
    /// from this CA when `mtls` is set.
    pub(crate) fn server_config(&self, mtls: bool) -> Arc<rustls::ServerConfig> {
        let builder = rustls::ServerConfig::builder();
        let builder = if mtls {
            let mut roots = rustls::RootCertStore::empty();
            roots.add(self.ca_der.clone()).unwrap();
            builder.with_client_cert_verifier(
                rustls::server::WebPkiClientVerifier::builder(Arc::new(roots))
                    .build()
                    .unwrap(),
            )
        } else {
            builder.with_no_client_auth()
        };
        Arc::new(
            builder
                .with_single_cert(self.server_chain.clone(), self.server_key.clone_key())
                .unwrap(),
        )
    }
}

fn grant(ca: Option<&Path>, roots: Option<TlsRoots>) -> TlsGrant {
    TlsGrant {
        ca: ca.map(Path::to_path_buf),
        roots,
        ..Default::default()
    }
}

fn host(s: &str) -> AllowedHost {
    s.parse().unwrap()
}

/// Run one handshake through `client` against `server`, over an in-memory
/// pipe, and echo a line to prove the session carries data.
async fn handshake(
    client: Arc<rustls::ClientConfig>,
    server: Arc<rustls::ServerConfig>,
) -> std::io::Result<()> {
    let (client_io, server_io) = tokio::io::duplex(16 * 1024);
    let server_task = tokio::spawn(async move {
        let mut tls = tokio_rustls::TlsAcceptor::from(server)
            .accept(server_io)
            .await?;
        let mut buf = [0u8; 5];
        tls.read_exact(&mut buf).await?;
        tls.write_all(&buf).await?;
        tls.shutdown().await?;
        std::io::Result::Ok(())
    });
    let name = rustls::pki_types::ServerName::try_from("cluster.internal").unwrap();
    let mut tls = tokio_rustls::TlsConnector::from(client)
        .connect(name, client_io)
        .await?;
    tls.write_all(b"PING\n").await?;
    let mut buf = [0u8; 5];
    tls.read_exact(&mut buf).await?;
    assert_eq!(&buf, b"PING\n");
    server_task.await.unwrap()
}

#[test]
fn a_string_entry_parses_as_a_bare_grant() {
    let entry: PluginAllowedHost = serde_json::from_str(r#""cluster.internal:8093""#).unwrap();
    assert_eq!(entry.host, host("cluster.internal:8093"));
    assert!(entry.tls.is_none());
    assert_eq!(
        serde_json::to_string(&entry).unwrap(),
        r#""cluster.internal:8093""#
    );
}

#[test]
fn a_record_entry_carries_its_tls_block() {
    let entry: PluginAllowedHost = serde_json::from_str(
        r#"{"host": "cluster.internal:11207",
            "tls": {"ca": "tls/ca.crt", "roots": "replace",
                    "clientCert": "c.crt", "clientKey": "c.key"}}"#,
    )
    .unwrap();
    let tls = entry.tls.as_ref().unwrap();
    assert_eq!(tls.roots, Some(TlsRoots::Replace));
    assert_eq!(tls.client_key.as_deref(), Some(Path::new("c.key")));
    let round_trip: PluginAllowedHost =
        serde_json::from_str(&serde_json::to_string(&entry).unwrap()).unwrap();
    assert_eq!(round_trip, entry);
}

#[test]
fn a_misspelled_tls_field_is_refused() {
    let err = serde_json::from_str::<PluginAllowedHost>(
        r#"{"host": "cluster.internal", "tls": {"caFile": "x"}}"#,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("caFile"), "got: {err}");
    assert!(
        serde_json::from_str::<PluginAllowedHost>(r#"{"host": "a", "tsl": {}}"#).is_err(),
        "a misspelled `tls` key must not be dropped"
    );
}

#[test]
fn relative_paths_resolve_against_the_base() {
    let mut grant = TlsGrant {
        ca: Some("tls/ca.crt".into()),
        client_cert: Some("/abs/c.crt".into()),
        ..Default::default()
    };
    grant.resolve_relative_to(Path::new("/project"));
    assert_eq!(grant.ca.as_deref(), Some(Path::new("/project/tls/ca.crt")));
    assert_eq!(grant.client_cert.as_deref(), Some(Path::new("/abs/c.crt")));
}

#[test]
fn a_client_cert_without_its_key_is_refused() {
    let pki = Pki::new();
    let err = TlsGrant {
        client_cert: Some(pki.path("client.crt")),
        ..Default::default()
    }
    .load(&host("cluster.internal"))
    .unwrap_err()
    .to_string();
    assert!(err.contains("clientKey"), "got: {err}");
}

#[test]
fn replace_without_a_ca_is_refused() {
    let err = grant(None, Some(TlsRoots::Replace))
        .load(&host("cluster.internal"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("tls.ca"), "got: {err}");
}

#[test]
fn a_missing_or_empty_ca_fails_at_load() {
    let pki = Pki::new();
    for name in ["missing.crt", "empty.crt"] {
        assert!(
            grant(Some(&pki.path(name)), None)
                .load(&host("cluster.internal"))
                .is_err(),
            "{name} must not load"
        );
    }
}

#[test]
fn tls_on_a_plaintext_scheme_is_refused() {
    let pki = Pki::new();
    for entry in ["http://cluster.internal", "ws://*.internal"] {
        let err = grant(Some(&pki.path("ca.crt")), None)
            .load(&host(entry))
            .unwrap_err()
            .to_string();
        assert!(err.contains("never"), "{entry}: {err}");
    }
    grant(Some(&pki.path("ca.crt")), None)
        .load(&host("https://cluster.internal"))
        .unwrap();
}

#[test]
fn duplicate_hosts_still_validate_each_scheme() {
    crate::init_crypto();
    for (a, b) in [
        ("https://cluster.internal", "http://cluster.internal"),
        ("wss://*.internal", "ws://*.internal"),
    ] {
        for entries in [[a, b], [b, a]] {
            let err = policy(&entries.map(|host| (host, Some(TlsGrant::default()))))
                .unwrap_err()
                .to_string();
            assert!(err.contains("never"), "{entries:?}: {err}");
        }
    }
}

#[test]
fn wildcard_tls_names_ignore_the_trailing_dot() {
    crate::init_crypto();
    let policy = policy(&[("*.internal.", Some(TlsGrant::default()))])
        .unwrap()
        .unwrap();
    for name in ["cluster.internal", "CLUSTER.internal."] {
        assert!(policy.for_server_name(name).is_some(), "{name}");
    }
    assert!(policy.for_server_name("internal").is_none());
    assert!(policy.for_server_name("notinternal").is_none());
}

#[tokio::test]
async fn add_trusts_the_private_ca_beside_public_roots() {
    let pki = Pki::new();
    let trust = grant(Some(&pki.path("ca.crt")), Some(TlsRoots::Add))
        .load(&host("cluster.internal"))
        .unwrap();
    handshake(trust.client_config(), pki.server_config(false))
        .await
        .unwrap();
}

#[tokio::test]
async fn replace_trusts_only_the_private_ca() {
    let pki = Pki::new();
    let trust = grant(Some(&pki.path("ca.crt")), Some(TlsRoots::Replace))
        .load(&host("cluster.internal"))
        .unwrap();
    handshake(trust.client_config(), pki.server_config(false))
        .await
        .unwrap();
}

#[tokio::test]
async fn public_roots_alone_refuse_a_private_ca() {
    let pki = Pki::new();
    let trust = TlsGrant::default().load(&host("cluster.internal")).unwrap();
    let err = handshake(trust.client_config(), pki.server_config(false))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("UnknownIssuer"), "got: {err}");
}

#[tokio::test]
async fn a_declared_client_certificate_satisfies_mtls() {
    let pki = Pki::new();
    let with_identity = TlsGrant {
        ca: Some(pki.path("ca.crt")),
        client_cert: Some(pki.path("client.crt")),
        client_key: Some(pki.path("client.key")),
        ..Default::default()
    }
    .load(&host("cluster.internal"))
    .unwrap();
    handshake(with_identity.client_config(), pki.server_config(true))
        .await
        .unwrap();

    let without = grant(Some(&pki.path("ca.crt")), None)
        .load(&host("cluster.internal"))
        .unwrap();
    assert!(
        handshake(without.client_config(), pki.server_config(true))
            .await
            .is_err()
    );
}

fn policy(entries: &[(&str, Option<TlsGrant>)]) -> anyhow::Result<Option<PluginTlsPolicy>> {
    let grants: Vec<PluginAllowedHost> = entries
        .iter()
        .map(|(h, tls)| PluginAllowedHost {
            host: host(h),
            tls: tls.clone(),
        })
        .collect();
    PluginTlsPolicy::from_grants(&grants)
}

#[test]
fn no_tls_block_declares_no_policy() {
    assert!(
        policy(&[("*", None), ("a.example", None)])
            .unwrap()
            .is_none()
    );
}

#[test]
fn trust_covers_every_port_of_the_host_it_names() {
    let pki = Pki::new();
    let private = grant(Some(&pki.path("ca.crt")), None);
    let policy = policy(&[
        ("cluster.internal:11207", Some(private.clone())),
        ("cluster.internal:8093", None),
    ])
    .unwrap()
    .unwrap();
    let by_name = policy.for_server_name("CLUSTER.internal.").unwrap();
    assert_eq!(by_name.grant(), &private);
    let by_url = policy
        .for_url("couchbases://cluster.internal:18091")
        .unwrap();
    assert_eq!(by_url.unwrap().grant(), &private);
    assert!(policy.for_server_name("other.internal").is_none());
}

#[test]
fn the_most_specific_tls_entry_wins() {
    let pki = Pki::new();
    let public = TlsGrant::default();
    let private = grant(Some(&pki.path("ca.crt")), None);
    let replace = grant(Some(&pki.path("ca.crt")), Some(TlsRoots::Replace));
    let policy = policy(&[
        ("*", Some(public.clone())),
        ("*.internal", Some(replace.clone())),
        ("cluster.internal", Some(private.clone())),
    ])
    .unwrap()
    .unwrap();
    assert_eq!(
        policy.for_server_name("cluster.internal").unwrap().grant(),
        &private
    );
    assert_eq!(
        policy.for_server_name("other.internal").unwrap().grant(),
        &replace
    );
    assert_eq!(
        policy.for_server_name("api.example.com").unwrap().grant(),
        &public
    );
}

#[test]
fn one_host_with_different_tls_is_refused_whatever_the_ports() {
    let pki = Pki::new();
    for (a, b) in [
        ("cluster.internal", "cluster.internal"),
        ("cluster.internal:11207", "cluster.internal:18091"),
        ("tls://cluster.internal:4222", "cluster.internal"),
        ("10.0.0.5:4222", "[::ffff:10.0.0.5]:4223"),
        ("*.internal:1", "*.internal:2"),
        ("*.internal.", "*.internal"),
    ] {
        let err = policy(&[
            (a, Some(TlsGrant::default())),
            (b, Some(grant(Some(&pki.path("ca.crt")), None))),
        ])
        .unwrap_err()
        .to_string();
        assert!(err.contains("same host"), "{a} / {b}: {err}");
    }
    policy(&[
        ("cluster.internal:1", Some(TlsGrant::default())),
        ("cluster.internal:2", Some(TlsGrant::default())),
    ])
    .unwrap()
    .unwrap();
}

#[test]
fn an_address_is_matched_as_an_address() {
    let policy = policy(&[("10.0.0.5:4222", Some(TlsGrant::default()))])
        .unwrap()
        .unwrap();
    assert!(
        policy
            .for_url("nats://[::ffff:10.0.0.5]:4222")
            .unwrap()
            .is_some()
    );
    assert!(policy.for_server_name("::ffff:10.0.0.5").is_some());
    assert!(policy.for_server_name("10.0.0.6").is_none());
}
