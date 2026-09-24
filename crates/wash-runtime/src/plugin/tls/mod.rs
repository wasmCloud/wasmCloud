//! TLS trust carried on a plugin's `allowedHosts` grant.
//!
//! An `allowedHosts` entry for a plugin is either the plain string every
//! allowlist accepts, or a record that adds the trust a connection to that host
//! uses:
//!
//! ```yaml
//! allowedHosts:
//!   - host: "cluster.internal:11207"
//!     tls:
//!       ca: ./tls/ca.crt
//!       roots: add
//!       clientCert: ./tls/client.crt
//!       clientKey: ./tls/client.key
//!   - "cluster.internal:8093"
//! ```
//!
//! The files are loaded when the policy is built. HTTPS requests, native
//! adapters, and explicit TLS calls use this trust. A component with raw
//! sockets can still send plaintext unless a grant sets `required: true`.
//!
//! Required TLS disables every raw socket and plaintext HTTP request for the
//! component plugin, including overlapping grants. Its HTTPS requests and
//! `wasmcloud:tls/dialer` connections require matching TLS trust. The host
//! owns these transports and never exposes their raw sockets to the guest.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, bail, ensure};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::host::allowed_hosts::AllowedHost;
use crate::host::http_client::{ClientIdentity, ClientTlsOptions, TrustRoots};

#[cfg(all(feature = "host-component-plugins", feature = "oci"))]
pub(crate) mod component;

/// How a grant's `ca` combines with the built-in roots.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TlsRoots {
    /// Trust `ca` in addition to the host's built-in webpki roots, so a private
    /// CA can sit beside a public endpoint.
    #[default]
    Add,
    /// Trust `ca` and nothing else.
    Replace,
}

/// The `tls` block on one `allowedHosts` entry, as written.
///
/// Paths resolve against the process's working directory unless made absolute
/// first with [`TlsGrant::resolve_relative_to`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TlsGrant {
    /// Require host-owned TLS transport. Disables raw sockets and plaintext
    /// HTTP for the entire component plugin, including overlapping grants.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
    /// PEM bundle of CA certificates to trust; may hold several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca: Option<PathBuf>,
    /// Whether `ca` adds to the built-in roots or replaces them. Omitted means
    /// [`TlsRoots::Add`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roots: Option<TlsRoots>,
    /// PEM certificate chain, leaf first, to present when the server asks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<PathBuf>,
    /// PEM private key for `client_cert`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_key: Option<PathBuf>,
}

impl TlsGrant {
    /// Make every relative path absolute against `base`, the directory the
    /// declaration was read from.
    pub fn resolve_relative_to(&mut self, base: &Path) {
        for path in [&mut self.ca, &mut self.client_cert, &mut self.client_key]
            .into_iter()
            .flatten()
        {
            if path.is_relative() {
                *path = base.join(&*path);
            }
        }
    }

    /// Check the block against the entry it sits on and read its files.
    ///
    /// # Errors
    ///
    /// A client certificate without its key or the reverse, `roots: replace`
    /// with no `ca`, an entry pinned to a scheme that never carries TLS, or a
    /// file that is missing, unparsable, or holds nothing usable.
    pub fn load(&self, host: &AllowedHost) -> anyhow::Result<TlsTrust> {
        self.validate_scheme(host)?;
        let client_identity = match (&self.client_cert, &self.client_key) {
            (Some(cert_path), Some(key_path)) => Some(ClientIdentity::CertificatePem {
                cert_path: cert_path.clone(),
                key_path: key_path.clone(),
            }),
            (None, None) => None,
            (Some(_), None) => {
                bail!("allowedHosts entry '{host}' sets `tls.clientCert` without `tls.clientKey`")
            }
            (None, Some(_)) => {
                bail!("allowedHosts entry '{host}' sets `tls.clientKey` without `tls.clientCert`")
            }
        };
        let roots = match self.roots.unwrap_or_default() {
            TlsRoots::Add => TrustRoots::Webpki,
            TlsRoots::Replace => {
                ensure!(
                    self.ca.is_some(),
                    "allowedHosts entry '{host}' sets `tls.roots: replace` without a `tls.ca` \
                     to replace the built-in roots with"
                );
                TrustRoots::ExtraOnly
            }
        };
        let mut options = ClientTlsOptions::new(roots).with_ca_paths(self.ca.clone());
        if let Some(identity) = client_identity {
            options = options.with_client_identity(identity);
        }
        let config = options
            .build()
            .with_context(|| format!("allowedHosts entry '{host}' has an unusable `tls` block"))?;
        Ok(TlsTrust {
            grant: self.clone(),
            config,
        })
    }

    fn validate_scheme(&self, host: &AllowedHost) -> anyhow::Result<()> {
        if let Some(scheme) = host.scheme() {
            ensure!(
                !PLAINTEXT_SCHEMES
                    .iter()
                    .any(|plain| plain.eq_ignore_ascii_case(scheme)),
                "allowedHosts entry '{host}' declares `tls`, but its `{scheme}://` scheme never \
                 carries TLS; use the TLS scheme, or drop the scheme from the entry"
            );
        }
        Ok(())
    }
}

/// Schemes whose protocol has no TLS form, so a `tls` block on an entry pinned
/// to one of them can never be honoured.
const PLAINTEXT_SCHEMES: &[&str] = &["http", "ws"];

/// One plugin `allowedHosts` entry: the host it grants, and optionally the TLS
/// trust a connection to it uses.
///
/// Deserializes from either a plain string (`"cluster.internal:8093"`) or a
/// `{ host, tls }` record, and serializes back to the same form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginAllowedHost {
    /// The egress grant, parsed exactly as any `allowedHosts` string is.
    pub host: AllowedHost,
    /// The trust for a TLS connection to `host`, when declared.
    pub tls: Option<TlsGrant>,
}

impl From<AllowedHost> for PluginAllowedHost {
    fn from(host: AllowedHost) -> Self {
        Self { host, tls: None }
    }
}

impl fmt::Display for PluginAllowedHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.host)
    }
}

impl Serialize for PluginAllowedHost {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Record<'a> {
            host: &'a AllowedHost,
            tls: &'a TlsGrant,
        }
        match &self.tls {
            None => self.host.serialize(serializer),
            Some(tls) => Record {
                host: &self.host,
                tls,
            }
            .serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for PluginAllowedHost {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Record {
            host: AllowedHost,
            #[serde(default)]
            tls: Option<TlsGrant>,
        }

        struct EntryVisitor;
        impl<'de> Visitor<'de> for EntryVisitor {
            type Value = PluginAllowedHost;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an allowed-host string, or a `{ host, tls }` record")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                value
                    .parse::<AllowedHost>()
                    .map(PluginAllowedHost::from)
                    .map_err(|err| E::custom(format!("'{value}': {err:#}")))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                let Record { host, tls } =
                    Record::deserialize(de::value::MapAccessDeserializer::new(map))?;
                Ok(PluginAllowedHost { host, tls })
            }
        }

        deserializer.deserialize_any(EntryVisitor)
    }
}

/// The trust one grant resolved to: a ready TLS client configuration, and the
/// declaration it was built from.
#[derive(Debug)]
pub struct TlsTrust {
    grant: TlsGrant,
    config: Arc<rustls::ClientConfig>,
}

impl TlsTrust {
    /// The client configuration to connect with: the declared roots, and the
    /// client certificate when one was declared.
    #[must_use]
    pub fn client_config(&self) -> Arc<rustls::ClientConfig> {
        Arc::clone(&self.config)
    }

    /// The `tls` block this was built from.
    #[must_use]
    pub fn grant(&self) -> &TlsGrant {
        &self.grant
    }
}

#[derive(Debug)]
struct TlsEntry {
    key: HostKey,
    host: AllowedHost,
    trust: Arc<TlsTrust>,
}

/// The TLS trust a plugin's `allowedHosts` declared, by host name.
///
/// Trust belongs to a host, not a port: an entry's port and scheme still gate
/// egress, but its `tls` block covers every connection to the host it names.
/// A TLS client often knows only the name — `wasmcloud:tls` is handed one over
/// a socket that is already open — so trust that varied by port could not be
/// chosen correctly there. Two entries naming the same host with different
/// `tls` blocks are therefore refused when the policy is built.
///
/// A destination can still match several entries (`*` and `cluster.internal`,
/// say). Only entries that declare `tls` take part, and the most specific wins:
/// an exact host over a `*.suffix` wildcard, a longer suffix over a shorter
/// one, either over `*`. An address literal is compared as an address, so
/// `::ffff:10.0.0.5` and `10.0.0.5` are one host.
///
/// A native plugin must connect with TLS, using [`TlsTrust::client_config`],
/// to every destination this returns trust for. Returning `None` means the
/// operator declared nothing for the destination, not that TLS is off.
#[derive(Debug, Clone)]
pub struct PluginTlsPolicy {
    entries: Arc<[TlsEntry]>,
}

impl PluginTlsPolicy {
    /// Whether this component plugin must use host-owned TLS transports.
    #[must_use]
    pub fn requires_tls(&self) -> bool {
        self.entries.iter().any(|entry| entry.trust.grant.required)
    }

    /// Build the policy from a plugin's grants, reading every declared file.
    ///
    /// `None` when no entry declares `tls`: such a plugin declared no trust,
    /// and is not asked to honour any.
    ///
    /// # Errors
    ///
    /// Any error [`TlsGrant::load`] reports, and two entries naming the same
    /// host, on any port or scheme, with different `tls` blocks.
    pub fn from_grants(grants: &[PluginAllowedHost]) -> anyhow::Result<Option<Self>> {
        let mut entries: Vec<TlsEntry> = Vec::new();
        for grant in grants {
            let Some(tls) = &grant.tls else {
                continue;
            };
            let key = HostKey::of(&grant.host);
            if let Some(existing) = entries.iter().find(|e| e.key == key) {
                tls.validate_scheme(&grant.host)?;
                ensure!(
                    existing.trust.grant == *tls,
                    "allowedHosts entries '{}' and '{}' name the same host with different \
                     `tls` blocks; trust covers every port of a host, so declare one",
                    existing.host,
                    grant.host
                );
                if existing.host == grant.host {
                    tracing::warn!(
                        host = %grant.host,
                        "allowedHosts lists the same host with the same `tls` block twice; \
                         the duplicate is ignored"
                    );
                }
                continue;
            }
            entries.push(TlsEntry {
                key,
                host: grant.host.clone(),
                trust: Arc::new(tls.load(&grant.host)?),
            });
        }
        if entries.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            entries: entries.into(),
        }))
    }

    /// The trust for a connection to `server_name`, a host name or address.
    #[must_use]
    pub fn for_server_name(&self, server_name: &str) -> Option<Arc<TlsTrust>> {
        let name = canonical_name(server_name);
        self.entries
            .iter()
            .filter(|entry| entry.key.matches(&name))
            .max_by_key(|entry| entry.key.specificity())
            .map(|entry| Arc::clone(&entry.trust))
    }

    /// The trust for a connection to the URL `target`, by its host.
    ///
    /// # Errors
    ///
    /// `target` is not a URL with a host.
    pub fn for_url(&self, target: &str) -> anyhow::Result<Option<Arc<TlsTrust>>> {
        let url = url::Url::parse(target)
            .with_context(|| format!("plugin endpoint {target:?} is not a valid URL"))?;
        let host = url
            .host_str()
            .with_context(|| format!("plugin endpoint {target:?} has no host"))?;
        Ok(self.for_server_name(host))
    }

    /// Whether two policies carry the same declaration.
    #[must_use]
    pub fn same_declaration(&self, other: &Self) -> bool {
        self.entries.len() == other.entries.len()
            && self
                .entries
                .iter()
                .zip(other.entries.iter())
                .all(|(a, b)| a.host == b.host && a.trust.grant == b.trust.grant)
    }
}

impl PartialEq for PluginTlsPolicy {
    fn eq(&self, other: &Self) -> bool {
        self.same_declaration(other)
    }
}

impl Eq for PluginTlsPolicy {}

/// The host an entry names, whatever its port or scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HostKey {
    Any,
    /// Lowercased, with its leading dot.
    Suffix(String),
    /// Per [`canonical_name`].
    Name(String),
}

impl HostKey {
    fn of(host: &AllowedHost) -> Self {
        match host {
            AllowedHost::Any => Self::Any,
            AllowedHost::SuffixWildcard { suffix, .. } => Self::Suffix(canonical_name(suffix)),
            AllowedHost::Authority(authority) => Self::Name(canonical_name(authority.host())),
            AllowedHost::Url(url) => Self::Name(canonical_name(url.host_str().unwrap_or_default())),
        }
    }

    /// Whether this names `name`, already in [`canonical_name`] form.
    fn matches(&self, name: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Suffix(suffix) => name
                .strip_suffix(suffix.as_str())
                .is_some_and(|prefix| !prefix.is_empty()),
            Self::Name(own) => own == name,
        }
    }

    /// Greater is narrower: an exact name, then a longer suffix, then `*`.
    fn specificity(&self) -> (u8, usize) {
        match self {
            Self::Any => (0, 0),
            Self::Suffix(suffix) => (1, suffix.len()),
            Self::Name(_) => (2, 0),
        }
    }
}

/// A host name in the one form entries and destinations are compared in:
/// lowercased, without a trailing dot, and an address literal unbracketed and
/// canonical, so an IPv4-mapped IPv6 address is the IPv4 address it maps.
fn canonical_name(name: &str) -> String {
    let name = name.strip_prefix('[').unwrap_or(name);
    let name = name.strip_suffix(']').unwrap_or(name);
    let name = name.trim_end_matches('.');
    match name.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.to_canonical().to_string(),
        Err(_) => name.to_ascii_lowercase(),
    }
}

/// Network permissions for a host-owned plugin connection.
#[cfg_attr(
    not(all(feature = "host-component-plugins", feature = "oci")),
    allow(dead_code)
)]
pub(crate) struct PluginNetwork {
    pub sockets: Arc<crate::sockets::policy::SocketPolicy>,
    pub names: Arc<[crate::host::allowed_ip_name::AllowedIpName]>,
}

#[cfg(all(feature = "host-component-plugins", feature = "oci"))]
impl PluginNetwork {
    fn check_address(&self, addr: std::net::SocketAddr) -> anyhow::Result<()> {
        use crate::host::declared_port::Protocol;
        let ip = addr.ip().to_canonical();
        ensure!(
            !crate::sockets::internal_names::is_host_sentinel(ip),
            "TLS dialer requires a real endpoint"
        );
        if ip.is_loopback() {
            ensure!(
                self.sockets.host_loopback_enabled,
                "host-loopback access is disabled"
            );
            ensure!(
                crate::host::allowed_loopback::check_allowed_loopback(
                    &self.sockets.host_loopback,
                    addr,
                    Protocol::Tcp
                ),
                "TLS endpoint is not permitted by allowedHostLoopbackPorts"
            );
            ensure!(
                !self
                    .sockets
                    .host_owned_ports
                    .as_ref()
                    .is_some_and(|ports| ports
                        .is_published(Protocol::Tcp, std::net::SocketAddr::new(ip, addr.port()))),
                "TLS endpoint reaches a host-owned port"
            );
        } else {
            ensure!(
                self.sockets.egress_addrs.permits(ip),
                "TLS endpoint is in a denied address range"
            );
        }
        Ok(())
    }

    pub(super) async fn dial(
        &self,
        policy: &PluginTlsPolicy,
        endpoint: &str,
    ) -> anyhow::Result<(
        tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
        Option<crate::host::quota::ConnectionSlot>,
    )> {
        let url = url::Url::parse(endpoint)?;
        ensure!(
            url.scheme() == "tls"
                && url.username().is_empty()
                && url.password().is_none()
                && matches!(url.path(), "" | "/")
                && url.query().is_none()
                && url.fragment().is_none(),
            "TLS endpoint must be tls://host:port"
        );
        let host = url.host_str().context("TLS endpoint has no host")?;
        let port = url.port().context("TLS endpoint requires a port")?;
        let uri: http::Uri = endpoint.parse()?;
        ensure!(
            self.sockets
                .allowed_hosts
                .iter()
                .any(|grant| grant.matches(&uri)),
            "TLS endpoint is not permitted by allowedHosts"
        );
        let host = canonical_name(host);
        if host.parse::<std::net::IpAddr>().is_err() {
            ensure!(
                crate::host::allowed_ip_name::check_allowed_ip_name(
                    &self.names,
                    &url::Host::parse(&host)?
                ),
                "TLS endpoint is not permitted by allowedIpNameLookups"
            );
        }
        let trust = policy
            .for_server_name(&host)
            .context("TLS endpoint has no declared trust")?;
        let name = rustls::pki_types::ServerName::try_from(host.clone())?;
        let permit = self
            .sockets
            .quota
            .as_ref()
            .map(|quota| {
                quota
                    .try_acquire_outbound_socket()
                    .context("TLS connection quota exhausted")
            })
            .transpose()?;
        let connect = async {
            let addresses = tokio::net::lookup_host((host.as_str(), port)).await?;
            let mut last_error = anyhow::anyhow!("TLS endpoint resolved to no addresses");
            for addr in addresses {
                self.check_address(addr)?;
                match tokio::net::TcpStream::connect(addr).await {
                    Ok(socket) => {
                        let connector = tokio_rustls::TlsConnector::from(Arc::new(
                            crate::host::http_client::isolated_resumption(&trust.client_config()),
                        ));
                        return Ok(connector.connect(name, socket).await?);
                    }
                    Err(err) => last_error = err.into(),
                }
            }
            Err(last_error)
        };
        let stream = tokio::time::timeout(std::time::Duration::from_secs(30), connect)
            .await
            .context("TLS connection timed out")??;
        Ok((stream, permit))
    }
}

#[cfg(test)]
mod tests;
