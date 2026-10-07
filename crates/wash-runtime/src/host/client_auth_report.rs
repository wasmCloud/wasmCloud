//! Warns when an outbound TLS client-certificate request cannot be satisfied.
//!
//! Reports an absent identity, a declined identity, or a key that cannot sign
//! with the peer's offered schemes.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash as _, Hasher as _};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustls::client::ResolvesClientCert;
use rustls::pki_types::SubjectPublicKeyInfoDer;
use rustls::sign::{CertifiedKey, Signer, SigningKey};
use rustls::{SignatureAlgorithm, SignatureScheme};
use tracing::warn;

/// Shortest gap between two reports about the same peer.
const REPORT_INTERVAL: Duration = Duration::from_secs(60);
/// Limits retained reports when peers vary their advertised issuers.
const MAX_REPORTED_PEERS: usize = 256;
/// Leaves room for new peers after every tracked peer reports again.
const MAX_REPORTS_PER_INTERVAL: usize = MAX_REPORTED_PEERS * 2;

#[derive(Debug)]
struct ReportState {
    peers: HashMap<u64, Instant>,
    window_started: Instant,
    reports_in_window: usize,
}

impl ReportState {
    fn new() -> Self {
        Self {
            peers: HashMap::new(),
            window_started: Instant::now(),
            reports_in_window: 0,
        }
    }
}

/// Hands rustls the signer selected during certificate resolution.
#[derive(Debug)]
struct PreselectedSigningKey {
    inner: Arc<dyn SigningKey>,
    signer: Mutex<Option<Box<dyn Signer>>>,
}

impl SigningKey for PreselectedSigningKey {
    fn choose_scheme(&self, offered: &[SignatureScheme]) -> Option<Box<dyn Signer>> {
        let signer = self.signer.lock().unwrap_or_else(|e| e.into_inner()).take();
        signer.or_else(|| self.inner.choose_scheme(offered))
    }

    fn public_key(&self) -> Option<SubjectPublicKeyInfoDer<'_>> {
        self.inner.public_key()
    }

    fn algorithm(&self) -> SignatureAlgorithm {
        self.inner.algorithm()
    }
}

/// Reports a peer's client-certificate request that this host cannot satisfy.
///
/// Installed by
/// [`ClientTlsOptions::build`](crate::host::http_client::ClientTlsOptions::build)
/// and
/// [`build_with_resolver`](crate::host::http_client::ClientTlsOptions::build_with_resolver),
/// so a host gets this without asking. Wrapping is transparent: the inner
/// resolver decides what is presented, and this only observes and reports.
#[derive(Debug)]
pub struct ReportClientAuth {
    /// The resolver that decides what to present. `None` means no identity is
    /// configured, which is [`rustls`]'s `FailResolveClientCert` case.
    inner: Option<Arc<dyn ResolvesClientCert>>,
    /// Keyed by peer rather than shared. One stamp would let a peer that
    /// merely requests a certificate hold the throttle and mask the peer whose
    /// handshake is actually failing.
    reported: Mutex<ReportState>,
}

impl ReportClientAuth {
    /// A resolver for a host with no client identity configured.
    #[must_use]
    pub fn absent() -> Self {
        Self {
            inner: None,
            reported: Mutex::new(ReportState::new()),
        }
    }

    /// Wrap the resolver that would otherwise be installed.
    #[must_use]
    pub fn wrapping(inner: Arc<dyn ResolvesClientCert>) -> Self {
        Self {
            inner: Some(inner),
            reported: Mutex::new(ReportState::new()),
        }
    }

    /// Whether this peer is due a report again.
    fn due(&self, peer: u64) -> bool {
        let mut reported = self.reported.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if now.duration_since(reported.window_started) >= REPORT_INTERVAL {
            reported.window_started = now;
            reported.reports_in_window = 0;
        }
        if reported
            .peers
            .get(&peer)
            .is_some_and(|at| now.duration_since(*at) < REPORT_INTERVAL)
            || reported.reports_in_window >= MAX_REPORTS_PER_INTERVAL
        {
            return false;
        }
        if !reported.peers.contains_key(&peer) && reported.peers.len() >= MAX_REPORTED_PEERS {
            reported
                .peers
                .retain(|_, at| now.duration_since(*at) < REPORT_INTERVAL);
            if reported.peers.len() >= MAX_REPORTED_PEERS {
                if let Some(oldest) = reported
                    .peers
                    .iter()
                    .min_by_key(|(_, at)| *at)
                    .map(|(peer, _)| *peer)
                {
                    reported.peers.remove(&oldest);
                }
            }
        }
        reported.peers.insert(peer, now);
        reported.reports_in_window += 1;
        true
    }
}

/// Identifies a peer by the certificate issuers it advertises.
///
/// rustls passes no server name to this callback, so the acceptable-issuer
/// list is the only thing distinguishing one peer from another here. A peer
/// that advertises none collapses with every other such peer, which costs a
/// report rather than correctness.
fn peer_key(root_hint_subjects: &[&[u8]]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for subject in root_hint_subjects {
        subject.hash(&mut hasher);
    }
    hasher.finish()
}

impl ResolvesClientCert for ReportClientAuth {
    fn resolve(
        &self,
        root_hint_subjects: &[&[u8]],
        sigschemes: &[SignatureScheme],
    ) -> Option<Arc<CertifiedKey>> {
        let Some(inner) = &self.inner else {
            if self.due(peer_key(root_hint_subjects)) {
                warn!(
                    acceptable_issuers = root_hint_subjects.len(),
                    "peer requested a client certificate for outbound TLS and this host presents \
                     none; the call proceeds unauthenticated if the peer allows it"
                );
            }
            return None;
        };

        let Some(key) = inner.resolve(root_hint_subjects, sigschemes) else {
            // The inner resolver declined on its own terms — an expired
            // rotating credential, say.
            if self.due(peer_key(root_hint_subjects)) {
                warn!(
                    acceptable_issuers = root_hint_subjects.len(),
                    "peer requested a client certificate and this host declined to present one; \
                     the call proceeds unauthenticated if the peer allows it"
                );
            }
            return None;
        };

        // rustls repeats this check after the resolver returns and, on
        // failure, sends an empty certificate having logged only at debug.
        // Doing it here too is what lets the reason be named.
        if let Some(signer) = key.key.choose_scheme(sigschemes) {
            let mut selected = (*key).clone();
            selected.key = Arc::new(PreselectedSigningKey {
                inner: Arc::clone(&key.key),
                signer: Mutex::new(Some(signer)),
            });
            return Some(Arc::new(selected));
        }
        if self.due(peer_key(root_hint_subjects)) {
            warn!(
                acceptable_issuers = root_hint_subjects.len(),
                offered_schemes = ?sigschemes,
                "peer requested a client certificate and this host has one, but it signs for none \
                 of the signature schemes the peer accepts; the call proceeds unauthenticated if \
                 the peer allows it"
            );
        }
        None
    }

    fn has_certs(&self) -> bool {
        self.inner.as_ref().is_some_and(|inner| inner.has_certs())
    }

    fn only_raw_public_keys(&self) -> bool {
        self.inner
            .as_ref()
            .is_some_and(|inner| inner.only_raw_public_keys())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A resolver that declines, as a rotating identity does once its
    /// credential has expired.
    #[derive(Debug)]
    struct Declines;

    impl ResolvesClientCert for Declines {
        fn resolve(&self, _: &[&[u8]], _: &[SignatureScheme]) -> Option<Arc<CertifiedKey>> {
            None
        }

        fn has_certs(&self) -> bool {
            true
        }
    }

    #[derive(Debug)]
    struct RawPublicKey;

    impl ResolvesClientCert for RawPublicKey {
        fn resolve(&self, _: &[&[u8]], _: &[SignatureScheme]) -> Option<Arc<CertifiedKey>> {
            None
        }

        fn has_certs(&self) -> bool {
            true
        }

        fn only_raw_public_keys(&self) -> bool {
            true
        }
    }

    #[derive(Debug)]
    struct OneShotSigningKey {
        inner: Arc<dyn SigningKey>,
        calls: Arc<AtomicUsize>,
    }

    impl SigningKey for OneShotSigningKey {
        fn choose_scheme(&self, offered: &[SignatureScheme]) -> Option<Box<dyn Signer>> {
            if self.calls.fetch_add(1, Ordering::Relaxed) == 0 {
                self.inner.choose_scheme(offered)
            } else {
                None
            }
        }

        fn algorithm(&self) -> SignatureAlgorithm {
            self.inner.algorithm()
        }
    }

    /// A real credential of the given algorithm.
    fn resolver_for(alg: &'static rcgen::SignatureAlgorithm) -> Arc<dyn ResolvesClientCert> {
        crate::init_crypto();
        let key = rcgen::KeyPair::generate_for(alg).unwrap();
        let cert = rcgen::CertificateParams::new(vec!["client".to_string()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let provider = rustls::crypto::CryptoProvider::get_default().unwrap();
        let certified = CertifiedKey::from_der(
            vec![cert.der().clone()],
            rustls::pki_types::PrivateKeyDer::try_from(key.serialize_der()).unwrap(),
            provider,
        )
        .unwrap();
        Arc::new(rustls::sign::SingleCertAndKey::from(certified))
    }

    #[test]
    fn an_absent_identity_presents_nothing() {
        let resolver = ReportClientAuth::absent();
        assert!(!resolver.has_certs());
        assert!(resolver.resolve(&[], &[SignatureScheme::ED25519]).is_none());
    }

    #[test]
    fn a_declining_resolver_is_passed_through() {
        let resolver = ReportClientAuth::wrapping(Arc::new(Declines));
        assert!(resolver.has_certs(), "the inner resolver reports a cert");
        assert!(
            resolver
                .resolve(&[b"issuer".as_slice()], &[SignatureScheme::ED25519])
                .is_none()
        );
    }

    #[test]
    fn raw_public_key_support_is_passed_through() {
        let resolver = ReportClientAuth::wrapping(Arc::new(RawPublicKey));
        assert!(resolver.only_raw_public_keys());
        assert!(!ReportClientAuth::absent().only_raw_public_keys());
    }

    /// The gap this module exists for: rustls sends an empty certificate and
    /// logs at debug when the credential signs for no offered scheme.
    #[test]
    fn an_identity_that_cannot_sign_for_the_offered_schemes_is_refused() {
        let resolver = ReportClientAuth::wrapping(resolver_for(&rcgen::PKCS_ED25519));
        assert!(
            resolver
                .resolve(
                    &[b"issuer".as_slice()],
                    &[
                        SignatureScheme::RSA_PKCS1_SHA256,
                        SignatureScheme::ECDSA_NISTP256_SHA256,
                    ],
                )
                .is_none(),
            "an Ed25519 key signs for neither RSA nor ECDSA"
        );
    }

    #[test]
    fn an_identity_the_peer_accepts_is_presented() {
        let resolver = ReportClientAuth::wrapping(resolver_for(&rcgen::PKCS_ED25519));
        assert!(
            resolver
                .resolve(&[b"issuer".as_slice()], &[SignatureScheme::ED25519])
                .is_some()
        );
    }

    /// Wrapping must not change what is presented, only what is said about it.
    #[test]
    fn wrapping_is_transparent_to_a_satisfiable_request() {
        let inner = resolver_for(&rcgen::PKCS_ECDSA_P256_SHA256);
        let schemes = [SignatureScheme::ECDSA_NISTP256_SHA256];
        let direct = inner.resolve(&[], &schemes).unwrap();
        let wrapped = ReportClientAuth::wrapping(Arc::clone(&inner))
            .resolve(&[], &schemes)
            .unwrap();
        assert_eq!(direct.cert, wrapped.cert);
    }

    #[test]
    fn signing_key_is_consulted_once_per_handshake() {
        let schemes = [SignatureScheme::ED25519];
        let original = resolver_for(&rcgen::PKCS_ED25519)
            .resolve(&[], &schemes)
            .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut one_shot = (*original).clone();
        one_shot.key = Arc::new(OneShotSigningKey {
            inner: Arc::clone(&original.key),
            calls: Arc::clone(&calls),
        });
        let resolver =
            ReportClientAuth::wrapping(Arc::new(rustls::sign::SingleCertAndKey::from(one_shot)));
        let selected = resolver.resolve(&[], &schemes).unwrap();
        assert!(selected.key.choose_scheme(&schemes).is_some());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn one_peer_is_reported_once_and_another_separately() {
        let resolver = ReportClientAuth::absent();
        let a = [b"issuer-a".as_slice()];
        let b = [b"issuer-b".as_slice()];
        assert!(resolver.due(peer_key(&a)), "first sighting of a");
        assert!(!resolver.due(peer_key(&a)), "a is throttled");
        assert!(
            resolver.due(peer_key(&b)),
            "a holding the throttle must not silence b"
        );
    }

    #[test]
    fn report_cache_is_bounded_and_reuses_expired_slots() {
        let resolver = ReportClientAuth::absent();
        for peer in 0..MAX_REPORTED_PEERS as u64 {
            assert!(resolver.due(peer));
        }
        assert_eq!(
            resolver.reported.lock().unwrap().peers.len(),
            MAX_REPORTED_PEERS
        );

        let mut reported = resolver.reported.lock().unwrap();
        reported.peers.insert(0, Instant::now() - REPORT_INTERVAL);
        drop(reported);
        assert!(resolver.due(MAX_REPORTED_PEERS as u64));
        let reported = resolver.reported.lock().unwrap();
        assert_eq!(reported.peers.len(), MAX_REPORTED_PEERS);
        assert!(!reported.peers.contains_key(&0));
    }

    #[test]
    fn full_report_cache_evicts_an_active_peer() {
        let resolver = ReportClientAuth::absent();
        for peer in 0..MAX_REPORTED_PEERS as u64 {
            assert!(resolver.due(peer));
        }
        let mut reported = resolver.reported.lock().unwrap();
        reported
            .peers
            .insert(0, Instant::now() - Duration::from_secs(30));
        drop(reported);

        assert!(resolver.due(MAX_REPORTED_PEERS as u64));
        let reported = resolver.reported.lock().unwrap();
        assert_eq!(reported.peers.len(), MAX_REPORTED_PEERS);
        assert!(!reported.peers.contains_key(&0));
    }

    #[test]
    fn report_volume_is_bounded() {
        let resolver = ReportClientAuth::absent();
        for peer in 0..MAX_REPORTS_PER_INTERVAL as u64 {
            assert!(resolver.due(peer));
        }
        assert!(!resolver.due(MAX_REPORTS_PER_INTERVAL as u64));
        assert_eq!(
            resolver.reported.lock().unwrap().peers.len(),
            MAX_REPORTED_PEERS
        );
    }
}
