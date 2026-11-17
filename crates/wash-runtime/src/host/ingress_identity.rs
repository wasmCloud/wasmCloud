//! Workload identity on HTTPS ingress.
//!
//! For a host name a workload's identity grant lists in `ingress_hosts`, the
//! ingress presents that workload's X.509-SVID, picked by SNI, and asks the
//! client for one of its own. A client certificate that is presented must be
//! an X.509-SVID that verifies against the host's trust bundles, or the
//! handshake fails; a client may present none. A request without one may
//! instead carry a JWT-SVID as a bearer token whose audience is the routed
//! workload's own SPIFFE ID.
//!
//! The verified caller reaches the guest in [`PEER_ID_HEADER`] and
//! [`PEER_SVID_HEADER`], which the ingress strips from every inbound request
//! first, and local dispatch from every co-located caller's. They
//! authenticate the caller and authorize nothing: what a peer may do is the
//! workload's decision.
//!
//! A client certificate is verified again on every request, against the
//! bundles as they are then, so a connection outliving its peer's bundle or
//! certificate stops carrying the peer.

use std::sync::{Arc, OnceLock};

use hyper::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use tokio::net::TcpStream;
use tokio_rustls::server::TlsStream;
use tracing::debug;

use crate::host::trust_bundle::{SpiffeId, SvidUsage};
use crate::host::workload_identity::WorkloadCredentials;

/// The verified caller's SPIFFE ID.
pub const PEER_ID_HEADER: &str = "wasmcloud-peer-spiffe-id";
/// How the caller proved it: `x509` for a client certificate, `jwt` for a
/// bearer JWT-SVID, which anyone holding the token can replay to this
/// audience until it expires.
pub const PEER_SVID_HEADER: &str = "wasmcloud-peer-svid";

/// Set once the host is built with workload credentials.
pub(crate) type IdentitySlot = Arc<OnceLock<WorkloadCredentials>>;

/// What one TLS connection established about identity.
pub(crate) struct IngressConnection {
    credentials: WorkloadCredentials,
    /// The SPIFFE ID whose X.509-SVID this connection was served with.
    served_as: Option<String>,
    /// The client's X.509-SVID chain, taken only from a handshake the
    /// bundles verified.
    peer: Option<Vec<CertificateDer<'static>>>,
}

#[derive(Debug)]
struct FixedCert(Arc<CertifiedKey>);

impl ResolvesServerCert for FixedCert {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(Arc::clone(&self.0))
    }
}

/// The configuration for a workload's SVID: its certificate, SPIFFE client
/// authentication, and no session resumption, so every connection is
/// verified against the bundles as they are now.
fn served_config(
    default: &ServerConfig,
    credentials: &WorkloadCredentials,
    key: Arc<CertifiedKey>,
) -> Arc<ServerConfig> {
    let mut config = ServerConfig::builder()
        .with_client_cert_verifier(credentials.trust_bundles().client_verifier())
        .with_cert_resolver(Arc::new(FixedCert(key)));
    config.alpn_protocols = default.alpn_protocols.clone();
    config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    config.send_tls13_tickets = 0;
    Arc::new(config)
}

/// Complete a TLS handshake, serving a workload's SVID when the client's SNI
/// names one of its ingress hosts and `default` otherwise.
pub(crate) async fn accept(
    default: &tokio_rustls::TlsAcceptor,
    identity: &IdentitySlot,
    stream: TcpStream,
) -> std::io::Result<(TlsStream<TcpStream>, Option<Arc<IngressConnection>>)> {
    let Some(credentials) = identity.get() else {
        return default.accept(stream).await.map(|tls| (tls, None));
    };
    let start =
        tokio_rustls::LazyConfigAcceptor::new(rustls::server::Acceptor::default(), stream).await?;
    let served = start
        .client_hello()
        .server_name()
        .and_then(|name| credentials.ingress_svid(name));
    let (config, served_as) = match served {
        Some((id, key)) => (served_config(default.config(), credentials, key), Some(id)),
        None => (Arc::clone(default.config()), None),
    };
    let tls = start.into_stream(config).await?;
    let peer = served_as
        .is_some()
        .then(|| tls.get_ref().1.peer_certificates())
        .flatten()
        .filter(|chain| !chain.is_empty())
        .map(|chain| chain.iter().map(|c| c.clone().into_owned()).collect());
    let connection = IngressConnection {
        credentials: credentials.clone(),
        served_as,
        peer,
    };
    Ok((tls, Some(Arc::new(connection))))
}

/// Remove the peer headers a client sent, so only the host ever sets them.
pub(crate) fn strip(headers: &mut HeaderMap) {
    headers.remove(PEER_ID_HEADER);
    headers.remove(PEER_SVID_HEADER);
}

/// Pass the verified caller to `workload_id`. Refuses, with 421, a request
/// that routes to a workload with another SPIFFE ID than the one served on
/// the connection, since the client authenticated that one.
pub(crate) fn admit(
    connection: &IngressConnection,
    workload_id: &str,
    headers: &mut HeaderMap,
) -> Result<(), u16> {
    if let Some(served) = connection.served_as.as_deref()
        && connection.credentials.spiffe_id(workload_id).as_deref() != Some(served)
    {
        return Err(421);
    }
    let verified = x509_peer(connection, workload_id)
        .map(|id| (id, "x509"))
        .or_else(|| jwt_peer(connection, workload_id, headers).map(|id| (id, "jwt")));
    if let Some((id, svid)) = verified
        && let Ok(value) = HeaderValue::from_str(id.as_str())
    {
        headers.insert(PEER_ID_HEADER, value);
        headers.insert(PEER_SVID_HEADER, HeaderValue::from_static(svid));
    }
    Ok(())
}

fn x509_peer(connection: &IngressConnection, workload_id: &str) -> Option<SpiffeId> {
    let chain = connection.peer.as_ref()?;
    match connection.credentials.trust_bundles().verify_x509_svid(
        chain,
        SvidUsage::Client,
        UnixTime::now(),
    ) {
        Ok(id) => Some(id),
        Err(err) => {
            debug!(workload_id, %err, "a client X.509-SVID no longer verifies");
            None
        }
    }
}

fn jwt_peer(
    connection: &IngressConnection,
    workload_id: &str,
    headers: &HeaderMap,
) -> Option<SpiffeId> {
    let token = headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .split_once(' ')
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))?
        .1
        .trim();
    if token.bytes().filter(|b| *b == b'.').count() != 2 {
        return None;
    }
    let audience = connection.credentials.spiffe_id(workload_id)?;
    match connection
        .credentials
        .trust_bundles()
        .verify_jwt_svid(token, &audience)
    {
        Ok(claims) => Some(claims.spiffe_id),
        Err(err) => {
            debug!(workload_id, %err, "a bearer token did not verify as a JWT-SVID");
            None
        }
    }
}
