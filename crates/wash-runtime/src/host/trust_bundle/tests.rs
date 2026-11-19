use aws_lc_rs::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _};
use rcgen::{
    BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, SanType,
};

use super::*;

const PEER: &str = "spiffe://peer.test/wi/a";

struct Authority {
    issuer: Issuer<'static, KeyPair>,
    der: CertificateDer<'static>,
}

fn authority() -> Authority {
    let key = KeyPair::generate().unwrap();
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let der = params.self_signed(&key).unwrap().der().clone();
    Authority {
        issuer: Issuer::new(params, key),
        der,
    }
}

impl Authority {
    fn leaf(&self, uris: &[&str], eku: &[ExtendedKeyUsagePurpose]) -> CertificateDer<'static> {
        let key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.subject_alt_names = uris
            .iter()
            .map(|uri| SanType::URI((*uri).try_into().unwrap()))
            .collect();
        params.extended_key_usages = eku.to_vec();
        params.signed_by(&key, &self.issuer).unwrap().der().clone()
    }

    fn svid(&self, id: &str) -> Vec<CertificateDer<'static>> {
        vec![self.leaf(
            &[id],
            &[
                ExtendedKeyUsagePurpose::ClientAuth,
                ExtendedKeyUsagePurpose::ServerAuth,
            ],
        )]
    }
}

fn signing_key() -> EcdsaKeyPair {
    EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING).unwrap()
}

fn jwk(kid: &str, key: &EcdsaKeyPair) -> serde_json::Value {
    let point = key.public_key().as_ref();
    let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    serde_json::json!({
        "kty": "EC", "use": "jwt-svid", "kid": kid, "crv": "P-256",
        "x": enc.encode(&point[1..33]), "y": enc.encode(&point[33..]),
    })
}

fn x509_jwk(der: &CertificateDer<'_>) -> serde_json::Value {
    serde_json::json!({
        "kty": "EC", "use": "x509-svid",
        "x5c": [base64::engine::general_purpose::STANDARD.encode(der)],
    })
}

fn bundle(domain: &str, keys: Vec<serde_json::Value>) -> TrustBundle {
    let json =
        serde_json::json!({ "keys": keys, "spiffe_sequence": 7, "spiffe_refresh_hint": 300 });
    TrustBundle::from_spiffe_json(
        TrustDomain::new(domain).unwrap(),
        json.to_string().as_bytes(),
    )
    .unwrap()
}

fn sign(key: &EcdsaKeyPair, header: serde_json::Value, claims: serde_json::Value) -> String {
    let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let signing = format!(
        "{}.{}",
        enc.encode(header.to_string()),
        enc.encode(claims.to_string())
    );
    let signature = key
        .sign(&aws_lc_rs::rand::SystemRandom::new(), signing.as_bytes())
        .unwrap();
    format!("{signing}.{}", enc.encode(signature.as_ref()))
}

fn in_secs(secs: i64) -> u64 {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    (now + secs) as u64
}

fn later() -> SystemTime {
    SystemTime::now() + Duration::from_secs(60)
}

fn store(bundles: Vec<TrustBundle>) -> TrustBundles {
    let store = TrustBundles::new();
    store.replace(bundles, later()).unwrap();
    store
}

#[test]
fn spiffe_ids_follow_the_spec() {
    let id = SpiffeId::parse("spiffe://acme.test/wi/a-1_b.c").unwrap();
    assert_eq!(id.trust_domain(), "acme.test");
    assert_eq!(id.path(), "/wi/a-1_b.c");
    assert_eq!(SpiffeId::parse("spiffe://acme.test").unwrap().path(), "");
    for bad in [
        "spiffe://Acme.test/a",
        "spiffe:///a",
        "spiffe://acme.test/",
        "spiffe://acme.test//a",
        "spiffe://acme.test/./a",
        "spiffe://acme.test/../a",
        "spiffe://acme.test:443/a",
        "spiffe://user@acme.test/a",
        "spiffe://acme.test/a?b",
        "spiffe://acme.test/a#b",
        "https://acme.test/a",
    ] {
        assert!(SpiffeId::parse(bad).is_err(), "{bad} parsed");
    }
    assert!(SpiffeId::parse(&format!("spiffe://a.test/{}", "x".repeat(2048))).is_err());
}

#[test]
fn a_peer_is_a_trust_domain_or_one_id() {
    let domain = SpiffePeer::parse("spiffe://peer.test").unwrap();
    let one = SpiffePeer::parse(PEER).unwrap();
    let a = SpiffeId::parse(PEER).unwrap();
    let b = SpiffeId::parse("spiffe://peer.test/wi/b").unwrap();
    let other = SpiffeId::parse("spiffe://other.test/wi/a").unwrap();
    assert!(domain.admits(&a) && domain.admits(&b) && !domain.admits(&other));
    assert!(one.admits(&a) && !one.admits(&b));
    assert_eq!(one.trust_domain(), "peer.test");
}

#[test]
fn a_bundle_reads_the_spiffe_format() {
    let ca = authority();
    let key = signing_key();
    let mut keys = vec![x509_jwk(&ca.der), jwk("k1", &key)];
    keys.push(serde_json::json!({ "kty": "EC", "use": "enc", "crv": "P-256" }));
    let bundle = bundle("peer.test", keys);
    assert_eq!(bundle.x509_authorities().len(), 1);
    assert_eq!(bundle.jwt_key_ids().collect::<Vec<_>>(), ["k1"]);
    assert_eq!(bundle.sequence(), Some(7));
    assert_eq!(bundle.refresh_hint(), Some(Duration::from_secs(300)));
}

#[test]
fn a_malformed_bundle_is_refused() {
    let domain = || TrustDomain::new("peer.test").unwrap();
    let key = signing_key();
    let leaf = authority().leaf(&[PEER], &[]);
    let mut no_kid = jwk("k1", &key);
    no_kid.as_object_mut().unwrap().remove("kid");
    let mut no_use = jwk("k1", &key);
    no_use.as_object_mut().unwrap().remove("use");
    let mut hmac = jwk("k1", &key);
    hmac["alg"] = "HS256".into();
    let mut off_curve = jwk("k1", &key);
    off_curve["x"] = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode([7u8; 32])
        .into();
    for (why, keys) in [
        ("no kid", vec![no_kid]),
        ("no use", vec![no_use]),
        ("HMAC", vec![hmac]),
        ("off curve", vec![off_curve]),
        (
            "duplicate kid",
            vec![jwk("k1", &key), jwk("k1", &signing_key())],
        ),
        ("leaf as authority", vec![x509_jwk(&leaf)]),
        (
            "two x5c",
            vec![serde_json::json!({ "kty": "EC", "use": "x509-svid", "x5c": ["AA==", "AA=="] })],
        ),
    ] {
        let json = serde_json::json!({ "keys": keys }).to_string();
        assert!(
            TrustBundle::from_spiffe_json(domain(), json.as_bytes()).is_err(),
            "{why} was accepted"
        );
    }
    let huge = vec![b' '; MAX_BUNDLE_BYTES + 1];
    assert!(TrustBundle::from_spiffe_json(domain(), &huge).is_err());
    let empty = TrustBundle::from_spiffe_json(domain(), br#"{"keys":[]}"#).unwrap();
    assert!(TrustBundles::new().insert(empty, later()).is_err());
}

#[test]
fn an_x509_svid_verifies_only_against_its_own_trust_domain() {
    let peer = authority();
    let other = authority();
    let bundles = store(vec![
        bundle("peer.test", vec![x509_jwk(&peer.der)]),
        bundle("other.test", vec![x509_jwk(&other.der)]),
    ]);
    let now = UnixTime::now();
    let id = bundles
        .verify_x509_svid(&peer.svid(PEER), SvidUsage::Client, now)
        .unwrap();
    assert_eq!(id.as_str(), PEER);
    let err = bundles
        .verify_x509_svid(&other.svid(PEER), SvidUsage::Client, now)
        .unwrap_err();
    assert!(err.to_string().contains("does not chain"), "{err}");
    let err = bundles
        .verify_x509_svid(&peer.svid("spiffe://nobody.test/a"), SvidUsage::Client, now)
        .unwrap_err();
    assert!(err.to_string().contains("no current trust bundle"), "{err}");
}

#[test]
fn an_x509_svid_must_name_exactly_one_spiffe_id_on_a_non_ca_leaf() {
    let ca = authority();
    let bundles = store(vec![bundle("peer.test", vec![x509_jwk(&ca.der)])]);
    let now = UnixTime::now();
    for chain in [
        vec![ca.leaf(&[PEER, "spiffe://peer.test/wi/b"], &[])],
        vec![ca.leaf(&["https://peer.test/a"], &[])],
        vec![ca.leaf(&["spiffe://peer.test"], &[])],
        vec![ca.der.clone()],
        vec![],
    ] {
        assert!(
            bundles
                .verify_x509_svid(&chain, SvidUsage::Client, now)
                .is_err()
        );
    }
}

#[test]
fn a_client_only_svid_cannot_serve() {
    let ca = authority();
    let bundles = store(vec![bundle("peer.test", vec![x509_jwk(&ca.der)])]);
    let client_only = vec![ca.leaf(&[PEER], &[ExtendedKeyUsagePurpose::ClientAuth])];
    let now = UnixTime::now();
    assert!(
        bundles
            .verify_x509_svid(&client_only, SvidUsage::Client, now)
            .is_ok()
    );
    assert!(
        bundles
            .verify_x509_svid(&client_only, SvidUsage::Server, now)
            .is_err()
    );
}

#[test]
fn a_bundle_past_its_time_verifies_nothing() {
    let ca = authority();
    let bundles = TrustBundles::new();
    bundles
        .insert(
            bundle("peer.test", vec![x509_jwk(&ca.der)]),
            SystemTime::now() + Duration::from_millis(50),
        )
        .unwrap();
    assert!(bundles.get("peer.test").is_some());
    std::thread::sleep(Duration::from_millis(80));
    assert!(bundles.get("peer.test").is_none());
    assert!(bundles.trust_domains().is_empty());
    let err = bundles
        .verify_x509_svid(&ca.svid(PEER), SvidUsage::Client, UnixTime::now())
        .unwrap_err();
    assert!(err.to_string().contains("no current trust bundle"), "{err}");
    assert!(
        bundles
            .replace(Vec::new(), SystemTime::now() - Duration::from_secs(1))
            .is_err(),
        "a time already past is refused"
    );
}

#[tokio::test]
async fn removing_a_trust_domain_tells_its_connections_and_a_refresh_does_not() {
    let ca = authority();
    let bundles = store(vec![bundle("peer.test", vec![x509_jwk(&ca.der)])]);
    let dropped = bundles.dropped("peer.test").unwrap();
    bundles
        .replace(vec![bundle("peer.test", vec![x509_jwk(&ca.der)])], later())
        .unwrap();
    let mut dropped = std::pin::pin!(dropped);
    assert!(
        futures::poll!(dropped.as_mut()).is_pending(),
        "a refresh keeps connections"
    );
    bundles.replace(Vec::new(), later()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), dropped)
        .await
        .expect("a removal closes connections");
    assert!(bundles.dropped("peer.test").is_none());
    assert!(
        bundles
            .replace(
                vec![
                    bundle("peer.test", vec![x509_jwk(&ca.der)]),
                    bundle("peer.test", vec![x509_jwk(&ca.der)]),
                ],
                later(),
            )
            .is_err()
    );
}

#[test]
fn a_jwt_svid_verifies_against_its_trust_domains_keys() {
    let key = signing_key();
    let bundles = store(vec![bundle("peer.test", vec![jwk("k1", &key)])]);
    let header = serde_json::json!({ "alg": "ES256", "kid": "k1", "typ": "JWT" });
    let claims = serde_json::json!({ "sub": PEER, "aud": ["svc", "x"], "exp": in_secs(300) });
    let token = sign(&key, header.clone(), claims.clone());
    let verified = bundles.verify_jwt_svid(&token, "svc").unwrap();
    assert_eq!(verified.spiffe_id.as_str(), PEER);
    assert_eq!(verified.audience, ["svc", "x"]);
    assert!(bundles.verify_jwt_svid(&token, "other").is_err());

    let mut tampered = token.clone();
    tampered.insert(tampered.find('.').unwrap() + 2, 'A');
    assert!(bundles.verify_jwt_svid(&tampered, "svc").is_err());
}

#[test]
fn a_jwt_svid_is_refused_unless_every_rule_holds() {
    let key = signing_key();
    let stranger = signing_key();
    let bundles = store(vec![bundle("peer.test", vec![jwk("k1", &key)])]);
    let header = serde_json::json!({ "alg": "ES256", "kid": "k1" });
    let claims = |sub: &str, exp: u64| serde_json::json!({ "sub": sub, "aud": "svc", "exp": exp });
    let mut cases = vec![
        (
            "expired",
            sign(&key, header.clone(), claims(PEER, in_secs(-5))),
        ),
        (
            "unknown kid",
            sign(
                &key,
                serde_json::json!({ "alg": "ES256", "kid": "k2" }),
                claims(PEER, in_secs(60)),
            ),
        ),
        (
            "no kid",
            sign(
                &key,
                serde_json::json!({ "alg": "ES256" }),
                claims(PEER, in_secs(60)),
            ),
        ),
        (
            "HS256",
            sign(
                &key,
                serde_json::json!({ "alg": "HS256", "kid": "k1" }),
                claims(PEER, in_secs(60)),
            ),
        ),
        (
            "crit",
            sign(
                &key,
                serde_json::json!({ "alg": "ES256", "kid": "k1", "crit": ["b64"] }),
                claims(PEER, in_secs(60)),
            ),
        ),
        (
            "another key",
            sign(&stranger, header.clone(), claims(PEER, in_secs(60))),
        ),
        (
            "no bundle",
            sign(
                &key,
                header.clone(),
                claims("spiffe://other.test/a", in_secs(60)),
            ),
        ),
        (
            "no path",
            sign(
                &key,
                header.clone(),
                claims("spiffe://peer.test", in_secs(60)),
            ),
        ),
        (
            "exp out of range",
            sign(&key, header.clone(), claims(PEER, u64::MAX)),
        ),
        (
            "nbf out of range",
            sign(
                &key,
                header.clone(),
                serde_json::json!({ "sub": PEER, "aud": "svc", "exp": in_secs(600), "nbf": u64::MAX }),
            ),
        ),
        (
            "not yet",
            sign(
                &key,
                header.clone(),
                serde_json::json!({ "sub": PEER, "aud": "svc", "exp": in_secs(600), "nbf": in_secs(300) }),
            ),
        ),
    ];
    let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    cases.push((
        "alg none",
        format!(
            "{}.{}.",
            enc.encode(r#"{"alg":"none","kid":"k1"}"#),
            enc.encode(claims(PEER, in_secs(60)).to_string())
        ),
    ));
    for (why, token) in cases {
        assert!(
            bundles.verify_jwt_svid(&token, "svc").is_err(),
            "{why} verified"
        );
    }
}

#[test]
fn the_server_verifier_admits_only_its_peer() {
    let ca = authority();
    let bundles = store(vec![bundle("peer.test", vec![x509_jwk(&ca.der)])]);
    let chain = ca.svid(PEER);
    let name = ServerName::try_from("ignored.example").unwrap();
    let verify = |peer: &str| {
        bundles
            .server_verifier(SpiffePeer::parse(peer).unwrap())
            .verify_server_cert(&chain[0], &[], &name, &[], UnixTime::now())
            .is_ok()
    };
    assert!(verify("spiffe://peer.test"));
    assert!(verify(PEER));
    assert!(!verify("spiffe://peer.test/wi/b"));
    assert!(!verify("spiffe://other.test"));
}

#[test]
fn the_client_verifier_asks_only_while_a_bundle_is_current() {
    let ca = authority();
    let bundles = TrustBundles::new();
    let verifier = bundles.client_verifier();
    assert!(!verifier.offer_client_auth());
    bundles
        .insert(bundle("peer.test", vec![x509_jwk(&ca.der)]), later())
        .unwrap();
    assert!(verifier.offer_client_auth());
    assert!(!verifier.client_auth_mandatory());
    let chain = ca.svid(PEER);
    assert!(
        verifier
            .verify_client_cert(&chain[0], &[], UnixTime::now())
            .is_ok()
    );
    let stranger = authority().svid(PEER);
    assert!(
        verifier
            .verify_client_cert(&stranger[0], &[], UnixTime::now())
            .is_err()
    );
    bundles.remove("peer.test");
    assert!(!verifier.offer_client_auth());
}
