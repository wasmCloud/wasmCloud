//! Rotating client identity for components' outbound HTTPS.
//!
//! [`ClientTlsOptions::client_identity`] is read once when the configuration
//! is built, which is fine for a credential an operator installs by hand and
//! wrong for one an issuer renews. A certificate that outlives the host
//! process is the exception, not the rule: cert-manager, SPIRE and the like
//! all rotate on a schedule far shorter than a host's uptime.
//!
//! [`RotatingClientIdentity`] closes that gap. rustls consults a
//! [`ResolvesClientCert`] once per handshake, so replacing the credential
//! behind one takes effect on the next connection while established
//! connections keep what they negotiated with. No pool is drained and nothing
//! restarts.
//!
//! Authentication fails closed: an expired credential is refused rather than
//! offered, and one that is already expired fails to load at all.
//!
//! [`ClientTlsOptions::client_identity`]: crate::host::http_client::ClientTlsOptions::client_identity

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

/// The longest lead time on an expiry warning.
///
/// Capped rather than fixed: a SPIRE SVID defaults to an hour, so a flat
/// 24-hour window would warn from the moment a healthy credential loaded and
/// keep warning for its whole life, which teaches operators to ignore the one
/// message that matters. See [`warn_window`].
const MAX_EXPIRY_WARN_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// How much of a credential's life is spent warning that it is nearly over.
const EXPIRY_WARN_FRACTION: u32 = 4;

/// How long before expiry to start saying so, for a credential valid over
/// `lifetime`.
///
/// A quarter of its own lifetime, capped at [`MAX_EXPIRY_WARN_WINDOW`], so a
/// short-lived credential warns late rather than always and a long-lived one
/// still gets a day's notice.
fn warn_window(lifetime: Duration) -> Duration {
    (lifetime / EXPIRY_WARN_FRACTION).min(MAX_EXPIRY_WARN_WINDOW)
}

/// A loaded credential and the instant it stops being valid.
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

/// A client credential that can be replaced without rebuilding the
/// configuration it is installed on.
///
/// Install with
/// [`ClientTlsOptions::build_with_resolver`](crate::host::http_client::ClientTlsOptions::build_with_resolver)
/// and keep it current with [`spawn_refresh`].
///
/// An expired credential is refused rather than presented. rustls has no
/// error path in this callback, so declining to authenticate is the strongest
/// move available: a peer requiring mutual TLS drops the handshake, which is
/// what we want, while a peer that merely *requests* a certificate still
/// serves the call unauthenticated. Nothing downstream reports that second
/// case, which is why it is logged at error level.
#[derive(Debug)]
pub struct RotatingClientIdentity {
    current: ArcSwapOption<Loaded>,
    last_report: Mutex<Option<Instant>>,
}

impl RotatingClientIdentity {
    /// Read `identity` and hold it until it is replaced.
    ///
    /// Fails for the same reasons building a static identity does, plus one:
    /// a certificate that has already expired is refused here rather than
    /// leaving a host that looks healthy while authenticating as nobody.
    pub fn load(identity: &ClientIdentity) -> Result<Arc<Self>> {
        let loaded = Self::read(identity)?;
        Ok(Arc::new(Self {
            current: ArcSwapOption::from(Some(Arc::new(loaded))),
            last_report: Mutex::new(None),
        }))
    }

    fn read(identity: &ClientIdentity) -> Result<Loaded> {
        // Installs the process default, which `CryptoProvider::get_default`
        // returns `None` without. Every other entry point in this crate does
        // the same; reaching here first is not unusual, since a caller builds
        // the identity before the configuration it goes on.
        crate::init_crypto();

        let (certs, key) = identity.load()?;
        let leaf = certs
            .first()
            .with_context(|| format!("{identity} contains no certificate"))?;
        let (not_before, not_after) = validity(leaf)
            .with_context(|| format!("failed to read the validity period of {identity}"))?;
        let now = SystemTime::now();
        anyhow::ensure!(not_after > now, "{identity} has expired");
        // Rejected rather than installed: a credential pre-staged by an issuer
        // or written under clock skew would otherwise replace a running one
        // that still works, and then be refused by every peer.
        anyhow::ensure!(not_before <= now, "{identity} is not valid yet");

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
    /// Returns whether it changed. A read that fails leaves the running
    /// credential in place: a half-written secret caught mid-rotation must
    /// not take the host's identity away. That is not a way around expiry,
    /// since the running credential is refused on its own merits once it
    /// lapses.
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

    /// Whether enough time has passed to report the same condition again.
    fn report_due(&self) -> bool {
        let mut last = self.last_report.lock().unwrap_or_else(|e| e.into_inner());
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
        if self.report_due() {
            error!(
                "client certificate has expired; refusing to authenticate rather than present \
                 it. A peer requiring mutual TLS will reject this connection, and one that only \
                 requests a certificate will serve the call unauthenticated"
            );
        }
        None
    }

    fn has_certs(&self) -> bool {
        self.is_usable()
    }
}

/// The leaf certificate's validity span, as `(notBefore, notAfter)`.
fn validity(cert: &CertificateDer<'_>) -> Result<(SystemTime, SystemTime)> {
    let (_, parsed) = x509_parser::parse_x509_certificate(cert)
        .map_err(|err| anyhow::anyhow!("failed to parse the client certificate: {err}"))?;
    let at = |label: &str, seconds: i64| -> Result<SystemTime> {
        let seconds = u64::try_from(seconds)
            .map_err(|_| anyhow::anyhow!("certificate {label} predates the unix epoch"))?;
        Ok(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
    };
    Ok((
        at("notBefore", parsed.validity().not_before.timestamp())?,
        at("notAfter", parsed.validity().not_after.timestamp())?,
    ))
}

/// Re-read `source` on an interval, replacing `identity` when it changes.
///
/// Polling rather than watching the file: Kubernetes rotates a projected
/// volume by writing a new directory and relinking it, so an inotify watch on
/// the path itself never fires. For the same reason a Secret has to be mounted
/// as a directory rather than by `subPath`, which receives no updates at all.
///
/// The returned handle does *not* stop the refresh when dropped — a dropped
/// `JoinHandle` detaches its task. Call `abort()` to stop it, or drop it
/// deliberately to let the refresh run for the life of the process.
pub fn spawn_refresh(
    identity: Arc<RotatingClientIdentity>,
    source: ClientIdentity,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;
        loop {
            ticker.tick().await;
            // `spawn_blocking`, not `block_in_place`: the latter panics on a
            // current-thread runtime, and since callers are expected to drop
            // the handle that panic would never be observed — rotation would
            // stop for good with nothing logged.
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
    })
}

/// Say so while there is still time to act, and keep saying so once there is
/// not.
fn report_expiry(identity: &RotatingClientIdentity, source: &ClientIdentity) {
    let Some(current) = identity.current.load_full() else {
        return;
    };
    match current.remaining() {
        None if identity.report_due() => error!(
            identity = %source,
            "client certificate has expired and no valid replacement has been read; outbound \
             calls are no longer authenticated"
        ),
        Some(remaining)
            if remaining <= warn_window(current.lifetime()) && identity.report_due() =>
        {
            warn!(
                identity = %source,
                remaining_secs = remaining.as_secs(),
                "client certificate expires soon; rotate it before outbound calls stop being \
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

    /// Where a written pair sits relative to now.
    #[derive(Clone, Copy)]
    enum Validity {
        Current,
        Expired,
        NotYet,
        /// Valid, but only briefly — a SPIRE-shaped short-lived credential.
        ShortLived,
    }

    /// Writes a cert/key pair, as an operator mounts one.
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

    #[test]
    fn a_loaded_identity_is_presented() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let identity =
            RotatingClientIdentity::load(&write_pair(dir.path(), "id", Validity::Current)).unwrap();
        assert!(identity.has_certs());
        assert!(identity.resolve(&[], &[SignatureScheme::ED25519]).is_some());
    }

    /// A dead certificate must not start a host that then looks healthy while
    /// authenticating as nobody.
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

    /// The point of the type: a swap is visible to the next handshake.
    #[test]
    fn a_reload_replaces_what_is_presented() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let source = write_pair(dir.path(), "id", Validity::Current);
        let identity = RotatingClientIdentity::load(&source).unwrap();
        let before = identity.resolve(&[], &[SignatureScheme::ED25519]).unwrap();

        // Same paths, new material: what a rotated Secret looks like.
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

    /// A half-written secret caught mid-rotation must not disarm the host.
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

    /// Authentication fails closed: once the credential lapses while
    /// resident, it stops being offered rather than being sent in the hope
    /// that the peer is lenient.
    #[test]
    fn an_expired_credential_is_refused_not_presented() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let loaded =
            RotatingClientIdentity::load(&write_pair(dir.path(), "id", Validity::Current)).unwrap();
        let key = loaded.current.load_full().unwrap().key.clone();

        // Built directly: `load` refuses expired material, so this is the
        // credential lapsing after the host started.
        let identity = RotatingClientIdentity {
            current: ArcSwapOption::from(Some(Arc::new(Loaded {
                key,
                not_before: SystemTime::now() - Duration::from_secs(3_600),
                not_after: SystemTime::now() - Duration::from_secs(1),
            }))),
            last_report: Mutex::new(None),
        };

        assert!(!identity.is_usable());
        assert!(!identity.has_certs());
        assert!(
            identity.resolve(&[], &[SignatureScheme::ED25519]).is_none(),
            "an expired credential must never be presented"
        );
    }

    /// A credential pre-staged by an issuer, or written under clock skew, must
    /// not replace one that still works only to be refused by every peer.
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

    /// A SPIRE SVID defaults to an hour. A flat 24-hour warning window would
    /// fire from the moment such a credential loaded and never stop, which is
    /// how an operator learns to ignore it.
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

    /// The scaled window read off a real certificate, not just arithmetic.
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

    /// Default flavor: a current-thread runtime, which `block_in_place`
    /// panics on. The panic would be unobservable because callers drop the
    /// handle, so rotation would stop for good with nothing logged.
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
        );
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
        );
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
