use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

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

impl Pki {
    /// Trust bundles `add` and `replace` over this CA, an `empty` one, and the
    /// identity `client`.
    fn catalog(&self) -> Arc<TlsCatalog> {
        TlsCatalog::load(&self.bundles(), &self.identities()).unwrap()
    }

    fn bundles(&self) -> BTreeMap<String, TrustBundle> {
        [("add", TlsRoots::Add), ("replace", TlsRoots::Replace)]
            .into_iter()
            .map(|(name, roots)| {
                (
                    name.to_string(),
                    TrustBundle {
                        ca: self.path("ca.crt"),
                        roots: Some(roots),
                    },
                )
            })
            .collect()
    }

    fn identities(&self) -> BTreeMap<String, IdentitySource> {
        BTreeMap::from([(
            "client".to_string(),
            IdentitySource {
                cert: self.path("client.crt"),
                key: self.path("client.key"),
                refresh: None,
            },
        )])
    }
}

fn select(trust: Option<&str>, identity: Option<&str>) -> TlsGrant {
    TlsGrant {
        trust: trust.map(str::to_string),
        identity: identity.map(str::to_string),
    }
}

fn host(s: &str) -> AllowedHost {
    s.parse().unwrap()
}

fn policy(
    catalog: &Arc<TlsCatalog>,
    entries: &[(&str, Option<TlsGrant>)],
) -> anyhow::Result<Option<PluginTlsPolicy>> {
    let grants: Vec<PluginAllowedHost> = entries
        .iter()
        .map(|(h, tls)| PluginAllowedHost {
            host: host(h),
            tls: tls.clone(),
        })
        .collect();
    PluginTlsPolicy::from_grants(&grants, catalog)
}

fn config_for(catalog: &Arc<TlsCatalog>, grant: TlsGrant) -> Arc<rustls::ClientConfig> {
    policy(catalog, &[("cluster.internal", Some(grant))])
        .unwrap()
        .unwrap()
        .for_server_name("cluster.internal")
        .unwrap()
        .client_config()
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
fn a_record_entry_selects_by_name() {
    let entry: PluginAllowedHost = serde_json::from_str(
        r#"{"host": "cluster.internal:11207", "tls": {"trust": "corp", "identity": "db"}}"#,
    )
    .unwrap();
    assert_eq!(entry.tls, Some(select(Some("corp"), Some("db"))));
    let round_trip: PluginAllowedHost =
        serde_json::from_str(&serde_json::to_string(&entry).unwrap()).unwrap();
    assert_eq!(round_trip, entry);
}

#[test]
fn key_material_on_a_grant_is_refused() {
    // Material lives in the catalogs; a grant only names it.
    for tls in [
        r#"{"ca": "x"}"#,
        r#"{"clientCert": "x"}"#,
        r#"{"trsut": "x"}"#,
    ] {
        let json = format!(r#"{{"host": "cluster.internal", "tls": {tls}}}"#);
        assert!(
            serde_json::from_str::<PluginAllowedHost>(&json).is_err(),
            "{tls}"
        );
    }
    assert!(
        serde_json::from_str::<PluginAllowedHost>(r#"{"host": "a", "tsl": {}}"#).is_err(),
        "a misspelled `tls` key must not be dropped"
    );
}

#[test]
fn catalog_entries_parse_with_a_refresh_interval() {
    let identity: IdentitySource =
        serde_json::from_str(r#"{"cert": "c.crt", "key": "c.key", "refresh": "30s"}"#).unwrap();
    assert_eq!(identity.refresh, Some(Duration::from_secs(30)));
    let bundle: TrustBundle =
        serde_json::from_str(r#"{"ca": "ca.crt", "roots": "replace"}"#).unwrap();
    assert_eq!(bundle.roots, Some(TlsRoots::Replace));
    assert!(serde_json::from_str::<TrustBundle>(r#"{"roots": "add"}"#).is_err());
    assert!(serde_json::from_str::<IdentitySource>(r#"{"cert": "c.crt"}"#).is_err());
}

#[test]
fn relative_paths_resolve_against_the_base() {
    let mut identity = IdentitySource {
        cert: "tls/client.crt".into(),
        key: "/abs/client.key".into(),
        refresh: None,
    };
    identity.resolve_relative_to(Path::new("/project"));
    assert_eq!(identity.cert, Path::new("/project/tls/client.crt"));
    assert_eq!(identity.key, Path::new("/abs/client.key"));
}

#[test]
fn a_missing_or_empty_ca_fails_at_load() {
    let pki = Pki::new();
    std::fs::write(pki.path("empty.crt"), "").unwrap();
    for name in ["missing.crt", "empty.crt"] {
        let bundles = BTreeMap::from([(
            "bad".to_string(),
            TrustBundle {
                ca: pki.path(name),
                roots: None,
            },
        )]);
        let err = TlsCatalog::load(&bundles, &BTreeMap::new())
            .unwrap_err()
            .to_string();
        assert!(err.contains("'bad'"), "{name}: {err}");
    }
}

#[test]
fn an_identity_whose_key_does_not_match_fails_at_load() {
    let pki = Pki::new();
    let other = KeyPair::generate().unwrap();
    std::fs::write(pki.path("other.key"), other.serialize_pem()).unwrap();
    let identities = BTreeMap::from([(
        "crossed".to_string(),
        IdentitySource {
            cert: pki.path("client.crt"),
            key: pki.path("other.key"),
            refresh: None,
        },
    )]);
    assert!(TlsCatalog::load(&BTreeMap::new(), &identities).is_err());
}

#[test]
fn refresh_needs_an_async_runtime() {
    let pki = Pki::new();
    let mut identities = pki.identities();
    identities.get_mut("client").unwrap().refresh = Some(Duration::from_secs(30));
    let err = TlsCatalog::load(&BTreeMap::new(), &identities)
        .unwrap_err()
        .to_string();
    assert!(err.contains("runtime"), "{err}");
}

#[tokio::test]
async fn a_refreshing_identity_loads_inside_a_runtime() {
    let pki = Pki::new();
    let mut identities = pki.identities();
    identities.get_mut("client").unwrap().refresh = Some(Duration::from_secs(30));
    TlsCatalog::load(&BTreeMap::new(), &identities).unwrap();
}

#[test]
fn a_selection_naming_nothing_declared_is_refused() {
    let pki = Pki::new();
    let catalog = pki.catalog();
    for grant in [select(Some("nope"), None), select(None, Some("nope"))] {
        let err = policy(&catalog, &[("cluster.internal", Some(grant))])
            .unwrap_err()
            .to_string();
        assert!(err.contains("'nope'"), "{err}");
    }
}

#[test]
fn tls_on_a_plaintext_scheme_is_refused() {
    let pki = Pki::new();
    let catalog = pki.catalog();
    for entry in ["http://cluster.internal", "ws://*.internal"] {
        let err = policy(&catalog, &[(entry, Some(TlsGrant::default()))])
            .unwrap_err()
            .to_string();
        assert!(err.contains("never"), "{entry}: {err}");
    }
    policy(
        &catalog,
        &[("https://cluster.internal", Some(TlsGrant::default()))],
    )
    .unwrap();
}

/// A private CA on `*` could vouch for any hostname, and an identity there
/// would be presented to every server that asks for one.
#[test]
fn a_trust_bundle_or_identity_on_star_is_refused() {
    let pki = Pki::new();
    let catalog = pki.catalog();
    for grant in [select(Some("add"), None), select(None, Some("client"))] {
        let err = policy(&catalog, &[("*", Some(grant))])
            .unwrap_err()
            .to_string();
        assert!(err.contains("every destination"), "{err}");
    }
    // The platform's own roots, and no identity, are what `*` would get anyway.
    policy(&catalog, &[("*", Some(TlsGrant::default()))]).unwrap();
    // A suffix names what the material is for, unless it is a whole
    // top-level domain.
    policy(
        &catalog,
        &[("*.corp.internal", Some(select(Some("add"), Some("client"))))],
    )
    .unwrap();
    for broad in ["*.com", "https://*.internal"] {
        let err = policy(&catalog, &[(broad, Some(select(None, Some("client"))))])
            .unwrap_err()
            .to_string();
        assert!(err.contains("top-level domain"), "{broad}: {err}");
    }
    policy(&catalog, &[("*.com", Some(TlsGrant::default()))]).unwrap();
}

#[test]
fn duplicate_hosts_still_validate_each_scheme() {
    let pki = Pki::new();
    let catalog = pki.catalog();
    for (a, b) in [
        ("https://cluster.internal", "http://cluster.internal"),
        ("wss://*.internal", "ws://*.internal"),
    ] {
        for entries in [[a, b], [b, a]] {
            let err = policy(&catalog, &entries.map(|h| (h, Some(TlsGrant::default()))))
                .unwrap_err()
                .to_string();
            assert!(err.contains("never"), "{entries:?}: {err}");
        }
    }
}

/// Two plugins selecting one pair share its material, so a rotation reaches
/// both, but never its session store.
#[test]
fn plugins_selecting_the_same_pair_share_material_not_sessions() {
    let pki = Pki::new();
    let catalog = pki.catalog();
    let a = config_for(&catalog, select(Some("add"), Some("client")));
    let b = config_for(&catalog, select(Some("add"), Some("client")));
    assert!(!Arc::ptr_eq(&a, &b));
    assert!(Arc::ptr_eq(
        &a.client_auth_cert_resolver,
        &b.client_auth_cert_resolver
    ));
}

/// A resumed session would skip the rotating resolver and keep whatever it
/// authenticated as, so an identity-bearing configuration does not resume,
/// while one without an identity still does.
#[tokio::test]
async fn an_identity_bearing_configuration_does_not_resume() {
    use rustls::HandshakeKind;
    let pki = Pki::new();
    let catalog = pki.catalog();
    let second_handshake = |config: Arc<rustls::ClientConfig>, mtls: bool| {
        let server = pki.server_config(mtls);
        async move {
            connect_once(Arc::clone(&config), Arc::clone(&server))
                .await
                .unwrap();
            connect_once(config, server).await.unwrap()
        }
    };
    let with_identity = config_for(&catalog, select(Some("add"), Some("client")));
    assert_eq!(
        second_handshake(with_identity, true).await,
        Some(HandshakeKind::Full)
    );
    let without = config_for(&catalog, select(Some("add"), None));
    assert_eq!(
        second_handshake(without, false).await,
        Some(HandshakeKind::Resumed)
    );
}

/// A configuration built while its identity had lapsed still never resumes:
/// once the credential is renewed and lapses again, a server requiring mTLS
/// must see a full handshake with no certificate, not a resumed session
/// carrying the old authentication.
#[tokio::test]
async fn a_lapse_while_building_does_not_turn_resumption_on() {
    use rustls::HandshakeKind;
    let pki = Pki::new();
    let identity = RotatingClientIdentity::load(&pki.identities()["client"].identity()).unwrap();
    identity.set_lapsed(true);
    let mut roots = rustls::RootCertStore::empty();
    roots.add(pki.ca_der.clone()).unwrap();
    let base = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_client_cert_resolver(Arc::clone(&identity) as _);
    let config = Arc::new(crate::host::http_client::isolated_resumption(&base));
    let server = pki.server_config(true);

    identity.set_lapsed(false);
    for _ in 0..2 {
        assert_eq!(
            connect_once(Arc::clone(&config), Arc::clone(&server))
                .await
                .unwrap(),
            Some(HandshakeKind::Full)
        );
    }
    identity.set_lapsed(true);
    assert!(connect_once(config, server).await.is_err());
}

/// One connection through `client` to `server`, echoing a line and reading to
/// the end so any session ticket is received; returns how it was established.
async fn connect_once(
    client: Arc<rustls::ClientConfig>,
    server: Arc<rustls::ServerConfig>,
) -> std::io::Result<Option<rustls::HandshakeKind>> {
    let (client_io, server_io) = tokio::io::duplex(16 * 1024);
    let server_task = tokio::spawn(async move {
        let mut tls = tokio_rustls::TlsAcceptor::from(server)
            .accept(server_io)
            .await?;
        let mut buf = [0u8; 5];
        tls.read_exact(&mut buf).await?;
        tls.write_all(&buf).await?;
        tls.shutdown().await
    });
    let name = rustls::pki_types::ServerName::try_from("cluster.internal").unwrap();
    let mut tls = tokio_rustls::TlsConnector::from(client)
        .connect(name, client_io)
        .await?;
    tls.write_all(b"PING\n").await?;
    let mut buf = Vec::new();
    tls.read_to_end(&mut buf).await?;
    let kind = tls.get_ref().1.handshake_kind();
    server_task.await.unwrap()?;
    Ok(kind)
}

#[tokio::test]
async fn add_trusts_the_private_ca_beside_public_roots() {
    let pki = Pki::new();
    let config = config_for(&pki.catalog(), select(Some("add"), None));
    handshake(config, pki.server_config(false)).await.unwrap();
}

#[tokio::test]
async fn replace_trusts_only_the_private_ca() {
    let pki = Pki::new();
    let config = config_for(&pki.catalog(), select(Some("replace"), None));
    handshake(config, pki.server_config(false)).await.unwrap();
}

#[tokio::test]
async fn the_default_roots_refuse_a_private_ca() {
    let pki = Pki::new();
    let config = config_for(&pki.catalog(), TlsGrant::default());
    let err = handshake(config, pki.server_config(false))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("UnknownIssuer"), "got: {err}");
}

#[tokio::test]
async fn a_selected_identity_satisfies_mtls() {
    let pki = Pki::new();
    let catalog = pki.catalog();
    let with_identity = config_for(&catalog, select(Some("add"), Some("client")));
    handshake(with_identity, pki.server_config(true))
        .await
        .unwrap();

    let without = config_for(&catalog, select(Some("add"), None));
    assert!(handshake(without, pki.server_config(true)).await.is_err());
}

#[test]
fn no_tls_block_declares_no_policy() {
    let pki = Pki::new();
    assert!(
        policy(&pki.catalog(), &[("*", None), ("a.example", None)])
            .unwrap()
            .is_none()
    );
}

/// An identity granted to one endpoint is not presented to another endpoint
/// of the same host that the plugin may also reach.
#[test]
fn a_tls_block_covers_only_the_endpoint_it_grants() {
    let pki = Pki::new();
    let catalog = pki.catalog();
    let private = select(Some("add"), Some("client"));
    let policy = policy(
        &catalog,
        &[
            ("tls://shared.internal:4222", Some(private.clone())),
            ("https://shared.internal:8443", None),
        ],
    )
    .unwrap()
    .unwrap();
    let at = |host, port, scheme| policy.for_endpoint(host, port, &[scheme]).unwrap();
    assert_eq!(
        at("shared.internal", 4222, "tls").unwrap().grant(),
        &private
    );
    assert!(at("SHARED.internal.", 4222, "TLS").is_some());
    assert!(at("shared.internal", 8443, "https").is_none());
    assert!(at("shared.internal", 4222, "https").is_none());
    assert!(policy.declares_tls_at("shared.internal", 4222));
    assert!(!policy.declares_tls_at("shared.internal", 8443));
    assert!(policy.for_server_name("shared.internal").is_none());
}

/// URL parsing drops a written default port, so a scheme-pinned entry means
/// that scheme's default port rather than every port.
#[test]
fn a_default_port_is_pinned_whether_written_or_not() {
    let pki = Pki::new();
    let catalog = pki.catalog();
    let private = select(Some("add"), Some("client"));
    for entry in [
        "https://cluster.internal:443",
        "https://cluster.internal",
        "https://*.corp.internal",
    ] {
        let policy = policy(&catalog, &[(entry, Some(private.clone()))])
            .unwrap()
            .unwrap();
        let host = if entry.contains('*') {
            "api.corp.internal"
        } else {
            "cluster.internal"
        };
        let at = |port| policy.for_endpoint(host, port, &["https"]).unwrap();
        assert!(at(443).is_some(), "{entry}");
        assert!(at(8443).is_none(), "{entry}");
    }
    // A scheme with no default port still grants every port.
    let any_port = policy(&catalog, &[("tls://cluster.internal", Some(private))])
        .unwrap()
        .unwrap();
    assert!(
        any_port
            .for_endpoint("cluster.internal", 4222, &["tls"])
            .unwrap()
            .is_some()
    );
}

/// A lookup names its protocol's schemes: an entry for another protocol on the
/// same port never applies, and two of the protocol's own aliases that
/// disagree are refused rather than chosen by declaration order.
#[test]
fn a_lookup_applies_only_its_protocols_schemes() {
    let pki = Pki::new();
    let catalog = pki.catalog();
    let https_only = policy(
        &catalog,
        &[(
            "https://broker.internal:8443",
            Some(select(Some("add"), Some("client"))),
        )],
    )
    .unwrap()
    .unwrap();
    assert!(
        https_only
            .for_endpoint("broker.internal", 8443, &["nats", "tls"])
            .unwrap()
            .is_none()
    );

    let one = select(Some("add"), None);
    let aliases = policy(
        &catalog,
        &[
            ("nats://broker.internal:4222", Some(one.clone())),
            ("tls://broker.internal:4222", Some(one.clone())),
        ],
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        aliases
            .for_endpoint("broker.internal", 4222, &["nats", "tls"])
            .unwrap()
            .unwrap()
            .grant(),
        &one
    );

    for order in [false, true] {
        let mut entries = vec![
            ("nats://broker.internal:4222", Some(one.clone())),
            (
                "tls://broker.internal:4222",
                Some(select(Some("replace"), Some("client"))),
            ),
        ];
        if order {
            entries.reverse();
        }
        let err = policy(&catalog, &entries)
            .unwrap()
            .unwrap()
            .for_endpoint("broker.internal", 4222, &["nats", "tls"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("both grant this endpoint"), "{err}");
    }
}

#[test]
fn two_services_behind_one_name_select_their_own_tls() {
    let pki = Pki::new();
    let catalog = pki.catalog();
    let db = select(Some("add"), Some("client"));
    let admin = select(Some("replace"), None);
    let policy = policy(
        &catalog,
        &[
            ("db.internal:5432", Some(db.clone())),
            ("db.internal:8443", Some(admin.clone())),
        ],
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        policy
            .for_endpoint("db.internal", 5432, &["tcp"])
            .unwrap()
            .unwrap()
            .grant(),
        &db
    );
    assert_eq!(
        policy
            .for_endpoint("db.internal", 8443, &["https"])
            .unwrap()
            .unwrap()
            .grant(),
        &admin
    );
    assert!(
        policy
            .for_endpoint("db.internal", 9000, &["tcp"])
            .unwrap()
            .is_none()
    );
}

/// A client that knows only the server name gets an entry granting the whole
/// host, never one pinned to a port it cannot see.
#[test]
fn only_a_host_wide_entry_answers_by_server_name() {
    let pki = Pki::new();
    let catalog = pki.catalog();
    let host_wide = select(Some("add"), None);
    let pinned = select(Some("replace"), Some("client"));
    let policy = policy(
        &catalog,
        &[
            ("cluster.internal", Some(host_wide.clone())),
            ("cluster.internal:11207", Some(pinned.clone())),
            ("tls://cluster.internal", Some(pinned.clone())),
        ],
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        policy.for_server_name("CLUSTER.internal.").unwrap().grant(),
        &host_wide
    );
    assert_eq!(
        policy
            .for_endpoint("cluster.internal", 11207, &["tcp"])
            .unwrap()
            .unwrap()
            .grant(),
        &pinned
    );
    assert_eq!(
        policy
            .for_endpoint("cluster.internal", 18091, &["tls"])
            .unwrap()
            .unwrap()
            .grant(),
        &pinned
    );
    assert_eq!(
        policy
            .for_endpoint("cluster.internal", 18091, &["https"])
            .unwrap()
            .unwrap()
            .grant(),
        &host_wide
    );
    assert!(policy.for_server_name("other.internal").is_none());
}

#[test]
fn the_most_specific_tls_entry_wins() {
    let pki = Pki::new();
    let catalog = pki.catalog();
    let public = TlsGrant::default();
    let private = select(Some("add"), Some("client"));
    let replace = select(Some("replace"), None);
    let policy = policy(
        &catalog,
        &[
            ("*", Some(public.clone())),
            ("*.corp.internal", Some(replace.clone())),
            ("cluster.internal", Some(private.clone())),
        ],
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        policy.for_server_name("cluster.internal").unwrap().grant(),
        &private
    );
    assert_eq!(
        policy
            .for_server_name("other.corp.internal")
            .unwrap()
            .grant(),
        &replace
    );
    assert_eq!(
        policy.for_server_name("api.example.com").unwrap().grant(),
        &public
    );
}

#[test]
fn one_scope_with_different_tls_is_refused() {
    let pki = Pki::new();
    let catalog = pki.catalog();
    for (a, b) in [
        ("cluster.internal", "CLUSTER.internal"),
        ("cluster.internal:11207", "cluster.internal:11207"),
        ("tls://cluster.internal:4222", "TLS://cluster.internal:4222"),
        ("10.0.0.5:4222", "[::ffff:10.0.0.5]:4222"),
        ("*.corp.internal.", "*.corp.internal"),
    ] {
        let err = policy(
            &catalog,
            &[
                (a, Some(TlsGrant::default())),
                (b, Some(select(Some("add"), None))),
            ],
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("same host, port and scheme"),
            "{a} / {b}: {err}"
        );
    }
    for (a, b) in [
        ("cluster.internal:11207", "cluster.internal:18091"),
        ("tls://cluster.internal:4222", "cluster.internal"),
        ("*.corp.internal:1", "*.corp.internal:2"),
    ] {
        policy(
            &catalog,
            &[
                (a, Some(TlsGrant::default())),
                (b, Some(select(Some("add"), None))),
            ],
        )
        .unwrap()
        .unwrap();
    }
}

#[test]
fn wildcard_tls_names_ignore_the_trailing_dot() {
    let pki = Pki::new();
    let policy = policy(
        &pki.catalog(),
        &[("*.internal.", Some(TlsGrant::default()))],
    )
    .unwrap()
    .unwrap();
    for name in ["cluster.internal", "CLUSTER.internal."] {
        assert!(policy.for_server_name(name).is_some(), "{name}");
    }
    assert!(policy.for_server_name("internal").is_none());
    assert!(policy.for_server_name("notinternal").is_none());
}

#[test]
fn an_address_is_matched_as_an_address() {
    let pki = Pki::new();
    let policy = policy(
        &pki.catalog(),
        &[
            ("10.0.0.5:4222", Some(TlsGrant::default())),
            ("10.0.0.7", Some(TlsGrant::default())),
        ],
    )
    .unwrap()
    .unwrap();
    assert!(
        policy
            .for_endpoint("[::ffff:10.0.0.5]", 4222, &["tcp"])
            .unwrap()
            .is_some()
    );
    assert!(
        policy
            .for_endpoint("10.0.0.6", 4222, &["tcp"])
            .unwrap()
            .is_none()
    );
    assert!(policy.for_server_name("::ffff:10.0.0.7").is_some());
    assert!(policy.for_server_name("10.0.0.5").is_none());
}

#[test]
fn the_order_entries_are_written_in_does_not_change_the_declaration() {
    let pki = Pki::new();
    let catalog = pki.catalog();
    let entries = [
        ("cluster.internal", Some(select(Some("add"), None))),
        ("*.example", Some(TlsGrant::default())),
    ];
    let forward = policy(&catalog, &entries).unwrap().unwrap();
    let mut reversed = entries.clone();
    reversed.reverse();
    let backward = policy(&catalog, &reversed).unwrap().unwrap();
    assert!(forward.same_declaration(&backward));

    // Two ports of one host, written either way round, are one declaration.
    let ports = [
        ("cluster.internal:5432", Some(TlsGrant::default())),
        ("cluster.internal:8080", Some(TlsGrant::default())),
    ];
    let mut swapped = ports.clone();
    swapped.reverse();
    assert!(
        policy(&catalog, &ports)
            .unwrap()
            .unwrap()
            .same_declaration(&policy(&catalog, &swapped).unwrap().unwrap())
    );

    // The same names resolved against another catalog are other material.
    let elsewhere = policy(&pki.catalog(), &entries).unwrap().unwrap();
    assert!(!forward.same_declaration(&elsewhere));

    let changed = policy(
        &catalog,
        &[
            ("cluster.internal", Some(select(Some("replace"), None))),
            ("*.example", Some(TlsGrant::default())),
        ],
    )
    .unwrap()
    .unwrap();
    assert!(!forward.same_declaration(&changed));
}

#[tokio::test]
async fn failed_catalog_load_aborts_started_refresh_tasks() {
    let pki = Pki::new();
    let runtime = tokio::runtime::Handle::current();
    let baseline = runtime.metrics().num_alive_tasks();
    for missing_cert in [true, false] {
        let mut identities = pki.identities();
        let first = identities.get_mut("client").unwrap();
        first.refresh = Some(Duration::from_secs(30));
        let mut bad = first.clone();
        if missing_cert {
            bad.cert = pki.path("missing.crt");
        } else {
            bad.refresh = Some(Duration::ZERO);
        }
        identities.insert("z-bad".into(), bad);
        assert!(TlsCatalog::load(&BTreeMap::new(), &identities).is_err());
        tokio::task::yield_now().await;
        assert_eq!(runtime.metrics().num_alive_tasks(), baseline);
    }
}

#[tokio::test]
async fn dropping_catalog_aborts_refresh_tasks() {
    let pki = Pki::new();
    let runtime = tokio::runtime::Handle::current();
    let baseline = runtime.metrics().num_alive_tasks();
    let mut identities = pki.identities();
    identities.get_mut("client").unwrap().refresh = Some(Duration::from_secs(30));
    let catalog = TlsCatalog::load(&BTreeMap::new(), &identities).unwrap();
    assert_eq!(runtime.metrics().num_alive_tasks(), baseline + 1);
    drop(catalog);
    tokio::task::yield_now().await;
    assert_eq!(runtime.metrics().num_alive_tasks(), baseline);
}
