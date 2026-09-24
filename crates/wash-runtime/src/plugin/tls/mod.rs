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
//! The trust sits on the entry that already gates egress, so a mistyped host
//! denies the connection instead of opening it in plaintext. The PEM files are
//! read once, when the policy is built, so a missing or malformed file fails
//! the host rather than the first connection.
//!
//! A plugin reads [`PluginTlsPolicy`] through
//! [`crate::plugin::HostPlugin::configure_tls_policy`] and hands the trust to
//! its own TLS client. A plugin that cannot refuses the declaration at load.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, bail, ensure};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::host::allowed_hosts::AllowedHost;
use crate::host::http_client::{ClientIdentity, ClientTlsOptions, TrustRoots};

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
/// A TLS client often knows only the server name — one handed a socket that is
/// already open never sees the port — so trust that varied by port could not
/// be chosen correctly there. Two entries naming the same host with different
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

#[cfg(test)]
mod tests;
