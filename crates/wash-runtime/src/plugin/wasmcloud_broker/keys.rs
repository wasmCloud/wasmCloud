//! Host-held keys behind `signer` and `verifier`.
//!
//! The key never enters the guest: the message is streamed into a MAC or
//! digest context here and only the tag, signature or verdict goes back. ECDSA
//! P-256 with SHA-256 rather than Ed25519, because pure Ed25519 hashes the
//! message twice and cannot be fed a stream.

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use aws_lc_rs::digest::{self, SHA256};
use aws_lc_rs::encoding::AsDer;
use aws_lc_rs::hmac;
use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_FIXED, ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair,
    ParsedPublicKey,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use wasmtime::component::{Accessor, HasData, Source, StreamConsumer, StreamReader, StreamResult};

use super::bindings::wasmcloud::broker::signer::{Algorithm, KeyInfo};
use super::bindings::wasmcloud::broker::types::{DeniedReason, ErrorCode};

/// Default bound on one signed or verified message.
pub const DEFAULT_MAX_MESSAGE: usize = 16 * 1024 * 1024;

/// How long a message stream may go without a chunk before the call fails.
pub const ABSORB_IDLE: Duration = Duration::from_secs(30);

/// Hashed ahead of a context's length. A key with no context refuses a
/// message that begins with it, so a plain message can never pass for a
/// contextual one.
const CONTEXT_LABEL: &[u8] = b"wasmcloud:broker/context\0";

enum SigningKey {
    Hmac(Box<hmac::Key>),
    Ecdsa(Box<EcdsaKeyPair>),
}

/// A key a component may sign with and never read.
pub struct SignerKey {
    key: SigningKey,
    limits: LimitConfig,
    context: Option<Vec<u8>>,
    key_id: Option<String>,
}

/// What one binding may ask of a key: message size, operations in flight and
/// rate. Signer and verifier take the same ones.
#[derive(Clone, Copy)]
struct LimitConfig {
    max_message: usize,
    concurrency: Option<usize>,
    rate: Option<(f64, u32)>,
}

impl Default for LimitConfig {
    fn default() -> Self {
        Self {
            max_message: DEFAULT_MAX_MESSAGE,
            concurrency: None,
            rate: None,
        }
    }
}

/// One binding's count against a key's [`LimitConfig`], so components that
/// share a key do not share a budget.
#[derive(Default)]
pub(super) struct Limits {
    in_flight: Option<Arc<Semaphore>>,
    rate: Option<Mutex<Bucket>>,
}

impl Limits {
    fn new(config: &LimitConfig) -> Self {
        Self {
            in_flight: config.concurrency.map(|n| Arc::new(Semaphore::new(n))),
            rate: config.rate.map(|(per_second, burst)| {
                let burst = f64::from(burst.max(1));
                Mutex::new(Bucket {
                    per_second,
                    burst,
                    tokens: burst,
                    at: Instant::now(),
                })
            }),
        }
    }

    /// A slot to work in, or why there is none.
    fn admit(&self) -> Result<Option<OwnedSemaphorePermit>, ErrorCode> {
        let budget = || ErrorCode::Denied(DeniedReason::BudgetExceeded);
        let permit = match &self.in_flight {
            Some(slots) => Some(
                Arc::clone(slots)
                    .try_acquire_owned()
                    .map_err(|_| budget())?,
            ),
            None => None,
        };
        if let Some(rate) = &self.rate
            && !rate.lock().unwrap_or_else(|e| e.into_inner()).take()
        {
            return Err(budget());
        }
        Ok(permit)
    }
}

/// Builders shared by [`SignerKey`] and [`VerifierKey`].
macro_rules! key_options {
    () => {
        pub fn with_max_message(mut self, bytes: usize) -> Self {
            self.limits.max_message = bytes;
            self
        }

        /// Bind every message to `context`, so a signature or tag made for
        /// one purpose cannot be passed off as one for another with the same
        /// key. What is hashed is [`CONTEXT_LABEL`], the context's length as a
        /// big-endian u64, the context, then the message. Signer and verifier
        /// must agree on it. Never bind the same key material both with and
        /// without a context, or with two contexts: only the plain binding's
        /// refusal of the label keeps them apart.
        pub fn with_context(mut self, context: &[u8]) -> Self {
            self.context = Some(context.to_vec());
            self
        }

        /// At most `limit` operations in progress at once per binding.
        pub fn with_concurrency(mut self, limit: usize) -> Self {
            self.limits.concurrency = Some(limit);
            self
        }

        /// A token bucket per binding: up to `burst` operations back to back,
        /// then `per_second` more each second.
        pub fn with_rate(mut self, per_second: f64, burst: u32) -> Self {
            self.limits.rate = Some((per_second, burst));
            self
        }

        /// A fresh count against this key's limits, for one binding.
        pub(super) fn limits(&self) -> Limits {
            Limits::new(&self.limits)
        }
    };
}

/// A token bucket: `burst` signatures at once, refilled at `per_second`.
struct Bucket {
    per_second: f64,
    burst: f64,
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn take(&mut self) -> bool {
        let now = Instant::now();
        let refill = now.duration_since(self.at).as_secs_f64() * self.per_second;
        self.tokens = (self.tokens + refill).min(self.burst);
        self.at = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

impl SignerKey {
    fn with_key(key: SigningKey) -> Self {
        Self {
            key,
            limits: LimitConfig::default(),
            context: None,
            key_id: None,
        }
    }

    /// HMAC-SHA256 over `secret`.
    pub fn hmac_sha256(secret: &[u8]) -> Self {
        Self::with_key(SigningKey::Hmac(Box::new(hmac::Key::new(
            hmac::HMAC_SHA256,
            secret,
        ))))
    }

    /// ECDSA P-256 with SHA-256 from a PKCS#8 document. Signatures are the
    /// fixed 64-byte `r || s` form.
    pub fn ecdsa_p256(pkcs8: &[u8]) -> anyhow::Result<Self> {
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8)
            .map_err(|e| anyhow::anyhow!("not a P-256 PKCS#8 key: {e}"))?;
        Ok(Self::with_key(SigningKey::Ecdsa(Box::new(pair))))
    }

    /// A fresh ECDSA P-256 key that exists only in this host's memory.
    pub fn generate_ecdsa_p256() -> anyhow::Result<Self> {
        let pair = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING)
            .map_err(|_| anyhow::anyhow!("could not generate a P-256 key"))?;
        Ok(Self::with_key(SigningKey::Ecdsa(Box::new(pair))))
    }

    key_options!();

    /// The identifier verifiers look the key up by.
    pub fn with_key_id(mut self, key_id: &str) -> Self {
        self.key_id = Some(key_id.to_string());
        self
    }

    pub fn algorithm(&self) -> Algorithm {
        match &self.key {
            SigningKey::Hmac(_) => Algorithm::HmacSha256,
            SigningKey::Ecdsa(_) => Algorithm::EcdsaP256Sha256,
        }
    }

    /// DER `SubjectPublicKeyInfo`, or `None` for a MAC key.
    pub fn public_key(&self) -> Option<Vec<u8>> {
        match &self.key {
            SigningKey::Hmac(_) => None,
            SigningKey::Ecdsa(pair) => pair
                .public_key()
                .as_der()
                .ok()
                .map(|der| der.as_ref().to_vec()),
        }
    }

    pub(super) fn info(&self) -> KeyInfo {
        KeyInfo {
            algorithm: self.algorithm(),
            key_id: self.key_id.clone(),
            public_key: self.public_key(),
            context: self.context.clone(),
        }
    }

    fn start(&self) -> Hasher {
        let hasher = match &self.key {
            SigningKey::Hmac(key) => Hasher::Hmac(Box::new(hmac::Context::with_key(key))),
            SigningKey::Ecdsa(_) => Hasher::Digest(Box::new(digest::Context::new(&SHA256))),
        };
        hasher.prefixed(self.context.as_deref())
    }

    fn finish(&self, hasher: Hasher) -> Result<Vec<u8>, ErrorCode> {
        match (&self.key, hasher) {
            (SigningKey::Hmac(_), Hasher::Hmac(ctx)) => Ok(ctx.sign().as_ref().to_vec()),
            (SigningKey::Ecdsa(pair), Hasher::Digest(ctx)) => pair
                .sign_digest(&ctx.finish())
                .map(|sig| sig.as_ref().to_vec())
                .map_err(|_| ErrorCode::Other(Some("signing failed".into()))),
            _ => Err(ErrorCode::Other(None)),
        }
    }
}

enum CheckingKey {
    Hmac(Box<hmac::Key>),
    Ecdsa(Box<ParsedPublicKey>),
}

/// A key a component may check tags or signatures against.
pub struct VerifierKey {
    key: CheckingKey,
    limits: LimitConfig,
    context: Option<Vec<u8>>,
}

impl VerifierKey {
    fn with_key(key: CheckingKey) -> Self {
        Self {
            key,
            limits: LimitConfig::default(),
            context: None,
        }
    }

    /// HMAC-SHA256 over `secret`, for checking webhook signatures and the like.
    pub fn hmac_sha256(secret: &[u8]) -> Self {
        Self::with_key(CheckingKey::Hmac(Box::new(hmac::Key::new(
            hmac::HMAC_SHA256,
            secret,
        ))))
    }

    /// ECDSA P-256 with SHA-256, from a DER `SubjectPublicKeyInfo` or an
    /// uncompressed point. Signatures are the fixed 64-byte form.
    pub fn ecdsa_p256(public_key: &[u8]) -> anyhow::Result<Self> {
        let key = ParsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, public_key)
            .map_err(|e| anyhow::anyhow!("not a P-256 public key: {e}"))?;
        Ok(Self::with_key(CheckingKey::Ecdsa(Box::new(key))))
    }

    key_options!();

    fn start(&self) -> Hasher {
        let hasher = match &self.key {
            CheckingKey::Hmac(key) => Hasher::Hmac(Box::new(hmac::Context::with_key(key))),
            CheckingKey::Ecdsa(_) => Hasher::Digest(Box::new(digest::Context::new(&SHA256))),
        };
        hasher.prefixed(self.context.as_deref())
    }

    fn check(&self, hasher: Hasher, tag: &[u8]) -> bool {
        match (&self.key, hasher) {
            (CheckingKey::Hmac(_), Hasher::Hmac(ctx)) => {
                aws_lc_rs::constant_time::verify_slices_are_equal(ctx.sign().as_ref(), tag).is_ok()
            }
            (CheckingKey::Ecdsa(key), Hasher::Digest(ctx)) => {
                key.verify_digest_sig(&ctx.finish(), tag).is_ok()
            }
            _ => false,
        }
    }
}

enum Hasher {
    Hmac(Box<hmac::Context>),
    Digest(Box<digest::Context>),
}

impl Hasher {
    fn prefixed(mut self, context: Option<&[u8]>) -> Self {
        if let Some(context) = context {
            let len = u64::try_from(context.len()).unwrap_or(u64::MAX);
            self.update(CONTEXT_LABEL);
            self.update(&len.to_be_bytes());
            self.update(context);
        }
        self
    }

    fn update(&mut self, bytes: &[u8]) {
        match self {
            Hasher::Hmac(ctx) => ctx.update(bytes),
            Hasher::Digest(ctx) => ctx.update(bytes),
        }
    }
}

pub(super) async fn sign<T: 'static, D: HasData>(
    accessor: &Accessor<T, D>,
    key: &SignerKey,
    limits: &Limits,
    message: StreamReader<u8>,
) -> wasmtime::Result<Result<Vec<u8>, ErrorCode>> {
    let _permit = match limits.admit() {
        Ok(permit) => permit,
        Err(e) => return Ok(Err(e)),
    };
    Ok(
        match absorb(
            accessor,
            key.start(),
            (key.limits.max_message, key.context.is_none()),
            message,
        )
        .await?
        {
            Ok(hasher) => key.finish(hasher),
            Err(e) => Err(e),
        },
    )
}

pub(super) async fn verify<T: 'static, D: HasData>(
    accessor: &Accessor<T, D>,
    key: &VerifierKey,
    limits: &Limits,
    message: StreamReader<u8>,
    tag: &[u8],
) -> wasmtime::Result<Result<bool, ErrorCode>> {
    let _permit = match limits.admit() {
        Ok(permit) => permit,
        Err(e) => return Ok(Err(e)),
    };
    Ok(absorb(
        accessor,
        key.start(),
        (key.limits.max_message, key.context.is_none()),
        message,
    )
    .await?
    .map(|hasher| key.check(hasher, tag)))
}

async fn absorb<T: 'static, D: HasData>(
    accessor: &Accessor<T, D>,
    hasher: Hasher,
    (limit, plain): (usize, bool),
    message: StreamReader<u8>,
) -> wasmtime::Result<Result<Hasher, ErrorCode>> {
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    let progress = Arc::new(tokio::sync::Notify::new());
    accessor.with(|mut a| {
        message.pipe(
            &mut a,
            Absorb {
                hasher: Some(hasher),
                seen: 0,
                limit,
                head: plain.then_some(0),
                done: Some(tx),
                progress: Arc::clone(&progress),
            },
        )
    })?;
    loop {
        tokio::select! {
            result = &mut rx => {
                return Ok(result.unwrap_or(Err(ErrorCode::Other(Some(
                    "message stream abandoned".into(),
                )))));
            }
            idle = tokio::time::timeout(ABSORB_IDLE, progress.notified()) => {
                if idle.is_err() {
                    return Ok(Err(ErrorCode::Other(Some("message stream went idle".into()))));
                }
            }
        }
    }
}

/// Feeds each chunk into the hasher as it arrives, so a message is never held
/// whole. The runtime drops it at end of stream, which reports the result.
/// Once the call that wanted the result is gone, it stops at the next chunk.
struct Absorb {
    hasher: Option<Hasher>,
    seen: usize,
    limit: usize,
    /// How much of [`CONTEXT_LABEL`] a plain message has begun with so far.
    head: Option<usize>,
    done: Option<tokio::sync::oneshot::Sender<Result<Hasher, ErrorCode>>>,
    progress: Arc<tokio::sync::Notify>,
}

impl Drop for Absorb {
    fn drop(&mut self) {
        if let (Some(tx), Some(hasher)) = (self.done.take(), self.hasher.take()) {
            let _ = tx.send(Ok(hasher));
        }
    }
}

impl<D> StreamConsumer<D> for Absorb {
    type Item = u8;

    fn poll_consume(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        store: wasmtime::StoreContextMut<D>,
        src: Source<Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let this = self.get_mut();
        if this.done.as_ref().is_none_or(|tx| tx.is_closed()) {
            this.hasher.take();
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        let mut src = src.as_direct(store);
        let bytes = src.remaining();
        if bytes.is_empty() {
            return Poll::Ready(Ok(if finish {
                StreamResult::Cancelled
            } else {
                StreamResult::Completed
            }));
        }
        let n = bytes.len();
        this.progress.notify_one();
        this.seen = this.seen.saturating_add(n);
        let refused = if this.seen > this.limit {
            Some(DeniedReason::BudgetExceeded)
        } else {
            this.head = this.head.and_then(|matched| labelled(matched, bytes));
            (this.head == Some(CONTEXT_LABEL.len())).then_some(DeniedReason::OperationNotAllowed)
        };
        if let Some(reason) = refused {
            this.hasher.take();
            if let Some(tx) = this.done.take() {
                let _ = tx.send(Err(ErrorCode::Denied(reason)));
            }
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        if let Some(hasher) = &mut this.hasher {
            hasher.update(bytes);
        }
        src.mark_read(n);
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

/// How much of [`CONTEXT_LABEL`] a message has begun with once `bytes` follow
/// the `matched` it began with, or `None` once it diverges.
fn labelled(matched: usize, bytes: &[u8]) -> Option<usize> {
    let rest = CONTEXT_LABEL.get(matched..)?;
    let n = rest.len().min(bytes.len());
    (rest.get(..n) == bytes.get(..n)).then_some(matched + n)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn run(signer: &SignerKey, message: &[u8]) -> Vec<u8> {
        let mut hasher = signer.start();
        for chunk in message.chunks(3) {
            hasher.update(chunk);
        }
        signer.finish(hasher).unwrap()
    }

    fn check(verifier: &VerifierKey, message: &[u8], tag: &[u8]) -> bool {
        let mut hasher = verifier.start();
        for chunk in message.chunks(5) {
            hasher.update(chunk);
        }
        verifier.check(hasher, tag)
    }

    #[test]
    fn hmac_matches_a_one_shot_tag_and_checks_in_constant_time() {
        let secret = b"webhook-secret";
        let tag = run(&SignerKey::hmac_sha256(secret), b"payload body");
        let expected = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, secret), b"payload body");
        assert_eq!(tag, expected.as_ref());

        let verifier = VerifierKey::hmac_sha256(secret);
        assert!(check(&verifier, b"payload body", &tag));
        assert!(!check(&verifier, b"payload bodx", &tag));
        assert!(!check(&verifier, b"payload body", &tag[..31]));
    }

    #[test]
    fn ecdsa_signatures_verify_against_the_published_key() {
        let signer = SignerKey::generate_ecdsa_p256().unwrap();
        let sig = run(&signer, b"release manifest");
        assert_eq!(sig.len(), 64);

        let public = signer.public_key().unwrap();
        let verifier = VerifierKey::ecdsa_p256(&public).unwrap();
        assert!(check(&verifier, b"release manifest", &sig));
        assert!(!check(&verifier, b"release manifesto", &sig));

        let one_shot =
            aws_lc_rs::signature::UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &public);
        assert!(one_shot.verify(b"release manifest", &sig).is_ok());
    }

    #[test]
    fn a_context_is_length_prefixed_ahead_of_every_message() {
        let mut framed = CONTEXT_LABEL.to_vec();
        framed.extend_from_slice(&4u64.to_be_bytes());
        framed.extend_from_slice(b"ctx:msg");
        let plain = run(&SignerKey::hmac_sha256(b"k"), &framed);
        let prefixed = run(&SignerKey::hmac_sha256(b"k").with_context(b"ctx:"), b"msg");
        assert_eq!(plain, prefixed);

        let ab_c = run(&SignerKey::hmac_sha256(b"k").with_context(b"ab"), b"c");
        let a_bc = run(&SignerKey::hmac_sha256(b"k").with_context(b"a"), b"bc");
        assert_ne!(
            ab_c, a_bc,
            "moving bytes across the boundary changes the tag"
        );
        let no_context = run(&SignerKey::hmac_sha256(b"k"), b"abc");
        assert_ne!(ab_c, no_context);

        let label = CONTEXT_LABEL.len();
        assert_eq!(labelled(0, &framed), Some(label), "the forgery is caught");
        assert_eq!(
            labelled(0, &CONTEXT_LABEL[..10]).and_then(|m| labelled(m, &framed[10..])),
            Some(label),
            "across chunks"
        );
        assert_eq!(labelled(0, b"wasmcloud:broker/contexts"), None);
        assert_eq!(labelled(0, b"wasm"), Some(4));
        assert_eq!(labelled(label, b"anything"), Some(label));

        let info = SignerKey::hmac_sha256(b"k")
            .with_context(b"ctx:")
            .with_key_id("k1")
            .info();
        assert_eq!(info.context.as_deref(), Some(&b"ctx:"[..]));
        assert_eq!(info.key_id.as_deref(), Some("k1"));
        assert!(info.public_key.is_none());
    }

    #[test]
    fn each_binding_has_its_own_rate_and_concurrency() {
        let limited = SignerKey::hmac_sha256(b"k").with_rate(0.0, 2);
        let binding = limited.limits();
        assert!(binding.admit().is_ok());
        assert!(binding.admit().is_ok());
        assert!(matches!(
            binding.admit(),
            Err(ErrorCode::Denied(DeniedReason::BudgetExceeded))
        ));
        assert!(
            limited.limits().admit().is_ok(),
            "another binding is unspent"
        );

        let single = SignerKey::hmac_sha256(b"k").with_concurrency(1);
        let binding = single.limits();
        let held = binding.admit().unwrap();
        assert!(binding.admit().is_err());
        assert!(single.limits().admit().is_ok());
        drop(held);
        assert!(binding.admit().is_ok());

        let verifier = VerifierKey::hmac_sha256(b"k").with_concurrency(1);
        let binding = verifier.limits();
        let held = binding.admit().unwrap();
        assert!(binding.admit().is_err());
        drop(held);
        assert!(binding.admit().is_ok());
    }

    #[test]
    fn signer_and_verifier_round_trip_only_when_their_contexts_agree() {
        let secret = b"shared";
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(
            &ECDSA_P256_SHA256_FIXED_SIGNING,
            &aws_lc_rs::rand::SystemRandom::new(),
        )
        .unwrap();
        let public = SignerKey::ecdsa_p256(pkcs8.as_ref())
            .unwrap()
            .public_key()
            .unwrap();
        for context in [None, Some(&b"purpose:release\n"[..])] {
            let (mut signer, mut verifier) = (
                SignerKey::hmac_sha256(secret),
                VerifierKey::hmac_sha256(secret),
            );
            let mut ecdsa_verifier = VerifierKey::ecdsa_p256(&public).unwrap();
            let mut ecdsa = SignerKey::ecdsa_p256(pkcs8.as_ref()).unwrap();
            if let Some(context) = context {
                signer = signer.with_context(context);
                verifier = verifier.with_context(context);
                ecdsa = ecdsa.with_context(context);
                ecdsa_verifier = ecdsa_verifier.with_context(context);
            }
            let tag = run(&signer, b"message");
            assert!(check(&verifier, b"message", &tag), "{context:?}");
            let sig = run(&ecdsa, b"message");
            assert!(check(&ecdsa_verifier, b"message", &sig), "{context:?}");

            let other = VerifierKey::hmac_sha256(secret).with_context(b"other");
            assert!(!check(&other, b"message", &tag), "{context:?}");
            let other = VerifierKey::ecdsa_p256(&public)
                .unwrap()
                .with_context(b"other");
            assert!(!check(&other, b"message", &sig), "{context:?}");
        }
    }

    #[test]
    fn a_mac_key_has_no_public_half() {
        assert!(SignerKey::hmac_sha256(b"k").public_key().is_none());
    }

    #[test]
    fn a_verifier_refuses_a_key_that_is_not_p256() {
        assert!(VerifierKey::ecdsa_p256(b"not a key").is_err());
    }
}
