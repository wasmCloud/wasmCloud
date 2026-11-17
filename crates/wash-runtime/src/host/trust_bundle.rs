//! SPIFFE trust bundles, and peer verification against them.
//!
//! A bundle holds one trust domain's X.509 authorities and JWT-SVID signing
//! keys, in the SPIFFE bundle format. The host holds one per trust domain it
//! verifies peers from: its own and each federated peer's. Whoever installs a
//! bundle gives it a time it is good until, and a bundle not refreshed by then
//! counts as missing, so every check against it fails closed.
//!
//! Verification here authenticates a peer and authorizes nothing: the caller
//! decides what a verified SPIFFE ID may do.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwap;
use base64::Engine as _;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, ServerName, TrustAnchor, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{CertificateError, DistinguishedName, OtherError};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

const MAX_SPIFFE_ID: usize = 2048;
const MAX_TRUST_DOMAIN: usize = 255;
/// Larger than any real bundle, small enough that a hostile endpoint cannot
/// make the host hold much.
pub const MAX_BUNDLE_BYTES: usize = 256 * 1024;
const MAX_JWT: usize = 16 * 1024;
/// How far a JWT-SVID's `nbf` may be ahead of this host's clock.
const NBF_SKEW: Duration = Duration::from_secs(60);

/// A SPIFFE trust domain name, lowercase as the SPIFFE ID spec requires.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TrustDomain(String);

impl TrustDomain {
    /// `name` bare or as `spiffe://name`.
    pub fn new(name: &str) -> anyhow::Result<Self> {
        let name = name.strip_prefix("spiffe://").unwrap_or(name);
        anyhow::ensure!(
            !name.is_empty()
                && name.len() <= MAX_TRUST_DOMAIN
                && name
                    .bytes()
                    .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_')),
            "{name:?} is not a trust domain: lowercase letters, digits, '.', '-' and '_' only"
        );
        Ok(Self(name.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TrustDomain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A SPIFFE ID: `spiffe://<trust domain>` and an optional path.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SpiffeId {
    id: String,
    path_at: usize,
}

impl SpiffeId {
    pub fn parse(id: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            id.len() <= MAX_SPIFFE_ID,
            "a SPIFFE ID is at most {MAX_SPIFFE_ID} bytes"
        );
        let rest = id
            .strip_prefix("spiffe://")
            .ok_or_else(|| anyhow::anyhow!("{id:?} is not a spiffe:// URI"))?;
        let (domain, path) = rest.find('/').map_or((rest, ""), |at| rest.split_at(at));
        TrustDomain::new(domain).map_err(|e| anyhow::anyhow!("SPIFFE ID {id:?}: {e}"))?;
        if !path.is_empty() {
            for segment in path[1..].split('/') {
                anyhow::ensure!(
                    !segment.is_empty()
                        && segment != "."
                        && segment != ".."
                        && segment
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')),
                    "SPIFFE ID {id:?} has a path segment that is empty, a dot segment, or \
                     outside [A-Za-z0-9._-]"
                );
            }
        }
        Ok(Self {
            id: id.to_string(),
            path_at: "spiffe://".len() + domain.len(),
        })
    }

    pub fn trust_domain(&self) -> &str {
        &self.id["spiffe://".len()..self.path_at]
    }

    /// Empty for the ID of the trust domain itself.
    pub fn path(&self) -> &str {
        &self.id[self.path_at..]
    }

    pub fn as_str(&self) -> &str {
        &self.id
    }
}

impl std::fmt::Display for SpiffeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.id)
    }
}

/// Which peers a verifier admits: any ID in one trust domain, or one ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpiffePeer {
    TrustDomain(TrustDomain),
    Id(SpiffeId),
}

impl SpiffePeer {
    /// `spiffe://<trust domain>` admits the whole trust domain; a SPIFFE ID
    /// with a path admits that ID alone.
    pub fn parse(peer: &str) -> anyhow::Result<Self> {
        let id = SpiffeId::parse(peer)?;
        Ok(match id.path() {
            "" => Self::TrustDomain(TrustDomain(id.trust_domain().to_string())),
            _ => Self::Id(id),
        })
    }

    pub fn trust_domain(&self) -> &str {
        match self {
            Self::TrustDomain(domain) => domain.as_str(),
            Self::Id(id) => id.trust_domain(),
        }
    }

    pub fn admits(&self, id: &SpiffeId) -> bool {
        match self {
            Self::TrustDomain(domain) => id.trust_domain() == domain.as_str(),
            Self::Id(expected) => id == expected,
        }
    }
}

impl std::fmt::Display for SpiffePeer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TrustDomain(domain) => write!(f, "spiffe://{domain}"),
            Self::Id(id) => id.fmt(f),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JwtAlg {
    Es256,
    Es384,
    Rs256,
    Rs384,
    Rs512,
    Ps256,
    Ps384,
    Ps512,
}

impl JwtAlg {
    /// The algorithms the JWT-SVID spec allows. `none` and HMAC are not.
    fn parse(alg: &str) -> Option<Self> {
        Some(match alg {
            "ES256" => Self::Es256,
            "ES384" => Self::Es384,
            "RS256" => Self::Rs256,
            "RS384" => Self::Rs384,
            "RS512" => Self::Rs512,
            "PS256" => Self::Ps256,
            "PS384" => Self::Ps384,
            "PS512" => Self::Ps512,
            _ => return None,
        })
    }
}

#[derive(Clone)]
enum JwtKeyMaterial {
    /// An uncompressed point, and whether the curve is P-384.
    Ec {
        point: Vec<u8>,
        p384: bool,
    },
    Rsa {
        n: Vec<u8>,
        e: Vec<u8>,
    },
}

#[derive(Clone)]
struct JwtKey {
    material: JwtKeyMaterial,
    /// The `alg` the JWK pins, if it names one.
    alg: Option<JwtAlg>,
}

impl JwtKey {
    fn verify(&self, alg: JwtAlg, message: &[u8], signature: &[u8]) -> anyhow::Result<()> {
        use aws_lc_rs::signature as sig;
        if let Some(pinned) = self.alg {
            anyhow::ensure!(pinned == alg, "the key is for {pinned:?}, not {alg:?}");
        }
        let verified = match (&self.material, alg) {
            (JwtKeyMaterial::Ec { point, p384: false }, JwtAlg::Es256) => {
                sig::UnparsedPublicKey::new(&sig::ECDSA_P256_SHA256_FIXED, point)
                    .verify(message, signature)
            }
            (JwtKeyMaterial::Ec { point, p384: true }, JwtAlg::Es384) => {
                sig::UnparsedPublicKey::new(&sig::ECDSA_P384_SHA384_FIXED, point)
                    .verify(message, signature)
            }
            (JwtKeyMaterial::Rsa { n, e }, alg) => {
                let params: &sig::RsaParameters = match alg {
                    JwtAlg::Rs256 => &sig::RSA_PKCS1_2048_8192_SHA256,
                    JwtAlg::Rs384 => &sig::RSA_PKCS1_2048_8192_SHA384,
                    JwtAlg::Rs512 => &sig::RSA_PKCS1_2048_8192_SHA512,
                    JwtAlg::Ps256 => &sig::RSA_PSS_2048_8192_SHA256,
                    JwtAlg::Ps384 => &sig::RSA_PSS_2048_8192_SHA384,
                    JwtAlg::Ps512 => &sig::RSA_PSS_2048_8192_SHA512,
                    JwtAlg::Es256 | JwtAlg::Es384 => {
                        anyhow::bail!("an RSA key cannot verify {alg:?}")
                    }
                };
                sig::RsaPublicKeyComponents { n, e }.verify(params, message, signature)
            }
            _ => anyhow::bail!("the key's curve does not match {alg:?}"),
        };
        verified.map_err(|_| anyhow::anyhow!("the signature does not verify"))
    }
}

/// One trust domain's X.509 authorities and JWT-SVID keys.
#[derive(Clone)]
pub struct TrustBundle {
    trust_domain: TrustDomain,
    x509_authorities: Vec<CertificateDer<'static>>,
    anchors: Vec<TrustAnchor<'static>>,
    jwt_keys: HashMap<String, JwtKey>,
    sequence: Option<u64>,
    refresh_hint: Option<Duration>,
}

impl std::fmt::Debug for TrustBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrustBundle")
            .field("trust_domain", &self.trust_domain)
            .field("x509_authorities", &self.x509_authorities.len())
            .field("jwt_keys", &self.jwt_keys.keys().collect::<Vec<_>>())
            .field("sequence", &self.sequence)
            .finish()
    }
}

#[derive(serde::Deserialize)]
struct Jwks {
    keys: Vec<serde_json::Value>,
    #[serde(default)]
    spiffe_sequence: Option<u64>,
    #[serde(default)]
    spiffe_refresh_hint: Option<u64>,
}

#[derive(serde::Deserialize)]
struct Jwk {
    kty: String,
    #[serde(rename = "use")]
    usage: Option<String>,
    kid: Option<String>,
    alg: Option<String>,
    crv: Option<String>,
    x: Option<String>,
    y: Option<String>,
    n: Option<String>,
    e: Option<String>,
    x5c: Option<Vec<String>>,
}

fn b64url(field: &str, value: Option<&String>) -> anyhow::Result<Vec<u8>> {
    let value = value.ok_or_else(|| anyhow::anyhow!("the JWK has no {field}"))?;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| anyhow::anyhow!("the JWK's {field} is not base64url"))
}

impl TrustBundle {
    /// A bundle in the SPIFFE bundle format: a JWK Set whose keys are each
    /// `use: x509-svid` with one `x5c` certificate, or `use: jwt-svid` with a
    /// `kid`. Keys of another `use` are skipped, as the format requires.
    pub fn from_spiffe_json(trust_domain: TrustDomain, json: &[u8]) -> anyhow::Result<Self> {
        anyhow::ensure!(
            json.len() <= MAX_BUNDLE_BYTES,
            "the trust bundle for {trust_domain} is over {MAX_BUNDLE_BYTES} bytes"
        );
        let set: Jwks = serde_json::from_slice(json)
            .map_err(|e| anyhow::anyhow!("the trust bundle for {trust_domain}: {e}"))?;
        let mut authorities = Vec::new();
        let mut jwt_keys = HashMap::new();
        for (index, key) in set.keys.into_iter().enumerate() {
            let key: Jwk = serde_json::from_value(key)
                .map_err(|e| anyhow::anyhow!("key {index} of {trust_domain}'s bundle: {e}"))?;
            let context =
                |e: anyhow::Error| anyhow::anyhow!("key {index} of {trust_domain}'s bundle: {e}");
            match key.usage.as_deref() {
                Some("x509-svid") => authorities.push(x509_authority(&key).map_err(context)?),
                Some("jwt-svid") => {
                    let kid = key
                        .kid
                        .clone()
                        .filter(|kid| !kid.is_empty())
                        .ok_or_else(|| context(anyhow::anyhow!("a jwt-svid key needs a kid")))?;
                    let parsed = jwt_key(&key).map_err(context)?;
                    anyhow::ensure!(
                        jwt_keys.insert(kid.clone(), parsed).is_none(),
                        "{trust_domain}'s bundle has two jwt-svid keys with kid {kid:?}"
                    );
                }
                Some(_) => {}
                None => return Err(context(anyhow::anyhow!("the key has no use"))),
            }
        }
        let mut bundle = Self::from_x509_authorities(trust_domain, authorities)?;
        bundle.jwt_keys = jwt_keys;
        bundle.sequence = set.spiffe_sequence;
        bundle.refresh_hint = set.spiffe_refresh_hint.map(Duration::from_secs);
        Ok(bundle)
    }

    /// A bundle of X.509 authorities only, as from a PEM file.
    pub fn from_x509_authorities(
        trust_domain: TrustDomain,
        authorities: Vec<CertificateDer<'static>>,
    ) -> anyhow::Result<Self> {
        let mut anchors = Vec::with_capacity(authorities.len());
        for authority in &authorities {
            let (_, cert) = x509_parser::parse_x509_certificate(authority)
                .map_err(|e| anyhow::anyhow!("an X.509 authority for {trust_domain}: {e}"))?;
            anyhow::ensure!(
                cert.is_ca(),
                "an X.509 authority for {trust_domain} is not a CA certificate"
            );
            let anchor = webpki::anchor_from_trusted_cert(authority)
                .map_err(|e| anyhow::anyhow!("an X.509 authority for {trust_domain}: {e}"))?;
            anchors.push(anchor.to_owned());
        }
        Ok(Self {
            trust_domain,
            x509_authorities: authorities,
            anchors,
            jwt_keys: HashMap::new(),
            sequence: None,
            refresh_hint: None,
        })
    }

    pub fn trust_domain(&self) -> &TrustDomain {
        &self.trust_domain
    }

    pub fn x509_authorities(&self) -> &[CertificateDer<'static>] {
        &self.x509_authorities
    }

    pub fn jwt_key_ids(&self) -> impl Iterator<Item = &str> {
        self.jwt_keys.keys().map(String::as_str)
    }

    pub fn sequence(&self) -> Option<u64> {
        self.sequence
    }

    pub fn refresh_hint(&self) -> Option<Duration> {
        self.refresh_hint
    }

    fn is_empty(&self) -> bool {
        self.anchors.is_empty() && self.jwt_keys.is_empty()
    }
}

fn x509_authority(key: &Jwk) -> anyhow::Result<CertificateDer<'static>> {
    let [cert] = key.x5c.as_deref().unwrap_or_default() else {
        anyhow::bail!("an x509-svid key carries exactly one x5c certificate");
    };
    let der = base64::engine::general_purpose::STANDARD
        .decode(cert)
        .map_err(|_| anyhow::anyhow!("the x5c certificate is not base64"))?;
    Ok(CertificateDer::from(der))
}

fn jwt_key(key: &Jwk) -> anyhow::Result<JwtKey> {
    let alg = key
        .alg
        .as_deref()
        .map(|alg| JwtAlg::parse(alg).ok_or_else(|| anyhow::anyhow!("alg {alg:?} is not allowed")))
        .transpose()?;
    let material = match key.kty.as_str() {
        "EC" => {
            let (p384, len) = match key.crv.as_deref() {
                Some("P-256") => (false, 32),
                Some("P-384") => (true, 48),
                crv => anyhow::bail!("EC curve {crv:?} is not P-256 or P-384"),
            };
            let (x, y) = (b64url("x", key.x.as_ref())?, b64url("y", key.y.as_ref())?);
            anyhow::ensure!(
                x.len() == len && y.len() == len,
                "the EC point is the wrong size"
            );
            let point = [&[4u8][..], &x, &y].concat();
            let algorithm = if p384 {
                &aws_lc_rs::signature::ECDSA_P384_SHA384_FIXED
            } else {
                &aws_lc_rs::signature::ECDSA_P256_SHA256_FIXED
            };
            aws_lc_rs::signature::ParsedPublicKey::new(algorithm, &point)
                .map_err(|_| anyhow::anyhow!("the EC point is not on the curve"))?;
            JwtKeyMaterial::Ec { point, p384 }
        }
        "RSA" => {
            let n = b64url("n", key.n.as_ref())?;
            let e = b64url("e", key.e.as_ref())?;
            let bits = n.iter().skip_while(|b| **b == 0).count() * 8;
            anyhow::ensure!(bits >= 2048, "an RSA key is at least 2048 bits");
            JwtKeyMaterial::Rsa { n, e }
        }
        kty => anyhow::bail!("key type {kty:?} is not EC or RSA"),
    };
    Ok(JwtKey { material, alg })
}

/// Which way an X.509-SVID is presented, so the right extended key usage is
/// required of it.
#[derive(Clone, Copy, Debug)]
pub enum SvidUsage {
    Server,
    Client,
}

/// What a verified JWT-SVID says.
#[derive(Clone, Debug)]
pub struct JwtSvidClaims {
    pub spiffe_id: SpiffeId,
    pub audience: Vec<String>,
    pub expires_at: SystemTime,
}

struct Held {
    bundle: Arc<TrustBundle>,
    valid_until: SystemTime,
    /// Cancelled when the trust domain is removed, not when it is refreshed.
    dropped: CancellationToken,
}

/// The trust bundles this host verifies peers against, one per trust domain.
///
/// Cloning is cheap and shares the store. Reads take no lock.
#[derive(Clone, Default)]
pub struct TrustBundles {
    held: Arc<ArcSwap<HashMap<String, Arc<Held>>>>,
    write: Arc<Mutex<()>>,
}

impl std::fmt::Debug for TrustBundles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrustBundles")
            .field("trust_domains", &self.trust_domains())
            .finish()
    }
}

impl TrustBundles {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace every bundle at once, each good until `valid_until`. A trust
    /// domain left out is removed, and connections verified against it are
    /// told to close; one still present keeps its connections.
    pub fn replace(
        &self,
        bundles: impl IntoIterator<Item = TrustBundle>,
        valid_until: SystemTime,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            valid_until > SystemTime::now(),
            "trust bundles must be good until a time still to come"
        );
        let _write = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let old = self.held.load_full();
        let mut next = HashMap::new();
        for bundle in bundles {
            anyhow::ensure!(
                !bundle.is_empty(),
                "the trust bundle for {} has no keys",
                bundle.trust_domain
            );
            let domain = bundle.trust_domain.as_str().to_string();
            let dropped = old
                .get(&domain)
                .map(|held| held.dropped.clone())
                .unwrap_or_default();
            let held = Arc::new(Held {
                bundle: Arc::new(bundle),
                valid_until,
                dropped,
            });
            anyhow::ensure!(
                next.insert(domain.clone(), held).is_none(),
                "two trust bundles for {domain}"
            );
        }
        let next = Arc::new(next);
        self.held.store(Arc::clone(&next));
        for (domain, held) in old.iter() {
            if !next.contains_key(domain) {
                held.dropped.cancel();
            }
        }
        Ok(())
    }

    /// Add or refresh one trust domain's bundle, leaving the others.
    pub fn insert(&self, bundle: TrustBundle, valid_until: SystemTime) -> anyhow::Result<()> {
        anyhow::ensure!(
            valid_until > SystemTime::now(),
            "a trust bundle must be good until a time still to come"
        );
        anyhow::ensure!(
            !bundle.is_empty(),
            "the trust bundle for {} has no keys",
            bundle.trust_domain
        );
        let _write = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = (**self.held.load()).clone();
        let domain = bundle.trust_domain.as_str().to_string();
        let dropped = next
            .get(&domain)
            .map(|held| held.dropped.clone())
            .unwrap_or_default();
        next.insert(
            domain,
            Arc::new(Held {
                bundle: Arc::new(bundle),
                valid_until,
                dropped,
            }),
        );
        self.held.store(Arc::new(next));
        Ok(())
    }

    /// Stop trusting `trust_domain`, and tell its connections to close.
    pub fn remove(&self, trust_domain: &str) {
        let _write = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = (**self.held.load()).clone();
        if let Some(held) = next.remove(trust_domain) {
            self.held.store(Arc::new(next));
            held.dropped.cancel();
        }
    }

    /// Drop every bundle.
    pub fn clear(&self) {
        let _write = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let old = self.held.swap(Arc::default());
        for held in old.values() {
            held.dropped.cancel();
        }
    }

    fn current(&self, trust_domain: &str) -> Option<Arc<Held>> {
        self.held
            .load()
            .get(trust_domain)
            .filter(|held| held.valid_until > SystemTime::now())
            .cloned()
    }

    /// `trust_domain`'s bundle, unless it is missing or past its time.
    pub fn get(&self, trust_domain: &str) -> Option<Arc<TrustBundle>> {
        self.current(trust_domain)
            .map(|held| Arc::clone(&held.bundle))
    }

    /// The trust domains with a current bundle.
    pub fn trust_domains(&self) -> Vec<String> {
        let now = SystemTime::now();
        let mut domains: Vec<_> = self
            .held
            .load()
            .iter()
            .filter(|(_, held)| held.valid_until > now)
            .map(|(domain, _)| domain.clone())
            .collect();
        domains.sort();
        domains
    }

    /// Resolves once `trust_domain` is removed. `None` if it has no current
    /// bundle now.
    pub fn dropped(&self, trust_domain: &str) -> Option<WaitForCancellationFutureOwned> {
        self.current(trust_domain)
            .map(|held| held.dropped.clone().cancelled_owned())
    }

    /// The SPIFFE ID of a chain whose leaf is an X.509-SVID issued in its own
    /// trust domain, verified against that domain's current bundle.
    pub fn verify_x509_svid(
        &self,
        chain: &[CertificateDer<'_>],
        usage: SvidUsage,
        now: UnixTime,
    ) -> anyhow::Result<SpiffeId> {
        let (leaf, intermediates) = chain
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("the peer presented no certificate"))?;
        let id = svid_id(leaf)?;
        let held = self
            .current(id.trust_domain())
            .ok_or_else(|| anyhow::anyhow!("no current trust bundle for {}", id.trust_domain()))?;
        let cert = webpki::EndEntityCert::try_from(leaf)
            .map_err(|e| anyhow::anyhow!("the peer's X.509-SVID: {e}"))?;
        let eku = match usage {
            SvidUsage::Server => webpki::KeyUsage::server_auth(),
            SvidUsage::Client => webpki::KeyUsage::client_auth(),
        };
        cert.verify_for_usage(
            webpki::ALL_VERIFICATION_ALGS,
            &held.bundle.anchors,
            intermediates,
            now,
            eku,
            None,
            None,
        )
        .map_err(|e| {
            anyhow::anyhow!(
                "{id} does not chain to the trust bundle for {}: {e}",
                id.trust_domain()
            )
        })?;
        Ok(id)
    }

    /// Verify a compact JWT-SVID for `audience` against the current bundle of
    /// the trust domain its `sub` names.
    pub fn verify_jwt_svid(&self, token: &str, audience: &str) -> anyhow::Result<JwtSvidClaims> {
        #[derive(serde::Deserialize)]
        struct Header {
            alg: String,
            kid: Option<String>,
            typ: Option<String>,
            crit: Option<serde_json::Value>,
        }
        #[derive(serde::Deserialize)]
        struct Claims {
            sub: String,
            aud: Audience,
            exp: u64,
            nbf: Option<u64>,
        }
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Audience {
            One(String),
            Many(Vec<String>),
        }
        anyhow::ensure!(
            token.len() <= MAX_JWT,
            "the JWT-SVID is over {MAX_JWT} bytes"
        );
        let mut parts = token.split('.');
        let (Some(header), Some(payload), Some(signature), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            anyhow::bail!("the JWT-SVID is not a compact JWS");
        };
        let decode = |part: &str| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(part)
                .map_err(|_| anyhow::anyhow!("the JWT-SVID is not base64url"))
        };
        let head: Header = serde_json::from_slice(&decode(header)?)?;
        anyhow::ensure!(head.crit.is_none(), "the JWT-SVID has critical headers");
        anyhow::ensure!(
            head.typ
                .as_deref()
                .is_none_or(|typ| typ == "JWT" || typ == "JOSE"),
            "the JWT-SVID's typ is not JWT or JOSE"
        );
        let alg = JwtAlg::parse(&head.alg)
            .ok_or_else(|| anyhow::anyhow!("alg {:?} is not allowed for a JWT-SVID", head.alg))?;
        let kid = head
            .kid
            .ok_or_else(|| anyhow::anyhow!("the JWT-SVID names no kid"))?;
        let claims: Claims = serde_json::from_slice(&decode(payload)?)?;
        let id = SpiffeId::parse(&claims.sub)?;
        anyhow::ensure!(!id.path().is_empty(), "the JWT-SVID's sub {id} has no path");
        let held = self
            .current(id.trust_domain())
            .ok_or_else(|| anyhow::anyhow!("no current trust bundle for {}", id.trust_domain()))?;
        let key = held.bundle.jwt_keys.get(&kid).ok_or_else(|| {
            anyhow::anyhow!("{}'s bundle has no jwt-svid key {kid:?}", id.trust_domain())
        })?;
        // The claims are checked first, so a token for another audience or
        // out of date costs no signature check; it is accepted only once the
        // signature verifies.
        let audiences = match claims.aud {
            Audience::One(aud) => vec![aud],
            Audience::Many(auds) => auds,
        };
        anyhow::ensure!(
            audiences.iter().any(|aud| aud == audience),
            "the JWT-SVID for {id} is not for {audience}"
        );
        let now = SystemTime::now();
        let at = |secs: u64| {
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_secs(secs))
                .ok_or_else(|| anyhow::anyhow!("the JWT-SVID for {id} has a time out of range"))
        };
        let expires_at = at(claims.exp)?;
        anyhow::ensure!(expires_at > now, "the JWT-SVID for {id} has expired");
        if let Some(nbf) = claims.nbf {
            anyhow::ensure!(
                at(nbf)? <= now + NBF_SKEW,
                "the JWT-SVID for {id} is not valid yet"
            );
        }
        let signed = &token[..header.len() + 1 + payload.len()];
        key.verify(alg, signed.as_bytes(), &decode(signature)?)
            .map_err(|e| anyhow::anyhow!("the JWT-SVID for {id}: {e}"))?;
        Ok(JwtSvidClaims {
            spiffe_id: id,
            audience: audiences,
            expires_at,
        })
    }

    /// Verifies a TLS server as `peer`, by its X.509-SVID. The server's DNS
    /// name is not checked: a SPIFFE peer is authenticated by its ID.
    pub fn server_verifier(&self, peer: SpiffePeer) -> Arc<SpiffeServerVerifier> {
        Arc::new(SpiffeServerVerifier {
            bundles: self.clone(),
            peer,
            algorithms: algorithms(),
        })
    }

    /// Asks every TLS client for a certificate, admits one that presents
    /// none, and refuses one whose certificate is not an X.509-SVID that
    /// verifies against a current bundle. Asks only while a bundle is
    /// current.
    pub fn client_verifier(&self) -> Arc<SpiffeClientVerifier> {
        Arc::new(SpiffeClientVerifier {
            bundles: self.clone(),
            algorithms: algorithms(),
        })
    }
}

fn algorithms() -> WebPkiSupportedAlgorithms {
    rustls::crypto::CryptoProvider::get_default()
        .map(|provider| provider.signature_verification_algorithms)
        .unwrap_or_else(|| {
            rustls::crypto::aws_lc_rs::default_provider().signature_verification_algorithms
        })
}

/// The SPIFFE ID of an X.509-SVID leaf: exactly one URI SAN, a spiffe:// one
/// with a path, on a certificate that is not a CA.
pub fn svid_id(leaf: &CertificateDer<'_>) -> anyhow::Result<SpiffeId> {
    let (_, cert) = x509_parser::parse_x509_certificate(leaf)
        .map_err(|e| anyhow::anyhow!("the peer's certificate does not parse: {e}"))?;
    anyhow::ensure!(!cert.is_ca(), "the peer's leaf certificate is a CA");
    let uris: Vec<&str> = cert
        .subject_alternative_name()
        .map_err(|e| anyhow::anyhow!("the peer's certificate SAN: {e}"))?
        .map(|san| {
            san.value
                .general_names
                .iter()
                .filter_map(|name| match name {
                    x509_parser::extensions::GeneralName::URI(uri) => Some(*uri),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let [uri] = uris.as_slice() else {
        anyhow::bail!("an X.509-SVID carries exactly one URI SAN");
    };
    let id = SpiffeId::parse(uri)?;
    anyhow::ensure!(
        !id.path().is_empty(),
        "the X.509-SVID's ID {id} has no path"
    );
    Ok(id)
}

fn refuse(err: anyhow::Error) -> rustls::Error {
    let err: Box<dyn std::error::Error + Send + Sync> = err.into();
    rustls::Error::InvalidCertificate(CertificateError::Other(OtherError(Arc::from(err))))
}

/// See [`TrustBundles::server_verifier`].
#[derive(Debug)]
pub struct SpiffeServerVerifier {
    bundles: TrustBundles,
    peer: SpiffePeer,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for SpiffeServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let chain: Vec<_> = std::iter::once(end_entity.clone())
            .chain(intermediates.iter().cloned())
            .collect();
        let id = self
            .bundles
            .verify_x509_svid(&chain, SvidUsage::Server, now)
            .map_err(refuse)?;
        if !self.peer.admits(&id) {
            return Err(refuse(anyhow::anyhow!(
                "the server is {id}, not {}",
                self.peer
            )));
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// See [`TrustBundles::client_verifier`].
#[derive(Debug)]
pub struct SpiffeClientVerifier {
    bundles: TrustBundles,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ClientCertVerifier for SpiffeClientVerifier {
    fn offer_client_auth(&self) -> bool {
        !self.bundles.trust_domains().is_empty()
    }

    fn client_auth_mandatory(&self) -> bool {
        false
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        let chain: Vec<_> = std::iter::once(end_entity.clone())
            .chain(intermediates.iter().cloned())
            .collect();
        self.bundles
            .verify_x509_svid(&chain, SvidUsage::Client, now)
            .map_err(refuse)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests;
