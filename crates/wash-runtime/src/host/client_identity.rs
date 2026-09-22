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

/// How long before expiry to start saying so, leaving time to act before
/// calls stop being authenticated.
const EXPIRY_WARN_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// A loaded credential and the instant it stops being valid.
#[derive(Debug)]
struct Loaded {
    key: Arc<CertifiedKey>,
    not_after: SystemTime,
}

impl Loaded {
    /// How long until this expires, or `None` once it has.
    fn remaining(&self) -> Option<Duration> {
        self.not_after.duration_since(SystemTime::now()).ok()
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
        let (certs, key) = identity.load()?;
        let leaf = certs
            .first()
            .with_context(|| format!("{identity} contains no certificate"))?;
        let not_after = not_after(leaf)
            .with_context(|| format!("failed to read the validity period of {identity}"))?;
        anyhow::ensure!(not_after > SystemTime::now(), "{identity} has expired");

        let provider = rustls::crypto::CryptoProvider::get_default()
            .context("no rustls crypto provider installed")?;
        let certified = CertifiedKey::from_der(certs, key, provider)
            .with_context(|| format!("{identity} is not a usable client identity"))?;
        Ok(Loaded {
            key: Arc::new(certified),
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

/// The leaf certificate's `notAfter`.
fn not_after(cert: &CertificateDer<'_>) -> Result<SystemTime> {
    let (_, parsed) = x509_parser::parse_x509_certificate(cert)
        .map_err(|err| anyhow::anyhow!("failed to parse the client certificate: {err}"))?;
    let seconds = u64::try_from(parsed.validity().not_after.timestamp())
        .map_err(|_| anyhow::anyhow!("certificate expiry predates the unix epoch"))?;
    Ok(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
}

/// Re-read `source` on an interval, replacing `identity` when it changes.
///
/// Polling rather than watching the file: Kubernetes rotates a projected
/// volume by writing a new directory and relinking it, so an inotify watch on
/// the path itself never fires. For the same reason a Secret has to be mounted
/// as a directory rather than by `subPath`, which receives no updates at all.
///
/// The returned handle stops the refresh when dropped or aborted.
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
            match tokio::task::block_in_place(|| identity.reload(&source)) {
                Ok(true) => debug!(
                    identity = %source,
                    "client identity rotated; new connections will present it"
                ),
                Ok(false) => {}
                Err(err) => warn!(
                    err = ?err,
                    identity = %source,
                    "failed to reload the client identity; keeping the running credential"
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
        Some(remaining) if remaining <= EXPIRY_WARN_WINDOW && identity.report_due() => warn!(
            identity = %source,
            remaining_secs = remaining.as_secs(),
            "client certificate expires soon; rotate it before outbound calls stop being \
             authenticated"
        ),
        _ => {}
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Writes a cert/key pair, as an operator mounts one. `expired` puts
    /// `notAfter` in the past.
    fn write_pair(dir: &Path, stem: &str, expired: bool) -> ClientIdentity {
        use rcgen::{CertificateParams, KeyPair};

        let mut params = CertificateParams::new(vec!["client".to_string()]).unwrap();
        let now = SystemTime::now();
        if expired {
            params.not_before = (now - Duration::from_secs(172_800)).into();
            params.not_after = (now - Duration::from_secs(3_600)).into();
        } else {
            params.not_before = (now - Duration::from_secs(3_600)).into();
            params.not_after = (now + Duration::from_secs(2_592_000)).into();
        }
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
        let identity = RotatingClientIdentity::load(&write_pair(dir.path(), "id", false)).unwrap();
        assert!(identity.has_certs());
        assert!(identity.resolve(&[], &[SignatureScheme::ED25519]).is_some());
    }

    /// A dead certificate must not start a host that then looks healthy while
    /// authenticating as nobody.
    #[test]
    fn an_already_expired_identity_refuses_to_load() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let err = RotatingClientIdentity::load(&write_pair(dir.path(), "old", true))
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
        let source = write_pair(dir.path(), "id", false);
        let identity = RotatingClientIdentity::load(&source).unwrap();
        let before = identity.resolve(&[], &[SignatureScheme::ED25519]).unwrap();

        // Same paths, new material: what a rotated Secret looks like.
        let _ = write_pair(dir.path(), "id", false);
        assert!(identity.reload(&source).unwrap(), "the pair changed");

        let after = identity.resolve(&[], &[SignatureScheme::ED25519]).unwrap();
        assert_ne!(before.cert, after.cert);
    }

    #[test]
    fn an_unchanged_pair_is_not_reported_as_rotated() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let source = write_pair(dir.path(), "id", false);
        let identity = RotatingClientIdentity::load(&source).unwrap();
        assert!(!identity.reload(&source).unwrap(), "nothing changed");
    }

    /// A half-written secret caught mid-rotation must not disarm the host.
    #[test]
    fn a_failed_reload_keeps_the_running_credential() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let source = write_pair(dir.path(), "id", false);
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
        let loaded = RotatingClientIdentity::load(&write_pair(dir.path(), "id", false)).unwrap();
        let key = loaded.current.load_full().unwrap().key.clone();

        // Built directly: `load` refuses expired material, so this is the
        // credential lapsing after the host started.
        let identity = RotatingClientIdentity {
            current: ArcSwapOption::from(Some(Arc::new(Loaded {
                key,
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

    #[tokio::test(flavor = "multi_thread")]
    async fn refresh_picks_up_a_rewritten_credential() {
        crate::init_crypto();
        let dir = tempfile::tempdir().unwrap();
        let source = write_pair(dir.path(), "id", false);
        let identity = RotatingClientIdentity::load(&source).unwrap();
        let before = identity.resolve(&[], &[SignatureScheme::ED25519]).unwrap();

        let handle = spawn_refresh(
            Arc::clone(&identity),
            source.clone(),
            Duration::from_millis(20),
        );
        let _ = write_pair(dir.path(), "id", false);

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
