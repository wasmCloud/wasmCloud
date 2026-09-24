//! Rotating client identity for outbound HTTPS.
//!
//! New connections use the latest valid credential. Existing connections keep
//! their negotiated identity. Expired credentials are not presented.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, Result};
use arc_swap::ArcSwapOption;
use rustls::SignatureScheme;
use rustls::client::ResolvesClientCert;
use rustls::pki_types::CertificateDer;
use rustls::sign::CertifiedKey;
use tracing::{debug, error, warn};

use crate::host::http_client::ClientIdentity;

/// Shortest gap between two repeated reports of the same condition.
const REPORT_INTERVAL: Duration = Duration::from_secs(60);

/// Maximum lead time for an expiry warning.
const MAX_EXPIRY_WARN_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// Divisor used to scale the warning window to the credential lifetime.
const EXPIRY_WARN_FRACTION: u32 = 4;

/// Allowed clock skew for `notBefore` checks.
const CLOCK_SKEW_TOLERANCE: Duration = Duration::from_secs(300);

/// Return one quarter of `lifetime`, capped at one day.
fn warn_window(lifetime: Duration) -> Duration {
    (lifetime / EXPIRY_WARN_FRACTION).min(MAX_EXPIRY_WARN_WINDOW)
}

/// A loaded credential and its effective chain validity.
#[derive(Debug)]
struct Loaded {
    key: Arc<CertifiedKey>,
    not_before: SystemTime,
    not_after: SystemTime,
}

impl Loaded {
    /// How long until this expires, or `None` once it has.
    fn remaining(&self) -> Option<Duration> {
        self.not_after.duration_since(SystemTime::now()).ok()
    }

    /// How long this credential is valid for in total.
    fn lifetime(&self) -> Duration {
        self.not_after
            .duration_since(self.not_before)
            .unwrap_or(MAX_EXPIRY_WARN_WINDOW)
    }
}

/// A client credential that can be replaced without rebuilding TLS state.
///
/// Install with
/// [`ClientTlsOptions::build_with_resolver`](crate::host::http_client::ClientTlsOptions::build_with_resolver)
/// and keep it current with [`spawn_refresh`].
///
/// An expired credential is not presented. A server may reject the connection
/// or continue without client authentication.
#[derive(Debug)]
pub struct RotatingClientIdentity {
    current: ArcSwapOption<Loaded>,
    /// Throttles the refresh loop's expiry reports.
    last_report: Mutex<Option<Instant>>,
    /// Throttles handshake refusal reports.
    last_refusal: Mutex<Option<Instant>>,
}

impl RotatingClientIdentity {
    /// Read `identity` and hold it until it is replaced.
    ///
    /// Rejects expired or not-yet-valid certificate chains.
    pub fn load(identity: &ClientIdentity) -> Result<Arc<Self>> {
        let loaded = Self::read(identity)?;
        Ok(Arc::new(Self {
            current: ArcSwapOption::from(Some(Arc::new(loaded))),
            last_report: Mutex::new(None),
            last_refusal: Mutex::new(None),
        }))
    }

    fn read(identity: &ClientIdentity) -> Result<Loaded> {
        // CertifiedKey requires the process crypto provider.
        crate::init_crypto();

        let (certs, key) = identity.load()?;
        let (not_before, not_after) = validity(&certs)
            .with_context(|| format!("failed to read the validity period of {identity}"))?;
        let now = SystemTime::now();
        anyhow::ensure!(not_after > now, "{identity} has expired");
        anyhow::ensure!(
            not_before <= now + CLOCK_SKEW_TOLERANCE,
            "{identity} is not valid yet"
        );

        let provider = rustls::crypto::CryptoProvider::get_default()
            .context("no rustls crypto provider installed")?;
        let certified = CertifiedKey::from_der(certs, key, provider)
            .with_context(|| format!("{identity} is not a usable client identity"))?;
        Ok(Loaded {
            key: Arc::new(certified),
            not_before,
            not_after,
        })
    }

    /// Re-read `identity`, replacing the held credential if it changed.
    ///
    /// Returns whether the certificate chain changed. A failed read keeps the
    /// current credential.
    pub fn reload(&self, identity: &ClientIdentity) -> Result<bool> {
        let loaded = Self::read(identity)?;
        let changed = self
            .current
            .load()
            .as_ref()
            .is_none_or(|current| current.key.cert != loaded.key.cert);
        if changed {
            self.current.store(Some(Arc::new(loaded)));
        }
        Ok(changed)
    }

    /// Whether a credential is held *and* still valid.
    pub fn is_usable(&self) -> bool {
        self.current
            .load()
            .as_ref()
            .is_some_and(|c| c.remaining().is_some())
    }

    /// Move the held credential's expiry into the past, or back an hour ahead.
    #[cfg(test)]
    pub(crate) fn set_lapsed(&self, lapsed: bool) {
        let current = self.current.load_full().expect("a credential is held");
        let now = SystemTime::now();
        let not_after = if lapsed {
            now - Duration::from_secs(1)
        } else {
            now + Duration::from_secs(3_600)
        };
        self.current.store(Some(Arc::new(Loaded {
            key: Arc::clone(&current.key),
            not_before: current.not_before,
            not_after,
        })));
    }

    /// Whether enough time has passed to report `slot`'s condition again.
    fn due(slot: &Mutex<Option<Instant>>) -> bool {
        let mut last = slot.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_none_or(|at| at.elapsed() >= REPORT_INTERVAL) {
            *last = Some(Instant::now());
            true
        } else {
            false
        }
    }
}

impl ResolvesClientCert for RotatingClientIdentity {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        _sigschemes: &[SignatureScheme],
    ) -> Option<Arc<CertifiedKey>> {
        let current = self.current.load_full()?;
        if current.remaining().is_some() {
            return Some(Arc::clone(&current.key));
        }
        if Self::due(&self.last_refusal) {
            error!(
                "client certificate chain has expired; refusing to authenticate rather than present \
                 it. A peer requiring mutual TLS will reject this connection, and one that only \
                 requests a certificate will serve the call unauthenticated"
            );
        }
        None
    }

    /// True while a credential is configured, even one that has lapsed:
    /// whether a configuration resumes sessions is decided from this, and a
    /// lapse must not let a later credential's sessions be resumed.
    fn has_certs(&self) -> bool {
        self.current.load().is_some()
    }
}

/// Return the chain's effective `(notBefore, notAfter)` span.
fn validity(certs: &[CertificateDer<'_>]) -> Result<(SystemTime, SystemTime)> {
    let at = |label: &str, seconds: i64| -> Result<SystemTime> {
        let seconds = u64::try_from(seconds)
            .map_err(|_| anyhow::anyhow!("certificate {label} predates the unix epoch"))?;
        Ok(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
    };
    let parse = |(index, cert): (usize, &CertificateDer<'_>)| {
        let (_, parsed) = x509_parser::parse_x509_certificate(cert)
            .map_err(|err| anyhow::anyhow!("failed to parse client certificate {index}: {err}"))?;
        Ok::<_, anyhow::Error>((
            at("notBefore", parsed.validity().not_before.timestamp())?,
            at("notAfter", parsed.validity().not_after.timestamp())?,
        ))
    };

    let mut certs = certs.iter().enumerate();
    let first = certs
        .next()
        .context("the client certificate chain is empty")?;
    let (mut not_before, mut not_after) = parse(first)?;
    for cert in certs {
        let (cert_not_before, cert_not_after) = parse(cert)?;
        not_before = not_before.max(cert_not_before);
        not_after = not_after.min(cert_not_after);
    }
    Ok((not_before, not_after))
}

/// Re-read `source` on an interval, replacing `identity` when it changes.
///
/// Polling supports atomically relinked Kubernetes Secret volumes. A dropped
/// handle leaves the task running. Call `abort()` to stop it.
pub fn spawn_refresh(
    identity: Arc<RotatingClientIdentity>,
    source: ClientIdentity,
    interval: Duration,
) -> Result<tokio::task::JoinHandle<()>> {
    anyhow::ensure!(
        !interval.is_zero(),
        "the client identity refresh interval must be greater than zero"
    );
    Ok(tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            // File reads run outside the async executor.
            let reloaded = {
                let identity = Arc::clone(&identity);
                let source = source.clone();
                tokio::task::spawn_blocking(move || identity.reload(&source)).await
            };
            match reloaded {
                Ok(Ok(true)) => debug!(
                    identity = %source,
                    "client identity rotated; new connections will present it"
                ),
                Ok(Ok(false)) => {}
                Ok(Err(err)) => warn!(
                    err = ?err,
                    identity = %source,
                    "failed to reload the client identity; keeping the running credential"
                ),
                Err(err) => warn!(
                    err = %err,
                    identity = %source,
                    "the client identity reload task failed; keeping the running credential"
                ),
            }
            report_expiry(&identity, &source);
        }
    }))
}

/// Report approaching or completed expiry.
fn report_expiry(identity: &RotatingClientIdentity, source: &ClientIdentity) {
    let Some(current) = identity.current.load_full() else {
        return;
    };
    match current.remaining() {
        None if RotatingClientIdentity::due(&identity.last_report) => error!(
            identity = %source,
            "client certificate chain has expired and no valid replacement has been read; outbound \
             calls are no longer authenticated"
        ),
        Some(remaining)
            if remaining <= warn_window(current.lifetime())
                && RotatingClientIdentity::due(&identity.last_report) =>
        {
            warn!(
                identity = %source,
                remaining_secs = remaining.as_secs(),
                "client certificate chain expires soon; rotate it before outbound calls stop being \
                 authenticated"
            )
        }
        _ => {}
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::Path;

    #[derive(Clone, Copy)]
    enum Validity {
        Current,
        Expired,
        NotYet,
        ShortLived,
        JustAhead,
    }

    fn write_pair(dir: &Path, stem: &str, validity: Validity) -> ClientIdentity {
        use rcgen::{CertificateParams, KeyPair};

        let mut params = CertificateParams::new(vec!["client".to_string()]).unwrap();
        let now = SystemTime::now();
        let (before, after) = match validity {
            Validity::Current => (
                now - Duration::from_secs(3_600),
                now + Duration::from_secs(2_592_000),
            ),
            Validity::Expired => (
                now - Duration::from_secs(172_800),
                now - Duration::from_secs(3_600),
            ),
            Validity::NotYet => (
                now + Duration::from_secs(3_600),
                now + Duration::from_secs(172_800),
            ),
            Validity::ShortLived => (
                now - Duration::from_secs(60),
                now + Duration::from_secs(3_540),
            ),
            Validity::JustAhead => (
                now + Duration::from_secs(30),
                now + Duration::from_secs(2_592_000),
            ),
        };
        params.not_before = before.into();
        params.not_after = after.into();
        let key = KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();

        let cert_path = dir.join(format!("{stem}.crt"));
        let key_path = dir.join(format!("{stem}.key"));
        std::fs::write(&cert_path, cert.pem()).unwrap();
        std::fs::write(&key_path, key.serialize_pem()).unwrap();
        ClientIdentity::CertificatePem {
            cert_path,
            key_path,
        }
    }

    fn write_pair_with_expired_intermediate(dir: &Path, stem: &str) -> ClientIdentity {
        use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};

        let now = SystemTime::now();
        let mut root_params = CertificateParams::new(Vec::new()).unwrap();
        root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        root_params.not_before = (now - Duration::from_secs(172_800)).into();
        root_params.not_after = (now + Duration::from_secs(2_592_000)).into();
        let root = CertifiedIssuer::self_signed(root_params, KeyPair::generate().unwrap()).unwrap();

        let mut issuer_params = CertificateParams::new(Vec::new()).unwrap();
        issuer_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        issuer_params.not_before = (now - Duration::from_secs(172_800)).into();
        issuer_params.not_after = (now - Duration::from_secs(3_600)).into();
        let issuer =
            CertifiedIssuer::signed_by(issuer_params, KeyPair::generate().unwrap(), &root).unwrap();

        let mut leaf_params = CertificateParams::new(vec!["client".to_string()]).unwrap();
        leaf_params.not_before = (now - Duration::from_secs(3_600)).into();
        leaf_params.not_after = (now + Duration::from_secs(172_800)).into();
        let key = KeyPair::generate().unwrap();
        let leaf = leaf_params.signed_by(&key, &issuer).unwrap();

        let cert_path = dir.join(format!("{stem}.crt"));
        let key_path = dir.join(format!("{stem}.key"));
        std::fs::write(&cert_path, format!("{}{}", leaf.pem(), issuer.pem())).unwrap();
        std::fs::write(&key_path, key.serialize_pem()).unwrap();
        ClientIdentity::CertificatePem {
            cert_path,
            key_path,
        }
    }

    #[test]
    fn a_loaded_identity_is_presented() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let identity =
            RotatingClientIdentity::load(&write_pair(dir.path(), "id", Validity::Current)).unwrap();
        assert!(identity.has_certs());
        assert!(identity.resolve(&[], &[SignatureScheme::ED25519]).is_some());
    }

    #[test]
    fn an_already_expired_identity_refuses_to_load() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let err = RotatingClientIdentity::load(&write_pair(dir.path(), "old", Validity::Expired))
            .expect_err("an expired pair must not load");
        assert!(
            format!("{err:#}").contains("has expired"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn an_expired_intermediate_refuses_to_load() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let source = write_pair_with_expired_intermediate(dir.path(), "expired-intermediate");
        let err = RotatingClientIdentity::load(&source)
            .expect_err("an expired intermediate must not load");
        assert!(
            format!("{err:#}").contains("has expired"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn a_reload_replaces_what_is_presented() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let source = write_pair(dir.path(), "id", Validity::Current);
        let identity = RotatingClientIdentity::load(&source).unwrap();
        let before = identity.resolve(&[], &[SignatureScheme::ED25519]).unwrap();

        let _ = write_pair(dir.path(), "id", Validity::Current);
        assert!(identity.reload(&source).unwrap(), "the pair changed");

        let after = identity.resolve(&[], &[SignatureScheme::ED25519]).unwrap();
        assert_ne!(before.cert, after.cert);
    }

    #[test]
    fn an_unchanged_pair_is_not_reported_as_rotated() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let source = write_pair(dir.path(), "id", Validity::Current);
        let identity = RotatingClientIdentity::load(&source).unwrap();
        assert!(!identity.reload(&source).unwrap(), "nothing changed");
    }

    #[test]
    fn a_failed_reload_keeps_the_running_credential() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let source = write_pair(dir.path(), "id", Validity::Current);
        let identity = RotatingClientIdentity::load(&source).unwrap();
        let before = identity.resolve(&[], &[SignatureScheme::ED25519]).unwrap();

        let ClientIdentity::CertificatePem { cert_path, .. } = &source;
        std::fs::write(cert_path, b"not a certificate").unwrap();
        assert!(identity.reload(&source).is_err());

        let after = identity.resolve(&[], &[SignatureScheme::ED25519]).unwrap();
        assert_eq!(before.cert, after.cert, "the running credential survives");
    }

    #[test]
    fn an_expired_credential_is_refused_not_presented() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let loaded =
            RotatingClientIdentity::load(&write_pair(dir.path(), "id", Validity::Current)).unwrap();
        let key = loaded.current.load_full().unwrap().key.clone();

        let identity = RotatingClientIdentity {
            current: ArcSwapOption::from(Some(Arc::new(Loaded {
                key,
                not_before: SystemTime::now() - Duration::from_secs(3_600),
                not_after: SystemTime::now() - Duration::from_secs(1),
            }))),
            last_report: Mutex::new(None),
            last_refusal: Mutex::new(None),
        };

        assert!(!identity.is_usable());
        assert!(
            identity.has_certs(),
            "a lapsed credential is still configured"
        );
        assert!(
            identity.resolve(&[], &[SignatureScheme::ED25519]).is_none(),
            "an expired credential must never be presented"
        );
    }

    #[test]
    fn a_not_yet_valid_identity_refuses_to_load() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let err = RotatingClientIdentity::load(&write_pair(dir.path(), "future", Validity::NotYet))
            .expect_err("a not-yet-valid pair must not load");
        assert!(
            format!("{err:#}").contains("not valid yet"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn a_not_yet_valid_reload_keeps_the_running_credential() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let source = write_pair(dir.path(), "id", Validity::Current);
        let identity = RotatingClientIdentity::load(&source).unwrap();
        let before = identity.resolve(&[], &[SignatureScheme::ED25519]).unwrap();

        let _ = write_pair(dir.path(), "id", Validity::NotYet);
        assert!(identity.reload(&source).is_err());

        let after = identity.resolve(&[], &[SignatureScheme::ED25519]).unwrap();
        assert_eq!(before.cert, after.cert);
    }

    #[test]
    fn the_warning_window_scales_with_a_short_lived_credential() {
        let hour = Duration::from_secs(3_600);
        assert_eq!(warn_window(hour), hour / 4);
        assert!(
            warn_window(hour) < hour,
            "a fresh short-lived credential must not warn immediately"
        );

        let year = Duration::from_secs(365 * 24 * 60 * 60);
        assert_eq!(
            warn_window(year),
            MAX_EXPIRY_WARN_WINDOW,
            "a long-lived credential still gets a day's notice, not three months'"
        );
    }

    #[test]
    fn a_freshly_loaded_short_lived_credential_is_not_already_warning() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let loaded =
            RotatingClientIdentity::load(&write_pair(dir.path(), "short", Validity::ShortLived))
                .unwrap();
        let current = loaded.current.load_full().unwrap();
        let remaining = current.remaining().expect("still valid");
        assert!(
            remaining > warn_window(current.lifetime()),
            "a credential one minute into its hour must not already be warning"
        );
    }

    #[tokio::test]
    async fn refresh_works_on_a_current_thread_runtime() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let source = write_pair(dir.path(), "id", Validity::Current);
        let identity = RotatingClientIdentity::load(&source).unwrap();
        let before = identity.resolve(&[], &[SignatureScheme::ED25519]).unwrap();

        let handle = spawn_refresh(
            Arc::clone(&identity),
            source.clone(),
            Duration::from_millis(20),
        )
        .unwrap();
        let _ = write_pair(dir.path(), "id", Validity::Current);

        let rotated = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let now = identity.resolve(&[], &[SignatureScheme::ED25519]).unwrap();
                if now.cert != before.cert {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(
            !handle.is_finished(),
            "the refresh task must not have panicked"
        );
        handle.abort();
        rotated.expect("rotation works without a multi-thread runtime");
    }

    #[tokio::test]
    async fn a_zero_interval_is_refused_before_anything_is_spawned() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let source = write_pair(dir.path(), "id", Validity::Current);
        let identity = RotatingClientIdentity::load(&source).unwrap();
        let err = spawn_refresh(identity, source, Duration::ZERO)
            .expect_err("a zero interval must not be spawned");
        assert!(
            format!("{err:#}").contains("greater than zero"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn a_credential_within_clock_skew_still_loads() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let source = write_pair(dir.path(), "skewed", Validity::JustAhead);
        RotatingClientIdentity::load(&source)
            .expect("a few seconds of clock skew must not fail startup");
    }

    #[test]
    fn a_mismatched_key_keeps_the_running_credential() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let source = write_pair(dir.path(), "id", Validity::Current);
        let identity = RotatingClientIdentity::load(&source).unwrap();
        let before = identity.resolve(&[], &[SignatureScheme::ED25519]).unwrap();
        let ClientIdentity::CertificatePem { key_path, .. } = &source;

        let other = rcgen::KeyPair::generate().unwrap();
        std::fs::write(key_path, other.serialize_pem()).unwrap();
        assert!(identity.reload(&source).is_err());

        let after = identity.resolve(&[], &[SignatureScheme::ED25519]).unwrap();
        assert_eq!(before.cert, after.cert);
    }

    #[test]
    fn the_two_expiry_reports_throttle_independently() {
        let identity = RotatingClientIdentity {
            current: ArcSwapOption::empty(),
            last_report: Mutex::new(None),
            last_refusal: Mutex::new(None),
        };
        assert!(RotatingClientIdentity::due(&identity.last_report));
        assert!(
            RotatingClientIdentity::due(&identity.last_refusal),
            "the loop claiming its slot must not silence the handshake path"
        );
        assert!(!RotatingClientIdentity::due(&identity.last_report));
        assert!(!RotatingClientIdentity::due(&identity.last_refusal));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refresh_picks_up_a_rewritten_credential() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let source = write_pair(dir.path(), "id", Validity::Current);
        let identity = RotatingClientIdentity::load(&source).unwrap();
        let before = identity.resolve(&[], &[SignatureScheme::ED25519]).unwrap();

        let handle = spawn_refresh(
            Arc::clone(&identity),
            source.clone(),
            Duration::from_millis(20),
        )
        .unwrap();
        let _ = write_pair(dir.path(), "id", Validity::Current);

        let rotated = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let now = identity.resolve(&[], &[SignatureScheme::ED25519]).unwrap();
                if now.cert != before.cert {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        handle.abort();
        rotated.expect("the rewritten credential is picked up");
    }
}
