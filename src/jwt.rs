//! Claims encoding, decoding, and validation for JSON Web Tokens (JWT)

use crate::errors;
use crate::errors::ErrorKind;
use crate::Result;
use chrono::NaiveDateTime;
use nkeys::KeyPair;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{from_str, to_string};
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const HEADER_TYPE: &str = "jwt";
const HEADER_ALGORITHM: &str = "Ed25519";

/// A structure containing a JWT and its associated decoded claims
#[derive(Debug, Serialize, Deserialize, PartialEq)]
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
    pub fn encode(&self, kp: &KeyPair) -> Result<String> {
        let header = ClaimsHeader {
            header_type: HEADER_TYPE.to_string(),
            algorithm: HEADER_ALGORITHM.to_string(),
        };
        let jheader = to_jwt_segment(&header)?;
        let jclaims = to_jwt_segment(self)?;

        let head_and_claims = format!("{}.{}", jheader, jclaims);
        let sig = kp.sign(head_and_claims.as_bytes())?;
        let sig64 = base64::encode_config(&sig, base64::URL_SAFE_NO_PAD);
        Ok(format!("{}.{}", head_and_claims, sig64))
    }

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
            .map_or("Anonymous".to_string(), |md| md.name())
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

impl WascapEntity for Invocation {
    fn name(&self) -> String {
        self.target_url.to_string()
    }
}

impl Claims<Account> {
    pub fn new(
        name: String,
        issuer: String,
        subject: String,
        additional_keys: Vec<String>,
    ) -> Claims<Account> {
        Self::with_dates(name, issuer, subject, None, None, additional_keys)
    }

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
        }
    }
}

impl Claims<CapabilityProvider> {
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
            }),
            expires,
            id: nuid::next(),
            issued_at: since_the_epoch().as_secs(),
            issuer,
            subject,
            not_before,
        }
    }
}

impl Claims<Operator> {
    pub fn new(
        name: String,
        issuer: String,
        subject: String,
        additional_keys: Vec<String>,
    ) -> Claims<Operator> {
        Self::with_dates(name, issuer, subject, None, None, additional_keys)
    }

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
        }
    }
}

impl Claims<Actor> {
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
        }
    }
}

impl Claims<Invocation> {
    pub fn new(
        issuer: String,
        subject: String,
        target_url: &str,
        origin_url: &str,
        hash: &str,
    ) -> Claims<Invocation> {
        Self::with_dates(issuer, subject, None, None, target_url, origin_url, hash)
    }

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

    /// Sets the appropriate metadata for this claims type (e.g. `Actor`, `Operator`, `CapabilityProvider` or `Account`)
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
pub fn validate_token<T>(input: &str) -> Result<TokenValidation>
where
    T: Serialize + DeserializeOwned + WascapEntity,
{
    let segments: Vec<&str> = input.split('.').collect();
    let header_and_claims = format!("{}.{}", segments[0], segments[1]);
    let sig = base64::decode_config(segments[2], base64::URL_SAFE_NO_PAD)?;

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

fn validate_issuer(iss: &String) -> Result<()> {
    if iss.is_empty() {
        Err(errors::new(ErrorKind::MissingIssuer))
    } else {
        Ok(())
    }
}

fn validate_subject(sub: &String) -> Result<()> {
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
    Ok(base64::encode_config(
        encoded.as_bytes(),
        base64::URL_SAFE_NO_PAD,
    ))
}

fn from_jwt_segment<B: AsRef<str>, T: DeserializeOwned>(encoded: B) -> Result<T> {
    let decoded = base64::decode_config(encoded.as_ref(), base64::URL_SAFE_NO_PAD)?;
    let s = String::from_utf8(decoded)?;

    Ok(from_str(&s)?)
}

fn stamp_to_human(stamp: Option<u64>) -> Option<String> {
    stamp.map(|s| {
        let now = NaiveDateTime::from_timestamp(since_the_epoch().as_secs() as i64, 0);
        let then = NaiveDateTime::from_timestamp(s as i64, 0);

        let diff = then - now;

        let ht = chrono_humanize::HumanTime::from(diff);
        format!("{}", ht)
    })
}

fn normalize_call_alias(alias: Option<String>) -> Option<String> {
    alias.map(|a| {
        let mut n = a.to_lowercase();
        n = n.trim().to_string();
        n = n.replace(|c: char| !c.is_ascii(), "");
        n = n.replace(" ", "_");
        n = n.replace("-", "_");
        n = n.replace(".", "_");
        n
    })
}

impl Actor {
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
            module_hash: "".to_string(),
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
        }
    }
}

impl Account {
    pub fn new(name: String, additional_keys: Vec<String>) -> Account {
        Account {
            name: Some(name),
            valid_signers: Some(additional_keys),
        }
    }
}

impl Operator {
    pub fn new(name: String, additional_keys: Vec<String>) -> Operator {
        Operator {
            name: Some(name),
            valid_signers: Some(additional_keys),
        }
    }
}

impl Invocation {
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
    use super::{Account, Actor, Claims, ErrorKind, Invocation, KeyPair, Operator};
    use crate::caps::{KEY_VALUE, LOGGING, MESSAGING};
    use crate::jwt::validate_token;
    use crate::jwt::{since_the_epoch, CapabilityProvider, ClaimsBuilder};
    use std::collections::HashMap;

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
                Some("".to_string()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: Some(since_the_epoch().as_secs() + 1000),
        };

        let encoded = claims.encode(&kp).unwrap();
        let vres = validate_token::<Actor>(&encoded);
        assert!(vres.is_ok());
        if let Ok(v) = vres {
            assert_eq!(v.expired, false);
            assert_eq!(v.cannot_use_yet, true);
            assert_eq!(v.not_before_human, "in 16 minutes");
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
                Some("".to_string()),
                None,
            )),
            expires: Some(since_the_epoch().as_secs() - 30000),
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
        };

        let encoded = claims.encode(&kp).unwrap();
        let vres = validate_token::<Actor>(&encoded);
        assert!(vres.is_ok());
        if let Ok(v) = vres {
            assert!(v.expired);
            assert_eq!(v.cannot_use_yet, false);
            assert_eq!(v.expires_human, "8 hours ago");
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
        };
        let encoded = claims.encode(&issuer).unwrap();
        let vres = validate_token::<Account>(&encoded);
        assert!(vres.is_ok());
        if let Ok(v) = vres {
            assert!(v.expired);
            assert_eq!(v.cannot_use_yet, false);
            assert_eq!(v.expires_human, "8 hours ago");
        }
    }

    #[test]
    fn validate_invocation() {
        let issuer = KeyPair::new_server();
        let claims = Claims {
            id: nuid::next(),
            metadata: Some(Invocation::new(
                "wasmbus://M1234/DeliverMessage",
                "wasmbus://wasmcloud/messaging/default",
                "abc",
            )),
            expires: None,
            not_before: None,
            issued_at: 0,
            issuer: issuer.public_key(),
            subject: "invocation1".to_string(),
        };
        let encoded = claims.encode(&issuer).unwrap();
        let vres = validate_token::<Invocation>(&encoded);
        let decoded = Claims::<Invocation>::decode(&encoded).unwrap();
        assert_eq!(
            decoded.metadata.unwrap().target_url,
            claims.metadata.unwrap().target_url
        );
        assert!(vres.is_ok());
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
                Some("".to_string()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
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
                Some("".to_string()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
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
                Some("".to_string()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
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
    fn encode_decode_logging_roundtrip() {
        let kp = KeyPair::new_account();
        let claims = Claims {
            metadata: Some(Actor::new(
                "test".to_string(),
                Some(vec![MESSAGING.to_string(), LOGGING.to_string()]),
                Some(vec![]),
                false,
                Some(1),
                Some("".to_string()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
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
    fn encode_decode_bad_token() {
        let kp = KeyPair::new_account();
        let claims = Claims {
            metadata: Some(Actor::new(
                "test".to_string(),
                Some(vec![MESSAGING.to_string(), KEY_VALUE.to_string()]),
                Some(vec![]),
                false,
                Some(1),
                Some("".to_string()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
        };

        let encoded_nosep = claims.encode(&kp).unwrap().replace(".", "");

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
                Some("".to_string()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
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
                Some("".to_string()),
                None,
            )),
            expires: None,
            id: nuid::next(),
            issued_at: 0,
            issuer: kp.public_key(),
            subject: "test.wasm".to_string(),
            not_before: None,
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
}
