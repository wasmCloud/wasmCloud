//! Identity the host holds for each workload.
//!
//! An issuer mints X.509-SVIDs and JWT-SVIDs for a workload. The host keeps
//! them here, renews them before they expire, presents them on the workload's
//! behalf and never hands them to the guest. The host generates every private
//! key and sends the issuer only a certificate signing request.
//!
//! Lifecycle, per workload id:
//! 1. A start binds what the control plane granted under its reservation,
//!    through [`crate::host::Host::workload_start_with_identity`] or the
//!    washlet.
//! 2. The host admits it with the digests of the components it loaded. A digest outside the approved set refuses the
//!    start; otherwise renewal begins, one task per credential.
//! 3. [`WorkloadCredentials::renew_lease`] replaces the scheduling lease the
//!    issuer is shown. Once it expires the host asks for nothing more.
//! 4. [`WorkloadCredentials::revoke`] stops renewal and drops the material
//!    as soon as the workload starts to stop; its mTLS destinations then
//!    refuse connections until [`WorkloadCredentials::release`] forgets it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime};

use arc_swap::ArcSwapOption;
use async_trait::async_trait;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use tracing::{debug, warn};
use zeroize::Zeroizing;

use crate::host::client_identity::RotatingClientIdentity;
use crate::host::egress_credential::{CredentialError, EgressCredentialSource};
use crate::host::http_client::{ClientTlsConfigResolver, DestinationTls};

/// First wait after a failed issuance. Doubles, with jitter, up to
/// [`MAX_BACKOFF`]. Also how often an expired lease is checked again.
const RETRY: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// Longest one issuer call may take.
const ISSUE_TIMEOUT: Duration = Duration::from_secs(30);

/// Renewal never comes sooner than this after an issuance, nor later than
/// half of [`MAX_LIFETIME`], whatever lifetime the issuer claims.
const MIN_RENEWAL: Duration = Duration::from_secs(1);
const MAX_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// Proof from the control plane that this host may run a workload, presented
/// to the issuer with every request. Opaque to the host apart from its expiry.
pub struct SchedulingLease {
    token: Zeroizing<Vec<u8>>,
    expires_at: SystemTime,
}

impl SchedulingLease {
    pub fn new(token: impl Into<Vec<u8>>, expires_at: SystemTime) -> Self {
        Self {
            token: Zeroizing::new(token.into()),
            expires_at,
        }
    }

    pub fn expires_at(&self) -> SystemTime {
        self.expires_at
    }

    fn is_current(&self, now: SystemTime) -> bool {
        self.expires_at > now
    }
}

impl std::fmt::Debug for SchedulingLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SchedulingLease")
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// An HTTPS destination the workload's X.509-SVID is presented to. Its server
/// certificate is verified against the host's roots, by host name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MtlsDestination {
    host: String,
    port: u16,
}

impl MtlsDestination {
    /// An exact host name or address and port. No wildcards. An IPv6
    /// address may be given with or without brackets.
    pub fn new(host: &str, port: u16) -> anyhow::Result<Self> {
        let host = canonical_host(host).filter(|_| port != 0).ok_or_else(|| {
            anyhow::anyhow!("an mTLS destination is an exact host and a port, not {host:?}:{port}")
        })?;
        Ok(Self { host, port })
    }

    fn matches(&self, host: &str, port: u16) -> bool {
        self.port == port && canonical_host(host).is_some_and(|host| host == self.host)
    }
}

/// `host` as a URI names it: lowercase, without a trailing dot, and an IPv6
/// address bracketed in its shortest form. `None` if it is not one host.
fn canonical_host(host: &str) -> Option<String> {
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = bare.parse::<std::net::Ipv6Addr>() {
        return Some(format!("[{ip}]"));
    }
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty() && !host.contains(['*', '/', ':', '@', '[', ']'])).then_some(host)
}

/// What the control plane granted one workload.
#[derive(Debug)]
pub struct IdentityGrant {
    /// The namespace `identity_ref` is resolved in.
    pub namespace: Option<String>,
    /// The named `WorkloadIdentity` the workload runs as, if it names one.
    pub identity_ref: Option<String>,
    /// Digests every loaded component must have, as `sha256:<hex>` of the
    /// component's bytes. Required.
    pub approved_digests: Vec<String>,
    /// Required.
    pub scheduling_lease: SchedulingLease,
    /// Destinations the X.509-SVID is presented to. Empty requests none.
    pub mtls_destinations: Vec<MtlsDestination>,
    /// Audiences a JWT-SVID is kept for, one SVID per audience.
    pub jwt_audiences: Vec<String>,
}

/// What the host tells the issuer about the workload it asks for.
#[non_exhaustive]
pub struct IssueRequest<'a> {
    pub workload_id: &'a str,
    pub namespace: Option<&'a str>,
    pub identity_ref: Option<&'a str>,
    /// Digests of the components the host loaded, as `sha256:<hex>`.
    pub loaded_digests: &'a [String],
    pub scheduling_lease: &'a [u8],
}

impl std::fmt::Debug for IssueRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssueRequest")
            .field("workload_id", &self.workload_id)
            .field("namespace", &self.namespace)
            .field("identity_ref", &self.identity_ref)
            .field("loaded_digests", &self.loaded_digests)
            .finish_non_exhaustive()
    }
}

/// An issued X.509-SVID: leaf first, then intermediates.
pub struct X509Svid {
    pub chain: Vec<CertificateDer<'static>>,
}

/// An issued JWT-SVID. Its `aud` claim names exactly the audience asked for.
pub struct JwtSvid {
    pub token: Zeroizing<String>,
}

/// Why an issuer did not issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssuerError {
    /// The issuer decided this workload may not have the credential.
    Refused(String),
    /// The issuer could not be reached or failed.
    Unavailable(String),
}

impl std::fmt::Display for IssuerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(why) => write!(f, "issuer refused: {why}"),
            Self::Unavailable(why) => write!(f, "issuer unavailable: {why}"),
        }
    }
}

impl std::error::Error for IssuerError {}

/// Mints SVIDs for workloads this host runs.
///
/// The implementer authenticates as the host and decides, from the request
/// and the scheduling lease, which workload identifier the caller may act as.
/// Nothing in the request comes from the guest. Calls that take longer than
/// 30 seconds are abandoned and retried.
#[async_trait]
pub trait IdentityIssuer: Send + Sync + 'static {
    /// Sign `csr_der`, a PKCS#10 request for a key the host holds.
    async fn issue_x509(
        &self,
        request: &IssueRequest<'_>,
        csr_der: &[u8],
    ) -> Result<X509Svid, IssuerError>;

    /// Mint a JWT-SVID for `audience` alone.
    async fn issue_jwt(
        &self,
        request: &IssueRequest<'_>,
        audience: &str,
    ) -> Result<JwtSvid, IssuerError>;
}

/// A JWT-SVID the host renews, usable as a bearer egress credential.
#[derive(Default)]
struct JwtSvidSource {
    current: ArcSwapOption<(Zeroizing<Vec<u8>>, SystemTime)>,
    revoked: tokio_util::sync::CancellationToken,
}

impl EgressCredentialSource for JwtSvidSource {
    fn current(&self) -> Result<Zeroizing<Vec<u8>>, CredentialError> {
        match self.current.load().as_deref() {
            Some((token, expires)) if *expires > SystemTime::now() => Ok(token.clone()),
            _ => Err(CredentialError::Unavailable),
        }
    }

    fn revocation(&self) -> Option<tokio_util::sync::CancellationToken> {
        Some(self.revoked.child_token())
    }
}

/// The configuration presenting one issued X.509-SVID, and its `notAfter`.
struct Presenting {
    config: Arc<rustls::ClientConfig>,
    not_after: SystemTime,
}

/// Presents one fixed credential, so a connection's certificate and its
/// deadline always come from the same SVID.
#[derive(Debug)]
struct FixedClientCert(Arc<rustls::sign::CertifiedKey>);

impl rustls::client::ResolvesClientCert for FixedClientCert {
    fn resolve(
        &self,
        _root_hint_subjects: &[&[u8]],
        _sigschemes: &[rustls::SignatureScheme],
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(Arc::clone(&self.0))
    }

    fn has_certs(&self) -> bool {
        true
    }
}

/// One workload's identity material.
struct Entry {
    grant: Mutex<IdentityGrant>,
    loaded_digests: Mutex<Vec<String>>,
    svid: Arc<RotatingClientIdentity>,
    spiffe_id: ArcSwapOption<String>,
    /// Built from `svid` each time one is installed.
    presenting: ArcSwapOption<Presenting>,
    /// Cancelled by revoke, closing every connection that presented an SVID.
    revoked: tokio_util::sync::CancellationToken,
    jwts: HashMap<String, Arc<JwtSvidSource>>,
    /// Set by revoke. Every store of issued material holds this lock and
    /// checks it, so an issuance that finishes after revoke stores nothing.
    released: Mutex<bool>,
    /// `Some` once admitted: one task per credential.
    renewal: Mutex<Option<Vec<tokio::task::JoinHandle<()>>>>,
}

impl Entry {
    fn store<T>(&self, store: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
        let released = self.released.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(!*released, "the workload's identity was revoked");
        store()
    }

    fn released(&self) -> bool {
        *self.released.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The SVID to present now, if one is unexpired and not revoked.
    fn presentable(&self) -> Option<Arc<Presenting>> {
        if self.released() {
            return None;
        }
        self.presenting
            .load_full()
            .filter(|presenting| presenting.not_after > SystemTime::now())
    }

    fn admitted(&self) -> bool {
        self.renewal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    fn stop(&self) {
        {
            let mut released = self.released.lock().unwrap_or_else(|e| e.into_inner());
            *released = true;
            self.svid.clear();
            self.presenting.store(None);
            self.spiffe_id.store(None);
            for jwt in self.jwts.values() {
                jwt.current.store(None);
                jwt.revoked.cancel();
            }
        }
        self.revoked.cancel();
        abort(&self.renewal);
    }

    /// What to show the issuer, or `None` while the lease has expired.
    fn request_parts(&self) -> Option<RequestParts> {
        let grant = self.grant.lock().unwrap_or_else(|e| e.into_inner());
        if !grant.scheduling_lease.is_current(SystemTime::now()) {
            return None;
        }
        Some(RequestParts {
            namespace: grant.namespace.clone(),
            identity_ref: grant.identity_ref.clone(),
            digests: self
                .loaded_digests
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            lease: grant.scheduling_lease.token.clone(),
        })
    }
}

fn abort(renewal: &Mutex<Option<Vec<tokio::task::JoinHandle<()>>>>) {
    let tasks = renewal.lock().unwrap_or_else(|e| e.into_inner()).take();
    for task in tasks.into_iter().flatten() {
        task.abort();
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        abort(&self.renewal);
    }
}

struct RequestParts {
    namespace: Option<String>,
    identity_ref: Option<String>,
    digests: Vec<String>,
    lease: Zeroizing<Vec<u8>>,
}

impl RequestParts {
    fn request<'a>(&'a self, workload_id: &'a str) -> IssueRequest<'a> {
        IssueRequest {
            workload_id,
            namespace: self.namespace.as_deref(),
            identity_ref: self.identity_ref.as_deref(),
            loaded_digests: &self.digests,
            scheduling_lease: &self.lease,
        }
    }
}

struct Inner {
    issuer: Arc<dyn IdentityIssuer>,
    /// Server verification for every mTLS destination.
    host_tls: Arc<rustls::ClientConfig>,
    entries: Mutex<HashMap<String, Arc<Entry>>>,
    retry: Duration,
}

/// The identity material for every workload on this host.
///
/// Cloning is cheap and shares the store.
#[derive(Clone)]
pub struct WorkloadCredentials {
    inner: Arc<Inner>,
}

impl WorkloadCredentials {
    /// `host_tls` supplies server verification for mTLS destinations; its own
    /// client identity, if any, is never presented.
    pub fn new(issuer: Arc<dyn IdentityIssuer>, host_tls: Arc<rustls::ClientConfig>) -> Self {
        Self {
            inner: Arc::new(Inner {
                issuer,
                host_tls,
                entries: Mutex::default(),
                retry: RETRY,
            }),
        }
    }

    #[cfg(test)]
    fn with_retry(mut self, retry: Duration) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.retry = retry;
        }
        self
    }

    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Entry>>> {
        self.inner.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn entry(&self, workload_id: &str) -> Option<Arc<Entry>> {
        self.entries().get(workload_id).cloned()
    }

    /// Record what `workload_id` was granted, under its start's reservation.
    /// Only the host binds, through [`crate::host::Host::workload_start_with_identity`]
    /// or the washlet, so nothing can replace a grant between a start's bind
    /// and its admit.
    ///
    /// Refused once the workload has been admitted: a running workload's grant
    /// changes only through [`Self::renew_lease`] or a new start. Refused too
    /// without a scheduling lease or approved digests.
    pub(crate) fn bind(&self, workload_id: &str, grant: IdentityGrant) -> anyhow::Result<()> {
        anyhow::ensure!(!workload_id.is_empty(), "workload id is required");
        anyhow::ensure!(
            !grant.scheduling_lease.token.is_empty(),
            "an identity grant needs a scheduling lease"
        );
        anyhow::ensure!(
            grant.scheduling_lease.is_current(SystemTime::now()),
            "the identity grant's scheduling lease has expired"
        );
        anyhow::ensure!(
            !grant.approved_digests.is_empty(),
            "an identity grant needs the digests its components must have"
        );
        for digest in &grant.approved_digests {
            anyhow::ensure!(
                is_sha256_digest(digest),
                "approved digest {digest:?} is not sha256:<64 hex>"
            );
        }
        anyhow::ensure!(
            grant.jwt_audiences.iter().all(|a| !a.is_empty()),
            "a JWT-SVID audience cannot be empty"
        );
        let mut entries = self.entries();
        if let Some(entry) = entries.get(workload_id) {
            anyhow::ensure!(
                !entry.admitted() && !entry.released(),
                "workload {workload_id} already runs with an identity, or is stopping"
            );
        }
        let svid = RotatingClientIdentity::empty();
        let jwts = grant
            .jwt_audiences
            .iter()
            .map(|audience| (audience.clone(), Arc::default()))
            .collect();
        entries.insert(
            workload_id.to_string(),
            Arc::new(Entry {
                grant: Mutex::new(grant),
                loaded_digests: Mutex::default(),
                svid,
                spiffe_id: ArcSwapOption::empty(),
                presenting: ArcSwapOption::empty(),
                revoked: tokio_util::sync::CancellationToken::new(),
                jwts,
                released: Mutex::new(false),
                renewal: Mutex::new(None),
            }),
        );
        Ok(())
    }

    /// Whether `workload_id` has a grant.
    pub fn is_bound(&self, workload_id: &str) -> bool {
        self.entries().contains_key(workload_id)
    }

    /// Replace `workload_id`'s scheduling lease with one that expires later.
    ///
    /// Refused for an empty or already expired lease, and for one that
    /// expires before the lease it replaces, so an older lease replayed
    /// cannot shorten or roll back the current one.
    pub fn renew_lease(&self, workload_id: &str, lease: SchedulingLease) -> anyhow::Result<()> {
        anyhow::ensure!(!lease.token.is_empty(), "the scheduling lease is empty");
        anyhow::ensure!(
            lease.is_current(SystemTime::now()),
            "the scheduling lease has already expired"
        );
        let entry = self
            .entry(workload_id)
            .filter(|entry| !entry.released())
            .ok_or_else(|| {
                anyhow::anyhow!("workload {workload_id} has no identity on this host")
            })?;
        let mut grant = entry.grant.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(
            lease.expires_at >= grant.scheduling_lease.expires_at,
            "the scheduling lease expires before the one it would replace"
        );
        grant.scheduling_lease = lease;
        Ok(())
    }

    /// Check the digests of what loaded for `workload_id` and start renewing
    /// its credentials. Returns `false`, doing nothing, for a workload with no
    /// grant.
    ///
    /// `loaded` names each component with the digest of the bytes that will
    /// run. One outside the approved set refuses the start.
    pub(crate) fn admit(
        &self,
        workload_id: &str,
        loaded: &[(String, String)],
    ) -> anyhow::Result<bool> {
        // Held throughout, so a bind cannot replace the entry being admitted.
        let entries = self.entries();
        let Some(entry) = entries.get(workload_id).cloned() else {
            return Ok(false);
        };
        anyhow::ensure!(
            !entry.released(),
            "workload {workload_id}'s identity was revoked"
        );
        let wants_x509 = {
            let grant = entry.grant.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((name, digest)) = loaded
                .iter()
                .find(|(_, digest)| !grant.approved_digests.contains(digest))
            {
                warn!(
                    workload_id,
                    component = name,
                    digest,
                    "refused to start a workload whose component is not approved for its identity"
                );
                anyhow::bail!(
                    "component {name} has digest {digest}, which is not approved for this \
                     workload's identity"
                );
            }
            !grant.mtls_destinations.is_empty()
        };
        *entry
            .loaded_digests
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = loaded.iter().map(|(_, d)| d.clone()).collect();
        let credentials = wants_x509
            .then_some(Credential::X509)
            .into_iter()
            .chain(entry.jwts.keys().cloned().map(Credential::Jwt));
        let tasks: Vec<_> = credentials
            .map(|credential| {
                tokio::spawn(renew(
                    Arc::downgrade(&entry),
                    workload_id.to_string(),
                    credential,
                    Arc::downgrade(&self.inner),
                ))
            })
            .collect();
        let previous = {
            // Checked under the renewal lock, which a revoke takes after
            // marking the entry, so it either sees these tasks or they see it.
            let mut renewal = entry.renewal.lock().unwrap_or_else(|e| e.into_inner());
            if entry.released() {
                tasks.iter().for_each(tokio::task::JoinHandle::abort);
                anyhow::bail!("workload {workload_id}'s identity was revoked");
            }
            renewal.replace(tasks)
        };
        drop(entries);
        for task in previous.into_iter().flatten() {
            task.abort();
        }
        Ok(true)
    }

    /// Stop renewing `workload_id`'s credentials and drop them, including
    /// any issuance still in flight. Its mTLS destinations refuse connections
    /// from then on, until [`Self::release`].
    pub fn revoke(&self, workload_id: &str) {
        if let Some(entry) = self.entry(workload_id) {
            entry.stop();
        }
    }

    /// [`Self::revoke`], then forget `workload_id`, so it can be bound again.
    pub fn release(&self, workload_id: &str) {
        let removed = self.entries().remove(workload_id);
        if let Some(entry) = removed {
            entry.stop();
        }
    }

    /// [`Self::release`] `workload_id` if it was bound but never admitted.
    pub(crate) fn release_unadmitted(&self, workload_id: &str) {
        let mut entries = self.entries();
        if entries
            .get(workload_id)
            .is_some_and(|entry| !entry.admitted())
            && let Some(entry) = entries.remove(workload_id)
        {
            drop(entries);
            entry.stop();
        }
    }

    /// [`Self::release`] every workload, as the host stops.
    pub fn release_all(&self) {
        let removed: Vec<_> = self.entries().drain().map(|(_, entry)| entry).collect();
        for entry in removed {
            entry.stop();
        }
    }

    /// The SPIFFE ID of `workload_id`'s current X.509-SVID.
    pub fn spiffe_id(&self, workload_id: &str) -> Option<String> {
        self.entry(workload_id)?
            .spiffe_id
            .load_full()
            .map(|id| id.to_string())
    }

    /// The JWT-SVID for `audience`, as a bearer credential the host keeps
    /// current. `None` when the grant does not name that audience.
    pub fn jwt_source(
        &self,
        workload_id: &str,
        audience: &str,
    ) -> Option<Arc<dyn EgressCredentialSource>> {
        let entry = self.entry(workload_id).filter(|entry| !entry.released())?;
        let source = entry.jwts.get(audience)?.clone();
        Some(source)
    }

    /// Wrap `inner` so each workload's X.509-SVID is presented to the
    /// destinations its grant names, and to no others.
    ///
    /// While there is no unexpired SVID (before the first arrives, after one
    /// lapses, and once the identity is revoked), connections to a granted
    /// destination fail their handshake rather than go out without it, and a
    /// pooled connection is closed when the SVID it presented expires.
    pub fn tls_resolver(
        &self,
        inner: Arc<dyn ClientTlsConfigResolver>,
    ) -> Arc<dyn ClientTlsConfigResolver> {
        let mut not_issued = (*self.inner.host_tls).clone();
        let provider = Arc::clone(not_issued.crypto_provider());
        not_issued
            .dangerous()
            .set_certificate_verifier(Arc::new(NotIssued(provider)));
        not_issued.client_auth_cert_resolver = RotatingClientIdentity::empty();
        not_issued.resumption = rustls::client::Resumption::disabled();
        Arc::new(IdentityTls {
            inner,
            credentials: self.clone(),
            not_issued: Arc::new(not_issued),
        })
    }
}

struct IdentityTls {
    inner: Arc<dyn ClientTlsConfigResolver>,
    credentials: WorkloadCredentials,
    not_issued: Arc<rustls::ClientConfig>,
}

impl ClientTlsConfigResolver for IdentityTls {
    fn config_for(&self, workload_id: &str) -> Arc<rustls::ClientConfig> {
        self.inner.config_for(workload_id)
    }

    fn host_config(&self) -> Arc<rustls::ClientConfig> {
        self.inner.host_config()
    }

    fn destination_tls(&self, workload_id: &str, host: &str, port: u16) -> Option<DestinationTls> {
        let Some(entry) = self.credentials.entry(workload_id) else {
            return self.inner.destination_tls(workload_id, host, port);
        };
        let granted = entry
            .grant
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mtls_destinations
            .iter()
            .any(|destination| destination.matches(host, port));
        if !granted {
            return self.inner.destination_tls(workload_id, host, port);
        }
        let Some(presenting) = entry.presentable() else {
            return Some(DestinationTls::new(Arc::clone(&self.not_issued)));
        };
        let left = presenting
            .not_after
            .duration_since(SystemTime::now())
            .unwrap_or_default();
        let deadline = tokio::time::Instant::now() + left;
        let revoked = entry.revoked.clone().cancelled_owned();
        Some(DestinationTls {
            config: Arc::clone(&presenting.config),
            closes: Some(Box::pin(async move {
                tokio::select! {
                    () = tokio::time::sleep_until(deadline) => {}
                    () = revoked => {}
                }
            })),
        })
    }
}

/// Fails every handshake: the destination is granted, but there is no
/// unexpired SVID to present to it.
#[derive(Debug)]
struct NotIssued(Arc<rustls::crypto::CryptoProvider>);

impl NotIssued {
    fn refuse() -> rustls::Error {
        rustls::Error::General("the workload has no current X.509-SVID to present".into())
    }
}

impl ServerCertVerifier for NotIssued {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Err(Self::refuse())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(Self::refuse())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(Self::refuse())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

enum Credential {
    X509,
    Jwt(String),
}

impl std::fmt::Display for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::X509 => f.write_str("X.509-SVID"),
            Self::Jwt(audience) => write!(f, "JWT-SVID for {audience}"),
        }
    }
}

/// Keep one credential current until the entry is released or dropped.
async fn renew(
    entry: Weak<Entry>,
    workload_id: String,
    credential: Credential,
    inner: Weak<Inner>,
) {
    let Some(retry) = inner.upgrade().map(|inner| inner.retry) else {
        return;
    };
    let mut backoff = retry;
    let mut lease_warned = false;
    loop {
        let (Some(held), Some(inner)) = (entry.upgrade(), inner.upgrade()) else {
            return;
        };
        let Some(parts) = held.request_parts() else {
            if !lease_warned {
                warn!(
                    workload_id,
                    "scheduling lease expired; not asking for credentials until it is renewed"
                );
                lease_warned = true;
            }
            drop((held, inner));
            tokio::time::sleep(retry).await;
            continue;
        };
        lease_warned = false;
        let request = parts.request(&workload_id);
        let issued = SystemTime::now();
        let outcome =
            tokio::time::timeout(ISSUE_TIMEOUT, issue(&inner, &held, &request, &credential))
                .await
                .unwrap_or_else(|_| {
                    Err(anyhow::anyhow!(
                        "the issuer did not answer within {ISSUE_TIMEOUT:?}"
                    ))
                });
        drop((held, inner));
        let wait = match outcome {
            Ok(expires) => {
                backoff = retry;
                renew_in(issued, expires)
            }
            Err(err) => {
                warn!(workload_id, %credential, %err, "failed to renew the workload's credential");
                let wait = backoff.mul_f64(0.5 + fastrand::f64() / 2.0);
                backoff = (backoff * 2).min(MAX_BACKOFF);
                wait
            }
        };
        tokio::time::sleep(wait).await;
    }
}

/// How long after `issued` a credential that expires at `expires` is
/// renewed: half way through its life, within the bounds above.
fn renew_in(issued: SystemTime, expires: SystemTime) -> Duration {
    let life = expires
        .duration_since(issued)
        .unwrap_or_default()
        .min(MAX_LIFETIME);
    (life / 2).max(MIN_RENEWAL)
}

/// Issue `credential` and install it. Returns when it expires.
async fn issue(
    inner: &Inner,
    entry: &Entry,
    request: &IssueRequest<'_>,
    credential: &Credential,
) -> anyhow::Result<SystemTime> {
    match credential {
        Credential::X509 => issue_x509(inner, entry, request).await,
        Credential::Jwt(audience) => issue_jwt(inner, entry, request, audience).await,
    }
}

async fn issue_x509(
    inner: &Inner,
    entry: &Entry,
    request: &IssueRequest<'_>,
) -> anyhow::Result<SystemTime> {
    let (csr, der) = {
        use zeroize::Zeroize as _;
        let mut key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let csr = rcgen::CertificateParams::default().serialize_request(&key);
        let der = Zeroizing::new(key.serialize_der());
        key.zeroize();
        (csr?, der)
    };
    let svid = inner.issuer.issue_x509(request, csr.der()).await?;
    let spiffe_id = leaf_spiffe_id(&svid.chain)?;
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(der.as_slice()));
    entry.store(|| {
        let expires = entry.svid.install(svid.chain, &key)?;
        let certified = entry
            .svid
            .certified()
            .ok_or_else(|| anyhow::anyhow!("the installed X.509-SVID was not held"))?;
        let mut config = (*inner.host_tls).clone();
        config.client_auth_cert_resolver = Arc::new(FixedClientCert(certified));
        config.resumption = rustls::client::Resumption::disabled();
        entry.presenting.store(Some(Arc::new(Presenting {
            config: Arc::new(config),
            not_after: expires,
        })));
        debug!(
            workload_id = request.workload_id,
            spiffe_id, "installed X.509-SVID"
        );
        entry.spiffe_id.store(Some(Arc::new(spiffe_id)));
        Ok(expires)
    })
}

async fn issue_jwt(
    inner: &Inner,
    entry: &Entry,
    request: &IssueRequest<'_>,
    audience: &str,
) -> anyhow::Result<SystemTime> {
    let source = entry
        .jwts
        .get(audience)
        .ok_or_else(|| anyhow::anyhow!("audience {audience} is not granted"))?;
    let svid = inner.issuer.issue_jwt(request, audience).await?;
    let claims = jwt_claims(&svid.token)?;
    anyhow::ensure!(
        claims.aud == [audience],
        "the issued JWT-SVID must name {audience} as its only audience"
    );
    let expires = SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(claims.exp))
        .ok_or_else(|| anyhow::anyhow!("the issued JWT-SVID's exp is out of range"))?;
    anyhow::ensure!(
        expires > SystemTime::now(),
        "the issued JWT-SVID has already expired"
    );
    entry.store(|| {
        source.current.store(Some(Arc::new((
            Zeroizing::new(svid.token.as_bytes().to_vec()),
            expires,
        ))));
        Ok(expires)
    })
}

struct Claims {
    exp: u64,
    aud: Vec<String>,
}

/// The `exp` and `aud` claims of a compact JWS, unverified: the issuer is
/// trusted, and these only steer renewal and catch a misissued token.
fn jwt_claims(token: &str) -> anyhow::Result<Claims> {
    use base64::Engine as _;
    let mut parts = token.split('.');
    let (Some(_), Some(payload), Some(_), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        anyhow::bail!("the issued JWT-SVID is not a compact JWS");
    };
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload)?;
    let claims: serde_json::Value = serde_json::from_slice(&payload)?;
    let exp = claims
        .get("exp")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("the issued JWT-SVID has no exp claim"))?;
    let aud = match claims.get("aud") {
        Some(serde_json::Value::String(aud)) => vec![aud.clone()],
        Some(serde_json::Value::Array(auds)) => auds
            .iter()
            .map(|a| a.as_str().map(str::to_string))
            .collect::<Option<_>>()
            .ok_or_else(|| {
                anyhow::anyhow!("the issued JWT-SVID has an aud that is not a string")
            })?,
        _ => Vec::new(),
    };
    Ok(Claims { exp, aud })
}

/// The one `spiffe://` URI SAN of the leaf.
fn leaf_spiffe_id(chain: &[CertificateDer<'_>]) -> anyhow::Result<String> {
    let leaf = chain
        .first()
        .ok_or_else(|| anyhow::anyhow!("the issued X.509-SVID chain is empty"))?;
    let (_, cert) = x509_parser::parse_x509_certificate(leaf)
        .map_err(|err| anyhow::anyhow!("failed to parse the issued X.509-SVID: {err}"))?;
    let ids: Vec<String> = cert
        .subject_alternative_name()
        .map_err(|err| anyhow::anyhow!("failed to read the X.509-SVID's SAN: {err}"))?
        .map(|san| {
            san.value
                .general_names
                .iter()
                .filter_map(|name| match name {
                    x509_parser::extensions::GeneralName::URI(uri)
                        if uri.starts_with("spiffe://") =>
                    {
                        Some(uri.to_string())
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    match ids.as_slice() {
        [id] => Ok(id.clone()),
        _ => anyhow::bail!("an X.509-SVID carries exactly one spiffe:// URI SAN"),
    }
}

fn is_sha256_digest(digest: &str) -> bool {
    digest.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// `sha256:<hex>` of `bytes`, as the host records what it loaded and keys its
/// compiles.
pub fn sha256_digest(bytes: &[u8]) -> String {
    crate::engine::content_digest(bytes)
}

#[cfg(test)]
mod tests;
