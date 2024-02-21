//! Claims encoding, decoding, and validation for JSON Web Tokens (JWT)

use crate::{errors, errors::ErrorKind, jwt, Result};

use data_encoding::BASE64URL_NOPAD;
use nkeys::KeyPair;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{from_str, to_string};
use std::{
    collections::HashMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
const HEADER_TYPE: &str = "jwt";
const HEADER_ALGORITHM: &str = "Ed25519";

// Current internal revision number that will go into embedded claims
pub(crate) const WASCAP_INTERNAL_REVISION: u32 = 3;

// Minimum revision number at which we verify module hashes
pub(crate) const MIN_WASCAP_INTERNAL_REVISION: u32 = 3;

/// A structure containing a JWT and its associated decoded claims
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct Token<T> {
    pub jwt: String,
    pub claims: Claims<T>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ClaimsHeader {
    #[serde(rename = "typ")]
    header_type: String,

    #[serde(rename = "alg")]
    algorithm: String,
}

fn default_as_false() -> bool {
    false
}

pub trait WascapEntity: Clone {
    fn name(&self) -> String;
}

/// The metadata that corresponds to an actor module
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Default)]
pub struct Actor {
    /// A descriptive name for this actor, should not include version information or public key
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// A hash of the module's bytes as they exist without the embedded signature. This is stored so wascap
    /// can determine if a WebAssembly module's bytecode has been altered after it was signed
    #[serde(rename = "hash")]
    pub module_hash: String,

    /// List of arbitrary string tags associated with the claims
    #[serde(rename = "tags", skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,

    /// List of capability attestations. Can be standard wascap capabilities or custom namespace capabilities
    #[serde(rename = "caps", skip_serializing_if = "Option::is_none")]
    pub caps: Option<Vec<String>>,

    /// Indicates a monotonically increasing revision number.  Optional.
    #[serde(rename = "rev", skip_serializing_if = "Option::is_none")]
    pub rev: Option<i32>,

    /// Indicates a human-friendly version string
    #[serde(rename = "ver", skip_serializing_if = "Option::is_none")]
    pub ver: Option<String>,

    /// An optional, code-friendly alias that can be used instead of a public key or
    /// OCI reference for invocations
    #[serde(rename = "call_alias", skip_serializing_if = "Option::is_none")]
    pub call_alias: Option<String>,

    /// Indicates whether this module is a capability provider
    #[serde(rename = "prov", default = "default_as_false")]
    pub provider: bool,
}

/// The claims metadata corresponding to a capability provider
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Default)]
pub struct CapabilityProvider {
    /// A descriptive name for the capability provider
    pub name: Option<String>,
    /// The capability contract ID this provider supports
    pub capid: String,
    /// A human-readable string identifying the vendor of this provider (e.g. Redis or Cassandra or NATS etc)
    pub vendor: String,
    /// Indicates a monotonically increasing revision number.  Optional.
    #[serde(rename = "rev", skip_serializing_if = "Option::is_none")]
    pub rev: Option<i32>,
    /// Indicates a human-friendly version string. Optional.
    #[serde(rename = "ver", skip_serializing_if = "Option::is_none")]
    pub ver: Option<String>,
    /// The file hashes that correspond to the achitecture-OS target triples for this provider.
    pub target_hashes: HashMap<String, String>,
    /// If the provider chooses, it can supply a JSON schma that describes its expected link configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_schema: Option<serde_json::Value>,
}

/// The claims metadata corresponding to an account
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Default)]
pub struct Account {
    /// A descriptive name for this account
    pub name: Option<String>,
    /// A list of valid public keys that may appear as an `issuer` on
    /// actors signed by one of this account's multiple seed keys
    pub valid_signers: Option<Vec<String>>,
}

/// The claims metadata corresponding to an operator
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Default)]
pub struct Operator {
    /// A descriptive name for the operator
    pub name: Option<String>,
    /// A list of valid public keys that may appear as an `issuer` on accounts
    /// signed by one of this operator's multiple seed keys
    pub valid_signers: Option<Vec<String>>,
}

/// The claims metadata corresponding to a cluster
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Default)]
pub struct Cluster {
    /// Optional friendly descriptive name for the cluster
    pub name: Option<String>,
    /// A list of valid public keys that may appear as an `issuer` on hosts
    /// or anything else signed by one of the cluster's seed keys
    pub valid_signers: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Default)]
pub struct Invocation {
    /// Fully qualified bus URL indicating the target of the invocation
    pub target_url: String,
    /// Fully qualified bus URL indicating the origin of the invocation
    pub origin_url: String,
    /// Hash of the invocation to which these claims belong
    #[serde(rename = "hash")]
    pub invocation_hash: String,
}

/// Represents a set of [RFC 7519](https://tools.ietf.org/html/rfc7519) compliant JSON Web Token
/// claims.
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Default)]
pub struct Claims<T> {
    /// All timestamps in JWTs are stored in _seconds since the epoch_ format
    /// as described as `NumericDate` in the RFC. Corresponds to the `exp` field in a JWT.
    #[serde(rename = "exp", skip_serializing_if = "Option::is_none")]
    pub expires: Option<u64>,

    /// Corresponds to the `jti` field in a JWT.
    #[serde(rename = "jti")]
    pub id: String,

    /// The `iat` field, stored in _seconds since the epoch_
    #[serde(rename = "iat")]
    pub issued_at: u64,

    /// Issuer of the token, by convention usually the public key of the _account_ that
    /// signed the token
    #[serde(rename = "iss")]
    pub issuer: String,

    /// Subject of the token, usually the public key of the _module_ corresponding to the WebAssembly file
    /// being signed
    #[serde(rename = "sub")]
    pub subject: String,

    /// The `nbf` JWT field, indicates the time when the token becomes valid. If `None` token is valid immediately
    #[serde(rename = "nbf", skip_serializing_if = "Option::is_none")]
    pub not_before: Option<u64>,

    /// Custom jwt claims in the `wascap` namespace
    #[serde(rename = "wascap", skip_serializing_if = "Option::is_none")]
    pub metadata: Option<T>,

    /// Internal revision number used to aid in parsing and validating claims
    #[serde(rename = "wascap_revision", skip_serializing_if = "Option::is_none")]
    pub(crate) wascap_revision: Option<u32>,
}

/// The result of the validation process perform on a JWT
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct TokenValidation {
    /// Indicates whether or not this token has expired, as determined by the current OS system clock.
    /// If `true`, you should treat the associated token as invalid
    pub expired: bool,
    /// Indicates whether this token is _not yet_ valid. If `true`, do not use this token
    pub cannot_use_yet: bool,
    /// A human-friendly (lowercase) description of the _relative_ expiration date (e.g. "in 3 hours").
    /// If the token never expires, the value will be "never"
    pub expires_human: String,
    /// A human-friendly description of the relative time when this token will become valid (e.g. "in 2 weeks").
    /// If the token has not had a "not before" date set, the value will be "immediately"
    pub not_before_human: String,
    /// Indicates whether the signature is valid according to a cryptographic comparison. If `false` you should
    /// reject this token.
    pub signature_valid: bool,
}

impl<T> Claims<T>
where
    T: Serialize + DeserializeOwned + WascapEntity,
{
    #[allow(clippy::missing_errors_doc)] // TODO: document
    pub fn encode(&self, kp: &KeyPair) -> Result<String> {
        let header = ClaimsHeader {
            header_type: HEADER_TYPE.to_string(),
            algorithm: HEADER_ALGORITHM.to_string(),
        };
        let header = to_jwt_segment(&header)?;
        let claims = to_jwt_segment(self)?;

        let head_and_claims = format!("{header}.{claims}");
        let sig = kp.sign(head_and_claims.as_bytes())?;
        let sig64 = BASE64URL_NOPAD.encode(&sig);
        Ok(format!("{head_and_claims}.{sig64}"))
    }

    #[allow(clippy::missing_errors_doc)] // TODO: document
    pub fn decode(input: &str) -> Result<Claims<T>> {
        let segments: Vec<&str> = input.split('.').collect();
        if segments.len() != 3 {
            return Err(errors::new(errors::ErrorKind::Token(
                "invalid token format".into(),
            )));
        }
        let claims: Claims<T> = from_jwt_segment(segments[1])?;

        Ok(claims)
    }

    pub fn name(&self) -> String {
        self.metadata
            .as_ref()
            .map_or("Anonymous".to_string(), jwt::WascapEntity::name)
    }
}

impl WascapEntity for Actor {
    fn name(&self) -> String {
        self.name
            .as_ref()
            .unwrap_or(&"Anonymous".to_string())
            .to_string()
    }
}

impl WascapEntity for CapabilityProvider {
    fn name(&self) -> String {
        self.name
            .as_ref()
            .unwrap_or(&"Unnamed Provider".to_string())
            .to_string()
    }
}

impl WascapEntity for Account {
    fn name(&self) -> String {
        self.name
            .as_ref()
            .unwrap_or(&"Anonymous".to_string())
            .to_string()
    }
}

impl WascapEntity for Operator {
    fn name(&self) -> String {
        self.name
            .as_ref()
            .unwrap_or(&"Anonymous".to_string())
            .to_string()
    }
}

impl WascapEntity for Cluster {
    fn name(&self) -> String {
        self.name
            .as_ref()
            .unwrap_or(&"Anonymous Cluster".to_string())
            .to_string()
    }
}
impl WascapEntity for Invocation {
    fn name(&self) -> String {
        self.target_url.to_string()
    }
}

impl Claims<Account> {
    /// Creates a new non-expiring Claims wrapper for metadata representing an account
    #[must_use]
    pub fn new(
        name: String,
        issuer: String,
        subject: String,
        additional_keys: Vec<String>,
    ) -> Claims<Account> {
        Self::with_dates(name, issuer, subject, None, None, additional_keys)
    }

    /// Creates a new Claims wrapper for metadata representing an account, with optional valid before and expiration dates
    #[must_use]
    pub fn with_dates(
        name: String,
        issuer: String,
        subject: String,
        not_before: Option<u64>,
        expires: Option<u64>,
        additional_keys: Vec<String>,
    ) -> Claims<Account> {
        Claims {
            metadata: Some(Account {
                name: Some(name),
                valid_signers: Some(additional_keys),
            }),
            expires,
            id: nuid::next(),
            issued_at: since_the_epoch().as_secs(),
            issuer,
            subject,
            not_before,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        }
    }
}

impl Claims<CapabilityProvider> {
    /// Creates a new non-expiring Claims wrapper for metadata representing a capability provider
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        name: String,
        issuer: String,
        subject: String,
        capid: String,
        vendor: String,
        rev: Option<i32>,
        ver: Option<String>,
        hashes: HashMap<String, String>,
    ) -> Claims<CapabilityProvider> {
        Self::with_dates(
            name, issuer, subject, capid, vendor, rev, ver, hashes, None, None,
        )
    }

    /// Creates a new Claims non-expiring wrapper for metadata representing a capability provider, with optional valid before and expiration dates
    #[must_use]
    pub fn with_provider(
        issuer: String,
        subject: String,
        not_before: Option<u64>,
        expires: Option<u64>,
        provider: CapabilityProvider,
    ) -> Claims<CapabilityProvider> {
        Claims {
            metadata: Some(provider),
            expires,
            id: nuid::next(),
            issued_at: since_the_epoch().as_secs(),
            issuer,
            subject,
            not_before,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        }
    }

    /// Creates a new Claims wrapper for metadata representing a capability provider, with optional valid before and expiration dates
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn with_dates(
        name: String,
        issuer: String,
        subject: String,
        capid: String,
        vendor: String,
        rev: Option<i32>,
        ver: Option<String>,
        hashes: HashMap<String, String>,
        not_before: Option<u64>,
        expires: Option<u64>,
    ) -> Claims<CapabilityProvider> {
        Claims {
            metadata: Some(CapabilityProvider {
                name: Some(name),
                capid,
                rev,
                ver,
                target_hashes: hashes,
                vendor,
                config_schema: None,
            }),
            expires,
            id: nuid::next(),
            issued_at: since_the_epoch().as_secs(),
            issuer,
            subject,
            not_before,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        }
    }
}

impl Claims<Operator> {
    /// Creates a new non-expiring Claims wrapper for metadata representing an operator
    #[must_use]
    pub fn new(
        name: String,
        issuer: String,
        subject: String,
        additional_keys: Vec<String>,
    ) -> Claims<Operator> {
        Self::with_dates(name, issuer, subject, None, None, additional_keys)
    }

    /// Creates a new Claims wrapper for metadata representing an operator, with optional valid before and expiration dates
    #[must_use]
    pub fn with_dates(
        name: String,
        issuer: String,
        subject: String,
        not_before: Option<u64>,
        expires: Option<u64>,
        additional_keys: Vec<String>,
    ) -> Claims<Operator> {
        Claims {
            metadata: Some(Operator {
                name: Some(name),
                valid_signers: Some(additional_keys),
            }),
            expires,
            id: nuid::next(),
            issued_at: since_the_epoch().as_secs(),
            issuer,
            subject,
            not_before,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        }
    }
}

impl Claims<Cluster> {
    /// Creates a new non-expiring Claims wrapper for metadata representing a cluster
    #[must_use]
    pub fn new(
        name: String,
        issuer: String,
        subject: String,
        additional_keys: Vec<String>,
    ) -> Claims<Cluster> {
        Self::with_dates(name, issuer, subject, None, None, additional_keys)
    }

    /// Creates a new Claims wrapper for metadata representing a cluster, with optional valid before and expiration dates
    #[must_use]
    pub fn with_dates(
        name: String,
        issuer: String,
        subject: String,
        not_before: Option<u64>,
        expires: Option<u64>,
        additional_keys: Vec<String>,
    ) -> Claims<Cluster> {
        Claims {
            metadata: Some(Cluster {
                name: Some(name),
                valid_signers: Some(additional_keys),
            }),
            expires,
            id: nuid::next(),
            issued_at: since_the_epoch().as_secs(),
            issuer,
            subject,
            not_before,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        }
    }
}

impl Claims<Actor> {
    /// Creates a new non-expiring Claims wrapper for metadata representing an actor
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        name: String,
        issuer: String,
        subject: String,
        caps: Option<Vec<String>>,
        tags: Option<Vec<String>>,
        provider: bool,
        rev: Option<i32>,
        ver: Option<String>,
        call_alias: Option<String>,
    ) -> Claims<Actor> {
        Self::with_dates(
            name, issuer, subject, caps, tags, None, None, provider, rev, ver, call_alias,
        )
    }

    /// Creates a new Claims wrapper for metadata representing an actor, with optional valid before and expiration dates
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn with_dates(
        name: String,
        issuer: String,
        subject: String,
        caps: Option<Vec<String>>,
        tags: Option<Vec<String>>,
        not_before: Option<u64>,
        expires: Option<u64>,
        provider: bool,
        rev: Option<i32>,
        ver: Option<String>,
        call_alias: Option<String>,
    ) -> Claims<Actor> {
        Claims {
            metadata: Some(Actor::new(name, caps, tags, provider, rev, ver, call_alias)),
            expires,
            id: nuid::next(),
            issued_at: since_the_epoch().as_secs(),
            issuer,
            subject,
            not_before,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        }
    }
}

impl Claims<Invocation> {
    /// Creates a new non-expiring Claims wrapper for metadata representing an invocation
    #[must_use]
    pub fn new(
        issuer: String,
        subject: String,
        target_url: &str,
        origin_url: &str,
        hash: &str,
    ) -> Claims<Invocation> {
        Self::with_dates(issuer, subject, None, None, target_url, origin_url, hash)
    }

    /// Creates a new Claims wrapper for metadata representing an invocation, with optional valid before and expiration dates
    #[must_use]
    pub fn with_dates(
        issuer: String,
        subject: String,
        not_before: Option<u64>,
        expires: Option<u64>,
        target_url: &str,
        origin_url: &str,
        hash: &str,
    ) -> Claims<Invocation> {
        Claims {
            metadata: Some(Invocation {
                target_url: target_url.to_string(),
                origin_url: origin_url.to_string(),
                invocation_hash: hash.to_string(),
            }),
            expires,
            id: nuid::next(),
            issued_at: since_the_epoch().as_secs(),
            issuer,
            subject,
            not_before,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        }
    }
}

#[derive(Default)]
pub struct ClaimsBuilder<T> {
    claims: Claims<T>,
}

impl<T> ClaimsBuilder<T>
where
    T: Default + WascapEntity,
{
    /// Creates a new builder
    #[must_use]
    pub fn new() -> Self {
        ClaimsBuilder::default()
    }

    /// Sets the issuer for the claims
    pub fn issuer(&mut self, issuer: &str) -> &mut Self {
        self.claims.issuer = issuer.to_string();
        self
    }

    /// Sets the subject for the claims
    pub fn subject(&mut self, module: &str) -> &mut Self {
        self.claims.subject = module.to_string();
        self
    }

    /// Indicates how long this claim set will remain valid
    pub fn expires_in(&mut self, d: Duration) -> &mut Self {
        self.claims.expires = Some(d.as_secs() + since_the_epoch().as_secs());
        self
    }

    /// Indicates how long until this claim set becomes valid
    pub fn valid_in(&mut self, d: Duration) -> &mut Self {
        self.claims.not_before = Some(d.as_secs() + since_the_epoch().as_secs());
        self
    }

    /// Sets the appropriate metadata for this claims type (e.g. `Actor`, `Operator`, `Invocation`, `CapabilityProvider` or `Account`)
    pub fn with_metadata(&mut self, metadata: T) -> &mut Self {
        self.claims.metadata = Some(metadata);
        self
    }

    // Produce a claims set from the builder
    pub fn build(&self) -> Claims<T> {
        Claims {
            id: nuid::next(),
            issued_at: since_the_epoch().as_secs(),
            ..self.claims.clone()
        }
    }
}

/// Validates a signed JWT. This will check the signature, expiration time, and not-valid-before time
#[allow(clippy::missing_errors_doc)] // TODO: document errors
pub fn validate_token<T>(input: &str) -> Result<TokenValidation>
where
    T: Serialize + DeserializeOwned + WascapEntity,
{
    let segments: Vec<&str> = input.split('.').collect();
    if segments.len() != 3 {
        return Err(crate::errors::new(ErrorKind::Token(format!(
            "invalid token format, expected 3 segments, found {}",
            segments.len()
        ))));
    }

    let header_and_claims = format!("{}.{}", segments[0], segments[1]);
    let sig = BASE64URL_NOPAD.decode(segments[2].as_bytes())?;

    let header: ClaimsHeader = from_jwt_segment(segments[0])?;
    validate_header(&header)?;

    let claims = Claims::<T>::decode(input)?;
    validate_issuer(&claims.issuer)?;
    validate_subject(&claims.subject)?;

    let kp = KeyPair::from_public_key(&claims.issuer)?;
    let sigverify = kp.verify(header_and_claims.as_bytes(), &sig);

    let validation = TokenValidation {
        signature_valid: sigverify.is_ok(),
        expired: validate_expiration(claims.expires).is_err(),
        expires_human: stamp_to_human(claims.expires).unwrap_or_else(|| "never".to_string()),
        not_before_human: stamp_to_human(claims.not_before)
            .unwrap_or_else(|| "immediately".to_string()),
        cannot_use_yet: validate_notbefore(claims.not_before).is_err(),
    };

    Ok(validation)
}

fn validate_notbefore(nb: Option<u64>) -> Result<()> {
    if let Some(nbf) = nb {
        let nbf_secs = Duration::from_secs(nbf);
        if since_the_epoch() < nbf_secs {
            Err(errors::new(ErrorKind::TokenTooEarly))
        } else {
            Ok(())
        }
    } else {
        Ok(())
    }
}

fn validate_expiration(exp: Option<u64>) -> Result<()> {
    if let Some(exp) = exp {
        let exp_secs = Duration::from_secs(exp);
        if exp_secs < since_the_epoch() {
            Err(errors::new(ErrorKind::ExpiredToken))
        } else {
            Ok(())
        }
    } else {
        Ok(())
    }
}

fn validate_issuer(iss: &str) -> Result<()> {
    if iss.is_empty() {
        Err(errors::new(ErrorKind::MissingIssuer))
    } else {
        Ok(())
    }
}

fn validate_subject(sub: &str) -> Result<()> {
    if sub.is_empty() {
        Err(errors::new(ErrorKind::MissingSubject))
    } else {
        Ok(())
    }
}

fn since_the_epoch() -> Duration {
    let start = SystemTime::now();
    start
        .duration_since(UNIX_EPOCH)
        .expect("A timey wimey problem has occurred!")
}

fn validate_header(h: &ClaimsHeader) -> Result<()> {
    if h.algorithm != HEADER_ALGORITHM {
        Err(errors::new(ErrorKind::InvalidAlgorithm))
    } else if h.header_type != HEADER_TYPE {
        Err(errors::new(ErrorKind::Token("Invalid header".to_string())))
    } else {
        Ok(())
    }
}

fn to_jwt_segment<T: Serialize>(input: &T) -> Result<String> {
    let encoded = to_string(input)?;
    Ok(BASE64URL_NOPAD.encode(encoded.as_bytes()))
}

fn from_jwt_segment<B: AsRef<str>, T: DeserializeOwned>(encoded: B) -> Result<T> {
    let decoded = BASE64URL_NOPAD.decode(encoded.as_ref().as_bytes())?;
    let s = String::from_utf8(decoded)?;

    Ok(from_str(&s)?)
}

fn stamp_to_human(stamp: Option<u64>) -> Option<String> {
    stamp.and_then(|s| {
        let now: i64 = since_the_epoch().as_secs().try_into().ok()?;
        let s: i64 = s.try_into().ok()?;
        let diff_sec = (now - s).abs();

        // calculate roundoff
        let diff_sec = if diff_sec >= 86400 {
            // round to days
            diff_sec - (diff_sec % 86400)
        } else if diff_sec >= 3600 {
            // round to hours
            diff_sec - (diff_sec % 3600)
        } else if diff_sec >= 60 {
            // round to minutes
            diff_sec - (diff_sec % 60)
        } else {
            diff_sec
        };
        let diff_sec = diff_sec.try_into().ok()?;
        let ht = humantime::format_duration(Duration::from_secs(diff_sec));

        let now: u64 = now.try_into().ok()?;
        let s: u64 = s.try_into().ok()?;
        if now > s {
            Some(format!("{ht} ago"))
        } else {
            Some(format!("in {ht}"))
        }
    })
}

fn normalize_call_alias(alias: Option<String>) -> Option<String> {
    alias.map(|a| {
        let mut n = a.to_lowercase();
        n = n.trim().to_string();
        n = n.replace(|c: char| !c.is_ascii(), "");
        n = n.replace(' ', "_");
        n = n.replace('-', "_");
        n = n.replace('.', "_");
        n
    })
}

impl Actor {
    #[must_use]
    pub fn new(
        name: String,
        caps: Option<Vec<String>>,
        tags: Option<Vec<String>>,
        provider: bool,
        rev: Option<i32>,
        ver: Option<String>,
        call_alias: Option<String>,
    ) -> Actor {
        Actor {
            name: Some(name),
            module_hash: String::new(),
            tags,
            caps,
            provider,
            rev,
            ver,
            call_alias: normalize_call_alias(call_alias),
        }
    }
}

impl CapabilityProvider {
    #[must_use]
    pub fn new(
        name: String,
        capid: String,
        vendor: String,
        rev: Option<i32>,
        ver: Option<String>,
        hashes: HashMap<String, String>,
    ) -> CapabilityProvider {
        CapabilityProvider {
            target_hashes: hashes,
            name: Some(name),
            capid,
            vendor,
            rev,
            ver,
            config_schema: None,
        }
    }
}

impl Account {
    #[must_use]
    pub fn new(name: String, additional_keys: Vec<String>) -> Account {
        Account {
            name: Some(name),
            valid_signers: Some(additional_keys),
        }
    }
}

impl Operator {
    #[must_use]
    pub fn new(name: String, additional_keys: Vec<String>) -> Operator {
        Operator {
            name: Some(name),
            valid_signers: Some(additional_keys),
        }
    }
}

impl Cluster {
    #[must_use]
    pub fn new(name: String, additional_keys: Vec<String>) -> Cluster {
        Cluster {
            name: Some(name),
            valid_signers: Some(additional_keys),
        }
    }
}

impl Invocation {
    #[must_use]
    pub fn new(target_url: &str, origin_url: &str, hash: &str) -> Invocation {
        Invocation {
            target_url: target_url.to_string(),
            origin_url: origin_url.to_string(),
            invocation_hash: hash.to_string(),
        }
    }
}

#[cfg(test)]
mod test {
    use super::{Account, Actor, Claims, ErrorKind, KeyPair, Operator};
    use crate::{
        caps::{KEY_VALUE, LOGGING, MESSAGING},
        jwt::{
            since_the_epoch, validate_token, CapabilityProvider, ClaimsBuilder, Cluster,
            WASCAP_INTERNAL_REVISION,
        },
    };
    use std::collections::HashMap;
    use std::io::Read;

    #[test]
    fn full_validation_nbf() {
        let kp = KeyPair::new_account();
        let claims = Claims {
            metadata: Some(Actor::new(
                "test".to_string(),
                Some(vec![MESSAGING.to_string(), KEY_VALUE.to_string()]),
                Some(vec![]),
                false,
                Some(0),
                Some(String::new()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: Some(since_the_epoch().as_secs() + 1000),
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        };

        let encoded = claims.encode(&kp).unwrap();
        let vres = validate_token::<Actor>(&encoded);
        assert!(vres.is_ok());
        if let Ok(v) = vres {
            assert!(!v.expired);
            assert!(v.cannot_use_yet);
            assert_eq!(v.not_before_human, "in 16m");
        }
    }

    #[test]
    fn full_validation_expires() {
        let kp = KeyPair::new_account();
        let claims = Claims {
            metadata: Some(Actor::new(
                "test".to_string(),
                Some(vec![MESSAGING.to_string(), KEY_VALUE.to_string()]),
                Some(vec![]),
                false,
                Some(1),
                Some(String::new()),
                None,
            )),
            expires: Some(since_the_epoch().as_secs() - 30000),
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        };

        let encoded = claims.encode(&kp).unwrap();
        let vres = validate_token::<Actor>(&encoded);
        assert!(vres.is_ok());
        if let Ok(v) = vres {
            assert!(v.expired);
            assert!(!v.cannot_use_yet);
            assert_eq!(v.expires_human, "8h ago");
        }
    }

    #[test]
    fn validate_account() {
        let issuer = KeyPair::new_operator();
        let claims = Claims {
            metadata: Some(Account::new("test account".to_string(), vec![])),
            expires: Some(since_the_epoch().as_secs() - 30000),
            id: nuid::next(),
            issued_at: 0,
            issuer: issuer.public_key(),
            subject: "foo".to_string(),
            not_before: None,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        };
        let encoded = claims.encode(&issuer).unwrap();
        let vres = validate_token::<Account>(&encoded);
        assert!(vres.is_ok());
        if let Ok(v) = vres {
            assert!(v.expired);
            assert!(!v.cannot_use_yet);
            assert_eq!(v.expires_human, "8h ago");
        }
    }

    #[test]
    fn full_validation() {
        let kp = KeyPair::new_account();
        let claims = Claims {
            metadata: Some(Actor::new(
                "test".to_string(),
                Some(vec![MESSAGING.to_string(), KEY_VALUE.to_string()]),
                Some(vec![]),
                false,
                Some(1),
                Some(String::new()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        };

        let encoded = claims.encode(&kp).unwrap();
        let vres = validate_token::<Actor>(&encoded);
        assert!(vres.is_ok());
    }

    #[test]
    fn encode_decode_mismatch() {
        let issuer = KeyPair::new_operator();
        let claims = Claims {
            metadata: Some(Account::new("test account".to_string(), vec![])),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: "foo".to_string(),
            subject: "test".to_string(),
            not_before: None,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        };
        let encoded = claims.encode(&issuer).unwrap();
        let decoded = Claims::<Actor>::decode(&encoded);
        assert!(decoded.is_err());
    }

    #[test]
    fn decode_actor_as_operator() {
        let kp = KeyPair::new_account();
        let claims = Claims {
            metadata: Some(Actor::new(
                "test".to_string(),
                Some(vec![MESSAGING.to_string(), KEY_VALUE.to_string()]),
                Some(vec![]),
                false,
                Some(1),
                Some(String::new()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        };
        let encoded = claims.encode(&kp).unwrap();
        let decoded = Claims::<Operator>::decode(&encoded);
        assert!(decoded.is_ok());
        assert_eq!(decoded.unwrap().metadata.unwrap().name.unwrap(), "test");
    }

    #[test]
    fn encode_decode_roundtrip() {
        let kp = KeyPair::new_account();
        let claims = Claims {
            metadata: Some(Actor::new(
                "test".to_string(),
                Some(vec![MESSAGING.to_string(), KEY_VALUE.to_string()]),
                Some(vec![]),
                false,
                Some(1),
                Some(String::new()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        };

        let encoded = claims.encode(&kp).unwrap();

        let decoded = Claims::decode(&encoded).unwrap();
        assert!(validate_token::<Actor>(&encoded).is_ok());

        assert_eq!(claims, decoded);
    }

    #[test]
    fn provider_round_trip() {
        let account = KeyPair::new_account();
        let provider = KeyPair::new_service();

        let mut hashes = HashMap::new();
        hashes.insert("aarch64-linux".to_string(), "abc12345".to_string());
        let claims = ClaimsBuilder::new()
            .subject(&provider.public_key())
            .issuer(&account.public_key())
            .with_metadata(CapabilityProvider::new(
                "Test Provider".to_string(),
                "wasmcloud:testing".to_string(),
                "wasmCloud Internal".to_string(),
                Some(1),
                Some("v0.0.1".to_string()),
                hashes,
            ))
            .build();

        let encoded = claims.encode(&account).unwrap();
        let decoded: Claims<CapabilityProvider> = Claims::decode(&encoded).unwrap();
        assert!(validate_token::<CapabilityProvider>(&encoded).is_ok());
        assert_eq!(decoded.issuer, account.public_key());
        assert_eq!(decoded.subject, provider.public_key());
        assert_eq!(
            decoded.metadata.as_ref().unwrap().vendor,
            "wasmCloud Internal"
        );
        assert_eq!(
            decoded.metadata.as_ref().unwrap().capid,
            "wasmcloud:testing"
        );
    }

    #[test]
    fn provider_round_trip_with_schema() {
        let raw_schema = r#"
        {
            "$id": "https://wasmcloud.com/httpserver.schema.json",
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "HTTP server provider schema",
            "type": "object",
            "properties": {
              "port": {
                "type": "integer",
                "description": "The port number to use for the web server",
                "minimum": 4000,
                "maximum": 10000
              },
              "lastName": {
                "type": "string",
                "description": "Someone's last name."
              },
              "easterEgg": {
                "description": "Indicates whether or not the easter egg should be displayed",
                "type": "boolean"                
              }
            }
          }
        "#;
        let account = KeyPair::new_account();
        let provider = KeyPair::new_service();

        let mut hashes = HashMap::new();
        hashes.insert("aarch64-linux".to_string(), "abc12345".to_string());

        let schema = serde_json::from_str::<serde_json::Value>(raw_schema).unwrap();
        let mut hashes = HashMap::new();
        hashes.insert("aarch64-linux".to_string(), "abc12345".to_string());
        let claims = ClaimsBuilder::new()
            .subject(&provider.public_key())
            .issuer(&account.public_key())
            .with_metadata(CapabilityProvider {
                name: Some("Test Provider".to_string()),
                capid: "wasmcloud:testing".to_string(),
                vendor: "wasmCloud Internal".to_string(),
                rev: Some(1),
                ver: Some("v0.0.1".to_string()),
                target_hashes: hashes,
                config_schema: Some(schema),
            })
            .build();

        let encoded = claims.encode(&account).unwrap();
        let decoded: Claims<CapabilityProvider> = Claims::decode(&encoded).unwrap();
        assert!(validate_token::<CapabilityProvider>(&encoded).is_ok());
        assert_eq!(decoded.issuer, account.public_key());
        assert_eq!(decoded.subject, provider.public_key());
        assert_eq!(
            decoded
                .metadata
                .as_ref()
                .unwrap()
                .config_schema
                .as_ref()
                .unwrap()["properties"]["port"]["minimum"],
            4000
        );
    }

    #[test]
    fn encode_decode_logging_roundtrip() {
        let kp = KeyPair::new_account();
        let claims = Claims {
            metadata: Some(Actor::new(
                "test".to_string(),
                Some(vec![MESSAGING.to_string(), LOGGING.to_string()]),
                Some(vec![]),
                false,
                Some(1),
                Some(String::new()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        };

        let encoded = claims.encode(&kp).unwrap();

        let decoded = Claims::decode(&encoded).unwrap();
        assert!(validate_token::<Actor>(&encoded).is_ok());

        assert_eq!(claims, decoded);
    }
    #[test]
    fn account_extra_signers() {
        let op = KeyPair::new_operator();
        let kp1 = KeyPair::new_account();
        let kp2 = KeyPair::new_account();
        let claims = Claims::<Account>::new(
            "test account".to_string(),
            op.public_key(),
            kp1.public_key(),
            vec![kp2.public_key()],
        );
        let encoded = claims.encode(&kp1).unwrap();
        let decoded = Claims::<Account>::decode(&encoded).unwrap();
        assert!(validate_token::<Account>(&encoded).is_ok());
        assert_eq!(claims, decoded);
        assert_eq!(claims.metadata.unwrap().valid_signers.unwrap().len(), 1);
    }

    #[test]
    fn cluster_extra_signers() {
        let op = KeyPair::new_operator();
        let kp1 = KeyPair::new_cluster();
        let kp2 = KeyPair::new_cluster();
        let claims = Claims::<Cluster>::new(
            "test cluster".to_string(),
            op.public_key(),
            kp1.public_key(),
            vec![kp2.public_key()],
        );
        let encoded = claims.encode(&kp1).unwrap();
        let decoded = Claims::<Cluster>::decode(&encoded).unwrap();
        assert!(validate_token::<Cluster>(&encoded).is_ok());
        assert_eq!(claims, decoded);
        assert_eq!(claims.metadata.unwrap().valid_signers.unwrap().len(), 1);
    }

    #[test]
    fn encode_decode_bad_token() {
        let kp = KeyPair::new_account();
        let claims = Claims {
            metadata: Some(Actor::new(
                "test".to_string(),
                Some(vec![MESSAGING.to_string(), KEY_VALUE.to_string()]),
                Some(vec![]),
                false,
                Some(1),
                Some(String::new()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        };

        let encoded_nosep = claims.encode(&kp).unwrap().replace('.', "");

        let decoded = Claims::<Account>::decode(&encoded_nosep);
        assert!(decoded.is_err());
        if let Err(e) = decoded {
            match e.kind() {
                ErrorKind::Token(s) => assert_eq!(s, "invalid token format"),
                _ => {
                    panic!("failed to assert errors::ErrorKind::Token");
                }
            }
        }
    }

    #[test]
    fn ensure_issuer_on_token() {
        let kp = KeyPair::new_account();
        let mut claims = Claims {
            metadata: Some(Actor::new(
                "test".to_string(),
                Some(vec![MESSAGING.to_string(), KEY_VALUE.to_string()]),
                Some(vec![]),
                false,
                Some(1),
                Some(String::new()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        };

        let encoded = claims.encode(&kp).unwrap();
        assert!(validate_token::<Account>(&encoded).is_ok());

        // Set the issuer to empty
        claims.issuer = String::new();
        let bad_encode = claims.encode(&kp).unwrap();
        let issuer_err = validate_token::<Account>(&bad_encode);
        assert!(issuer_err.is_err());
        if let Err(e) = issuer_err {
            match e.kind() {
                ErrorKind::MissingIssuer => (),
                _ => panic!("failed to assert errors::ErrorKind::MissingIssuer"),
            }
        }
    }

    #[test]
    fn ensure_subject_on_token() {
        let kp = KeyPair::new_account();
        let mut claims = Claims {
            metadata: Some(Actor::new(
                "test".to_string(),
                Some(vec![MESSAGING.to_string(), KEY_VALUE.to_string()]),
                Some(vec![]),
                false,
                Some(1),
                Some(String::new()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
            wascap_revision: Some(WASCAP_INTERNAL_REVISION),
        };

        claims.subject = String::new();
        let bad_subject = claims.encode(&kp).unwrap();
        let subject_err = validate_token::<Account>(&bad_subject);
        assert!(subject_err.is_err());
        assert!(subject_err.is_err());
        if let Err(e) = subject_err {
            match e.kind() {
                ErrorKind::MissingSubject => (),
                _ => panic!("failed to assert errors::ErrorKind::MissingSubject"),
            }
        }
    }

    #[test]
    fn ensure_backwards_compatible() {
        let mut echo_messaging_v9 = vec![];
        let mut file = std::fs::File::open("./fixtures/echo_messaging_v0.9.0.wasm").unwrap();
        file.read_to_end(&mut echo_messaging_v9).unwrap();

        let extracted = crate::wasm::extract_claims(&echo_messaging_v9)
            .unwrap()
            .unwrap();

        let vres = validate_token::<Actor>(&extracted.jwt);
        assert!(vres.is_ok());
    }

    #[test]
    fn ensure_jwt_valid_segments() {
        let valid = "eyJ0eXAiOiJqd3QiLCJhbGciOiJFZDI1NTE5In0.eyJqdGkiOiJTakI1Zm05NzRTanU5V01nVFVjaHNiIiwiaWF0IjoxNjQ0ODQzNzQzLCJpc3MiOiJBQ09KSk42V1VQNE9ERDc1WEVCS0tUQ0NVSkpDWTVaS1E1NlhWS1lLNEJFSldHVkFPT1FIWk1DVyIsInN1YiI6Ik1CQ0ZPUE02SlcyQVBKTFhKRDNaNU80Q043Q1BZSjJCNEZUS0xKVVI1WVI1TUlUSVU3SEQzV0Q1Iiwid2FzY2FwIjp7Im5hbWUiOiJFY2hvIiwiaGFzaCI6IjRDRUM2NzNBN0RDQ0VBNkE0MTY1QkIxOTU4MzJDNzkzNjQ3MUNGN0FCNDUwMUY4MzdGOEQ2NzlGNDQwMEJDOTciLCJ0YWdzIjpbXSwiY2FwcyI6WyJ3YXNtY2xvdWQ6aHR0cHNlcnZlciJdLCJyZXYiOjQsInZlciI6IjAuMy40IiwicHJvdiI6ZmFsc2V9fQ.ZWyD6VQqzaYM1beD2x9Fdw4o_Bavy3ZG703Eg4cjhyJwUKLDUiVPVhqHFE6IXdV4cW6j93YbMT6VGq5iBDWmAg";
        let too_few = "asd.123123123";
        let too_many = "asd.123.abc.easy";
        let correct_but_wrong = "ddd.123.notajwt";

        assert!(validate_token::<Actor>(valid).is_ok());
        assert!(validate_token::<Actor>(too_few)
            .is_err_and(|e| e.to_string()
                == "JWT error: invalid token format, expected 3 segments, found 2"));
        assert!(validate_token::<Actor>(too_many)
            .is_err_and(|e| e.to_string()
                == "JWT error: invalid token format, expected 3 segments, found 4"));
        // Should be an error, but not because of the segment validation
        assert!(validate_token::<Actor>(correct_but_wrong).is_err_and(|e| !e
            .to_string()
            .contains("invalid token format, expected 3 segments")));
    }
}
