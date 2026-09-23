//! Reporting for a peer's client-certificate request the host cannot satisfy.
//!
//! rustls declines a CertificateRequest quietly in two situations, and from
//! outside the host they look identical. When the peer merely *requests* a
//! certificate rather than requiring one, the handshake completes, the peer
//! answers, and whatever it does about the missing credential happens at the
//! application layer — so an operator sees an upstream rejecting requests and
//! nothing at all suggesting TLS.
//!
//! - **No identity configured.** `with_no_client_auth` installs a resolver
//!   that returns nothing.
//! - **An identity that cannot sign for the schemes offered.** rustls asks the
//!   resolver, then checks `choose_scheme` on what came back; if no offered
//!   scheme fits it sends an *empty* certificate. An Ed25519 credential
//!   against a peer offering only RSA and ECDSA does exactly this.
//!
//! rustls logs one `debug!` covering both, which is of little help to an
//! operator who has no reason to suspect TLS and therefore no reason to raise
//! the log level of a TLS library. [`ReportClientAuth`] wraps whichever
//! resolver would otherwise be installed, tells the two cases apart, and says
//! so at warn level.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash as _, Hasher as _};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustls::SignatureScheme;
use rustls::client::ResolvesClientCert;
use rustls::sign::CertifiedKey;
use tracing::warn;

/// Shortest gap between two reports about the same peer.
const REPORT_INTERVAL: Duration = Duration::from_secs(60);
/// Limits retained reports when peers vary their advertised issuers.
const MAX_REPORTED_PEERS: usize = 256;

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
    reported: Mutex<HashMap<u64, Instant>>,
}

impl ReportClientAuth {
    /// A resolver for a host with no client identity configured.
    #[must_use]
    pub fn absent() -> Self {
        Self {
            inner: None,
            reported: Mutex::new(HashMap::new()),
        }
    }

    /// Wrap the resolver that would otherwise be installed.
    #[must_use]
    pub fn wrapping(inner: Arc<dyn ResolvesClientCert>) -> Self {
        Self {
            inner: Some(inner),
            reported: Mutex::new(HashMap::new()),
        }
    }

    /// Whether this peer is due a report again.
    fn due(&self, peer: u64) -> bool {
        let mut reported = self.reported.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        match reported.get(&peer) {
            Some(at) if now.duration_since(*at) < REPORT_INTERVAL => false,
            _ => {
                if !reported.contains_key(&peer) && reported.len() >= MAX_REPORTED_PEERS {
                    reported.retain(|_, at| now.duration_since(*at) < REPORT_INTERVAL);
                    if reported.len() >= MAX_REPORTED_PEERS {
                        return false;
                    }
                }
                reported.insert(peer, now);
                true
            }
        }
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
        if key.key.choose_scheme(sigschemes).is_some() {
            return Some(key);
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
        assert!(!resolver.due(MAX_REPORTED_PEERS as u64));
        assert_eq!(resolver.reported.lock().unwrap().len(), MAX_REPORTED_PEERS);

        resolver
            .reported
            .lock()
            .unwrap()
            .insert(0, Instant::now() - REPORT_INTERVAL);
        assert!(resolver.due(MAX_REPORTED_PEERS as u64));
        let reported = resolver.reported.lock().unwrap();
        assert_eq!(reported.len(), MAX_REPORTED_PEERS);
        assert!(!reported.contains_key(&0));
    }
}
