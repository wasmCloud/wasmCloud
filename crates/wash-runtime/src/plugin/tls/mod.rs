//! TLS trust and client identity carried on a plugin's `allowedHosts` grant.
//!
//! The material is declared once, in named catalogs; a grant selects from them
//! by name, so the egress policy never carries key material:
//!
//! ```yaml
//! trustBundles:
//!   corp-ca: { ca: ./tls/ca.crt, roots: replace }
//! identities:
//!   db-client: { cert: ./tls/client.crt, key: ./tls/client.key, refresh: 30s }
//! host:
//!   plugins:
//!     - id: database
//!       allowedHosts:
//!         - host: "db.internal:5432"
//!           tls: { trust: corp-ca, identity: db-client }
//!         - "db.internal:8080"
//! ```
//!
//! The selection sits on the entry that already gates egress, so one decision
//! both authorizes a destination and says how the plugin authenticates to it
//! and verifies it; a mistyped host denies the connection instead of opening it
//! in plaintext. `trust` omitted means the platform's default roots (webpki),
//! `identity` omitted means no client certificate.
//!
//! A [`TlsCatalog`] reads every file when it loads, so a missing or malformed
//! one fails the host rather than the first connection. An identity is a
//! [`RotatingClientIdentity`]: an expired certificate is never presented, and
//! with `refresh` set a rewritten one is picked up without a restart.
//!
//! A plugin reads [`PluginTlsPolicy`] through
//! [`crate::plugin::HostPlugin::configure_tls_policy`] and hands the trust to
//! its own TLS client. A plugin that cannot refuses the declaration at load.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, ensure};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::host::allowed_hosts::AllowedHost;
use crate::host::client_identity::{RotatingClientIdentity, spawn_refresh};
use crate::host::http_client::{ClientIdentity, ClientTlsOptions, TrustRoots};

/// How a trust bundle's `ca` combines with the built-in roots.
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

/// A named set of CA certificates a grant can select as its trust.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustBundle {
    /// PEM bundle of CA certificates; may hold several.
    pub ca: PathBuf,
    /// Whether `ca` adds to the built-in roots or replaces them. Omitted means
    /// [`TlsRoots::Add`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roots: Option<TlsRoots>,
}

/// A named client identity a grant can select to present.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IdentitySource {
    /// PEM certificate chain, leaf first.
    pub cert: PathBuf,
    /// PEM private key for the leaf.
    pub key: PathBuf,
    /// How often to re-read both files, picking up a rotated credential for new
    /// connections. Omitted reads them once. A failed read keeps the running
    /// credential, and an expired one is never presented either way.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "duration_opt"
    )]
    pub refresh: Option<Duration>,
}

mod duration_opt {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(
        value: &Option<Duration>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(duration) => {
                serializer.serialize_str(&humantime::format_duration(*duration).to_string())
            }
            None => serializer.serialize_none(),
        }
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Duration>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|text| humantime::parse_duration(&text).map_err(serde::de::Error::custom))
            .transpose()
    }
}

fn resolve(path: &mut PathBuf, base: &Path) {
    if path.is_relative() {
        *path = base.join(&*path);
    }
}

impl TrustBundle {
    /// Make a relative `ca` absolute against `base`.
    pub fn resolve_relative_to(&mut self, base: &Path) {
        resolve(&mut self.ca, base);
    }
}

impl IdentitySource {
    /// Make relative paths absolute against `base`.
    pub fn resolve_relative_to(&mut self, base: &Path) {
        resolve(&mut self.cert, base);
        resolve(&mut self.key, base);
    }

    fn identity(&self) -> ClientIdentity {
        ClientIdentity::CertificatePem {
            cert_path: self.cert.clone(),
            key_path: self.key.clone(),
        }
    }
}

/// A grant's `(trust, identity)` names: what a client configuration is built from.
type Selection = (Option<String>, Option<String>);

/// The trust bundles and identities a host declared, loaded.
///
/// Grants selecting the same pair share one client configuration, and every
/// grant naming an identity shares its one [`RotatingClientIdentity`], so a
/// rotation reaches all of them. Refresh tasks stop when the catalog is
/// dropped; a [`PluginTlsPolicy`] built from it keeps it alive.
pub struct TlsCatalog {
    default_roots: Arc<rustls::RootCertStore>,
    trust: BTreeMap<String, Arc<rustls::RootCertStore>>,
    identities: BTreeMap<String, Arc<RotatingClientIdentity>>,
    configs: Mutex<BTreeMap<Selection, Arc<rustls::ClientConfig>>>,
    refresh: Vec<tokio::task::JoinHandle<()>>,
}

impl fmt::Debug for TlsCatalog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsCatalog")
            .field("trust", &self.trust.keys().collect::<Vec<_>>())
            .field("identities", &self.identities.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Drop for TlsCatalog {
    fn drop(&mut self) {
        for task in &self.refresh {
            task.abort();
        }
    }
}

impl TlsCatalog {
    /// A catalog declaring nothing: grants may still use `tls: {}`.
    pub fn empty() -> anyhow::Result<Arc<Self>> {
        Self::load(&BTreeMap::new(), &BTreeMap::new())
    }

    /// Read every bundle and identity, and start refreshing identities that
    /// ask for it.
    ///
    /// # Errors
    ///
    /// A file that is missing, unparsable, or holds nothing usable; a client
    /// certificate that is expired, not yet valid, or does not match its key;
    /// `refresh` of zero, or set outside an async runtime.
    pub fn load(
        trust: &BTreeMap<String, TrustBundle>,
        identities: &BTreeMap<String, IdentitySource>,
    ) -> anyhow::Result<Arc<Self>> {
        let default_roots = Arc::new(ClientTlsOptions::new(TrustRoots::Webpki).root_store()?);
        let trust = trust
            .iter()
            .map(|(name, bundle)| {
                let roots = match bundle.roots.unwrap_or_default() {
                    TlsRoots::Add => TrustRoots::Webpki,
                    TlsRoots::Replace => TrustRoots::ExtraOnly,
                };
                let store = ClientTlsOptions::new(roots)
                    .with_ca_paths([bundle.ca.clone()])
                    .root_store()
                    .with_context(|| format!("trust bundle '{name}' is unusable"))?;
                Ok((name.clone(), Arc::new(store)))
            })
            .collect::<anyhow::Result<_>>()?;
        let mut refresh = Vec::new();
        let mut loaded = BTreeMap::new();
        for (name, source) in identities {
            let identity = RotatingClientIdentity::load(&source.identity())
                .with_context(|| format!("identity '{name}' is unusable"))?;
            if let Some(interval) = source.refresh {
                ensure!(
                    tokio::runtime::Handle::try_current().is_ok(),
                    "identity '{name}' sets `refresh`, which needs the host's async runtime"
                );
                refresh.push(
                    spawn_refresh(Arc::clone(&identity), source.identity(), interval)
                        .with_context(|| format!("identity '{name}' has an invalid `refresh`"))?,
                );
            }
            loaded.insert(name.clone(), identity);
        }
        Ok(Arc::new(Self {
            default_roots,
            trust,
            identities: loaded,
            configs: Mutex::default(),
            refresh,
        }))
    }

    /// The client configuration for `grant`'s selection, built once per pair.
    fn config(
        &self,
        grant: &TlsGrant,
        host: &AllowedHost,
    ) -> anyhow::Result<Arc<rustls::ClientConfig>> {
        let key = (grant.trust.clone(), grant.identity.clone());
        let mut configs = self
            .configs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(config) = configs.get(&key) {
            return Ok(Arc::clone(config));
        }
        let roots = match &grant.trust {
            Some(name) => self.trust.get(name).with_context(|| {
                format!(
                    "allowedHosts entry '{host}' selects trust bundle '{name}', which \
                     `trustBundles` does not declare"
                )
            })?,
            None => &self.default_roots,
        };
        let builder = rustls::ClientConfig::builder().with_root_certificates(Arc::clone(roots));
        let config = match &grant.identity {
            Some(name) => {
                let identity = self.identities.get(name).with_context(|| {
                    format!(
                        "allowedHosts entry '{host}' selects identity '{name}', which \
                         `identities` does not declare"
                    )
                })?;
                let mut config = builder.with_client_cert_resolver(Arc::clone(identity) as _);
                // A resumed session skips the resolver, so a rotated or expired
                // credential would keep authenticating resumed connections.
                config.resumption = rustls::client::Resumption::disabled();
                config
            }
            None => builder.with_no_client_auth(),
        };
        let config = Arc::new(config);
        configs.insert(key, Arc::clone(&config));
        Ok(config)
    }
}

/// The `tls` block on one `allowedHosts` entry: which trust bundle to verify
/// the server with and which identity to present, both by catalog name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TlsGrant {
    /// A `trustBundles` entry. Omitted means the platform's default roots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust: Option<String>,
    /// An `identities` entry. Omitted presents no client certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
}

impl TlsGrant {
    /// Check the block against the entry it sits on.
    fn validate(&self, host: &AllowedHost) -> anyhow::Result<()> {
        if let Some(scheme) = host.scheme() {
            ensure!(
                !PLAINTEXT_SCHEMES
                    .iter()
                    .any(|plain| plain.eq_ignore_ascii_case(scheme)),
                "allowedHosts entry '{host}' declares `tls`, but its `{scheme}://` scheme never \
                 carries TLS; use the TLS scheme, or drop the scheme from the entry"
            );
        }
        // `*` matches every destination, and a one-label suffix such as `*.com`
        // a whole top-level domain: a private CA there could vouch for any of
        // those names, and an identity would be presented to every server that
        // asks for one.
        let names_material = self.trust.is_some() || self.identity.is_some();
        match host {
            AllowedHost::Any => ensure!(
                !names_material,
                "allowedHosts entry '*' selects a trust bundle or an identity, which would apply \
                 it to every destination; name the host, or a `*.suffix`, it is for"
            ),
            AllowedHost::SuffixWildcard { suffix, .. } => ensure!(
                !names_material || suffix.trim_matches('.').split('.').count() >= 2,
                "allowedHosts entry '{host}' selects a trust bundle or an identity, which would \
                 apply it to every host under a top-level domain; name the host, or a `*.suffix` \
                 of at least two labels, it is for"
            ),
            _ => {}
        }
        Ok(())
    }
}

/// Schemes whose protocol has no TLS form, so a `tls` block on an entry pinned
/// to one of them can never be honoured.
const PLAINTEXT_SCHEMES: &[&str] = &["http", "ws"];

/// One plugin `allowedHosts` entry: the host it grants, and optionally the TLS
/// a connection to it uses.
///
/// Deserializes from either a plain string (`"cluster.internal:8093"`) or a
/// `{ host, tls }` record, and serializes back to the same form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginAllowedHost {
    /// The egress grant, parsed exactly as any `allowedHosts` string is.
    pub host: AllowedHost,
    /// The TLS selection for a connection to `host`, when declared.
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
/// selection it was built from.
#[derive(Debug)]
pub struct TlsTrust {
    grant: TlsGrant,
    config: Arc<rustls::ClientConfig>,
}

impl TlsTrust {
    /// The client configuration to connect with: the selected roots, and the
    /// selected identity when there is one. Clones share the identity, so a
    /// rotation reaches them too.
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
    scope: Scope,
    host: AllowedHost,
    trust: Arc<TlsTrust>,
}

/// The TLS trust a plugin's `allowedHosts` declared, by endpoint.
///
/// An entry's `tls` block covers what the entry grants: its host, and the port
/// and scheme it pins, if any. `tls://db.internal:5432` selects nothing for
/// `https://db.internal:8443`, so two services behind one name can present
/// different identities. A scheme-pinned entry without a port grants the
/// scheme's default port, so `https://db.internal` covers 443 only. A lookup
/// names the protocol's schemes, and an entry pinning another never applies. A
/// TLS client that knows only the server name, such as one handed an open
/// socket, is answered only by entries that pin neither a port nor a scheme:
/// those are the ones granting the whole host.
///
/// A destination can match several entries, and the most specific wins: an
/// exact host over a `*.suffix` wildcard, a longer suffix over a shorter one,
/// either over `*`; then a pinned port; then a pinned scheme. Only entries that
/// declare `tls` take part. Two entries with the same scope and different `tls`
/// blocks are refused when the policy is built, and two that tie as a lookup's
/// most specific (`nats://` and `tls://` for one NATS server) are refused by
/// the lookup. An address literal is compared as an address, so
/// `::ffff:10.0.0.5` and `10.0.0.5` are one host.
///
/// A native plugin must connect with TLS, using [`TlsTrust::client_config`],
/// to every destination this returns trust for. Returning `None` means the
/// operator declared nothing for the destination, not that TLS is off.
#[derive(Debug, Clone)]
pub struct PluginTlsPolicy {
    entries: Arc<[TlsEntry]>,
    /// Keeps the catalog's identity refresh running while the policy is in
    /// use, and says which material the names resolved against.
    catalog: Arc<TlsCatalog>,
}

impl PluginTlsPolicy {
    /// Build the policy from a plugin's grants, resolving each selection
    /// against `catalog`.
    ///
    /// `None` when no entry declares `tls`: such a plugin declared no trust,
    /// and is not asked to honour any.
    ///
    /// # Errors
    ///
    /// A selection naming something `catalog` does not declare; `tls` on an
    /// entry pinned to a plaintext scheme; a trust bundle or identity on `*`;
    /// two entries with the same host, port and scheme and different `tls`
    /// blocks.
    pub fn from_grants(
        grants: &[PluginAllowedHost],
        catalog: &Arc<TlsCatalog>,
    ) -> anyhow::Result<Option<Self>> {
        let mut entries: Vec<TlsEntry> = Vec::new();
        for grant in grants {
            let Some(tls) = &grant.tls else {
                continue;
            };
            tls.validate(&grant.host)?;
            let scope = Scope::of(&grant.host);
            if let Some(existing) = entries.iter().find(|e| e.scope == scope) {
                ensure!(
                    existing.trust.grant == *tls,
                    "allowedHosts entries '{}' and '{}' grant the same host, port and scheme \
                     with different `tls` blocks; declare one",
                    existing.host,
                    grant.host
                );
                tracing::warn!(
                    host = %grant.host,
                    "allowedHosts grants the same endpoint with the same `tls` block twice; \
                     the duplicate is ignored"
                );
                continue;
            }
            entries.push(TlsEntry {
                scope,
                host: grant.host.clone(),
                trust: Arc::new(TlsTrust {
                    grant: tls.clone(),
                    // This plugin's own session store: two plugins selecting
                    // the same pair share its material, never its sessions.
                    config: Arc::new(crate::host::http_client::isolated_resumption(
                        &*catalog.config(tls, &grant.host)?,
                    )),
                }),
            });
        }
        if entries.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            entries: entries.into(),
            catalog: Arc::clone(catalog),
        }))
    }

    /// The trust for a connection known only by `server_name`, a host name or
    /// address: only an entry granting the whole host, with no port or scheme,
    /// applies.
    #[must_use]
    pub fn for_server_name(&self, server_name: &str) -> Option<Arc<TlsTrust>> {
        let name = canonical_name(server_name);
        // Host-wide entries matching one name at one specificity share a
        // scope, and scopes are unique, so these never tie.
        self.most_specific(|scope| scope.is_host_wide() && scope.host.matches(&name))
            .ok()
            .flatten()
    }

    /// The trust for a connection to `host` on `port` in a protocol whose URL
    /// schemes are `schemes`, such as `["nats", "tls"]` for NATS over TCP. An
    /// entry pinning any other scheme does not apply.
    ///
    /// # Errors
    ///
    /// The most specific entries that apply pin different schemes of
    /// `schemes` with different `tls` blocks, so which one applies would
    /// depend on the order they were declared in.
    pub fn for_endpoint(
        &self,
        host: &str,
        port: u16,
        schemes: &[&str],
    ) -> anyhow::Result<Option<Arc<TlsTrust>>> {
        let name = canonical_name(host);
        self.most_specific(|scope| scope.matches(&name, port, Some(schemes)))
    }

    /// Whether an entry declares `tls` for `host` on `port`, whatever scheme
    /// it pins: a plaintext connection there is one the grant says is TLS.
    #[must_use]
    pub fn declares_tls_at(&self, host: &str, port: u16) -> bool {
        let name = canonical_name(host);
        self.entries
            .iter()
            .any(|entry| entry.scope.matches(&name, port, None))
    }

    fn most_specific(
        &self,
        applies: impl Fn(&Scope) -> bool,
    ) -> anyhow::Result<Option<Arc<TlsTrust>>> {
        let matching = || self.entries.iter().filter(|entry| applies(&entry.scope));
        let Some(top) = matching().map(|entry| entry.scope.specificity()).max() else {
            return Ok(None);
        };
        let mut winners = matching().filter(|entry| entry.scope.specificity() == top);
        let Some(first) = winners.next() else {
            return Ok(None);
        };
        if let Some(other) = winners.find(|entry| entry.trust.grant != first.trust.grant) {
            anyhow::bail!(
                "allowedHosts entries '{}' and '{}' both grant this endpoint with different \
                 `tls` blocks; declare one",
                first.host,
                other.host
            );
        }
        Ok(Some(Arc::clone(&first.trust)))
    }

    /// Whether two policies carry the same declaration: the same selection for
    /// every scope, whatever order the entries were written in, resolved
    /// against the same loaded catalog. Entries are unique per scope, so
    /// matching each by scope is exact.
    #[must_use]
    pub fn same_declaration(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.catalog, &other.catalog)
            && self.entries.len() == other.entries.len()
            && self.entries.iter().all(|a| {
                other
                    .entries
                    .iter()
                    .any(|b| a.scope == b.scope && a.trust.grant == b.trust.grant)
            })
    }
}

impl PartialEq for PluginTlsPolicy {
    fn eq(&self, other: &Self) -> bool {
        self.same_declaration(other)
    }
}

impl Eq for PluginTlsPolicy {}

/// What an entry grants: its host, and the port and scheme it pins.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Scope {
    host: HostKey,
    port: Option<u16>,
    /// Lowercased.
    scheme: Option<String>,
}

impl Scope {
    /// A scheme-pinned entry that writes no port means the scheme's default
    /// port where it has one: URL parsing drops a written default port, so
    /// `https://x:443` and `https://x` are one entry, and it grants port 443.
    fn of(host: &AllowedHost) -> Self {
        let (key, port, scheme) = match host {
            AllowedHost::Any => (HostKey::Any, None, None),
            AllowedHost::SuffixWildcard { suffix, port, .. } => {
                let scheme = host.scheme();
                (
                    HostKey::Suffix(canonical_name(suffix)),
                    port.or_else(|| scheme.and_then(default_port)),
                    scheme,
                )
            }
            AllowedHost::Authority(authority) => (
                HostKey::Name(canonical_name(authority.host())),
                authority.port_u16(),
                None,
            ),
            AllowedHost::Url(url) => (
                HostKey::Name(canonical_name(url.host_str().unwrap_or_default())),
                url.port_or_known_default(),
                Some(url.scheme()),
            ),
        };
        Self {
            host: key,
            port,
            scheme: scheme.map(str::to_ascii_lowercase),
        }
    }

    fn is_host_wide(&self) -> bool {
        self.port.is_none() && self.scheme.is_none()
    }

    /// Whether this covers `name` (in [`canonical_name`] form) on `port`, for
    /// a protocol spelled with one of `schemes`; `None` compares no scheme.
    fn matches(&self, name: &str, port: u16, schemes: Option<&[&str]>) -> bool {
        let scheme_matches = match (&self.scheme, schemes) {
            (Some(own), Some(schemes)) => schemes.iter().any(|s| own.eq_ignore_ascii_case(s)),
            _ => true,
        };
        self.host.matches(name) && self.port.is_none_or(|own| own == port) && scheme_matches
    }

    /// Greater is narrower.
    fn specificity(&self) -> ((u8, usize), bool, bool) {
        (
            self.host.specificity(),
            self.port.is_some(),
            self.scheme.is_some(),
        )
    }
}

/// The host an entry names.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HostKey {
    Any,
    /// Lowercased, with its leading dot.
    Suffix(String),
    /// Per [`canonical_name`].
    Name(String),
}

impl HostKey {
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

/// The port `scheme` defaults to, where URLs give it one.
fn default_port(scheme: &str) -> Option<u16> {
    url::Url::parse(&format!("{scheme}://host"))
        .ok()?
        .port_or_known_default()
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
