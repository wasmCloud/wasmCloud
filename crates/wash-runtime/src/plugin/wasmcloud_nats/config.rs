//! Per-workload NATS configuration, parsed from a bound interface's config map.
//!
//! The host merges `config` -> `configFrom` -> `secretFrom` (later wins) before
//! a plugin sees this map, so credentials arrive here already resolved and are
//! never read from a manifest by this module.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context as _, bail};
use zeroize::Zeroizing;

/// Default per-consumer in-flight limit.
const DEFAULT_MAX_IN_FLIGHT: usize = 64;
/// Default subscription channel capacity.
const DEFAULT_SUBSCRIPTION_CAPACITY: usize = 1024;

/// How a workload authenticates to NATS.
///
/// Seeds are held in `Zeroizing` so they are wiped when the config is dropped.
/// The signing callback built from `JwtNkey` stays host-side; the seed never
/// crosses the sandbox boundary.
pub enum NatsAuth {
    Anonymous,
    CredsFile(PathBuf),
    JwtNkey {
        jwt: String,
        seed: Zeroizing<String>,
    },
    NkeySeed(Zeroizing<String>),
    UserPassword {
        username: String,
        password: Zeroizing<String>,
    },
    Token(Zeroizing<String>),
}

impl NatsAuth {
    /// Short label for logs. Never includes credential material.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Anonymous => "anonymous",
            Self::CredsFile(_) => "creds-file",
            Self::JwtNkey { .. } => "jwt+nkey",
            Self::NkeySeed(_) => "nkey",
            Self::UserPassword { .. } => "user-password",
            Self::Token(_) => "token",
        }
    }
}

impl std::fmt::Debug for NatsAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NatsAuth({})", self.kind())
    }
}

/// TLS material for the connection.
#[derive(Debug, Default, Clone)]
pub struct TlsConfig {
    pub ca: Option<PathBuf>,
    pub cert: Option<PathBuf>,
    pub key: Option<PathBuf>,
    pub first: bool,
}

/// What a workload is permitted to reach. Empty means deny-all.
#[derive(Debug, Default, Clone)]
pub struct PolicySpec {
    pub subject_allow: Vec<String>,
    pub stream_allow: Vec<String>,
    pub bucket_allow: Vec<String>,
}

/// Backpressure and concurrency bounds, all per-workload.
#[derive(Debug, Clone)]
pub struct Limits {
    pub max_in_flight: usize,
    pub subscription_capacity: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_in_flight: DEFAULT_MAX_IN_FLIGHT,
            subscription_capacity: DEFAULT_SUBSCRIPTION_CAPACITY,
        }
    }
}

/// Who acknowledges a JetStream delivery.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AckMode {
    /// Host acks on `ok`, naks on `error` or trap.
    #[default]
    Auto,
    /// Host acks nothing; the guest drives the handle.
    Manual,
}

/// A workload's complete NATS configuration.
#[derive(Debug)]
pub struct NatsConfig {
    pub servers: Vec<String>,
    pub name: Option<String>,
    pub jetstream_domain: Option<String>,
    pub inbox_prefix: Option<String>,
    pub auth: NatsAuth,
    pub tls: TlsConfig,
    pub policy: PolicySpec,
    pub limits: Limits,
    pub ack_mode: AckMode,
    pub request_timeout: Option<Duration>,
}

/// Reads a key in kebab-case, falling back to snake_case.
fn get<'a>(cfg: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    cfg.get(key)
        .or_else(|| cfg.get(&key.replace('-', "_")))
        .map(String::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

/// Splits a comma-separated list, dropping empties.
fn list(cfg: &HashMap<String, String>, key: &str) -> Vec<String> {
    get(cfg, key)
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_usize(cfg: &HashMap<String, String>, key: &str, default: usize) -> anyhow::Result<usize> {
    match get(cfg, key) {
        Some(raw) => raw
            .parse()
            .with_context(|| format!("`{key}` must be a positive integer, got `{raw}`")),
        None => Ok(default),
    }
}

fn parse_bool(cfg: &HashMap<String, String>, key: &str) -> anyhow::Result<bool> {
    match get(cfg, key) {
        Some(raw) => raw
            .parse()
            .with_context(|| format!("`{key}` must be `true` or `false`, got `{raw}`")),
        None => Ok(false),
    }
}

/// Selects exactly one auth mechanism, or fails.
///
/// Silently preferring one when several are set hides a misconfiguration until
/// the connection is refused, so a conflict is an error at bind time.
fn parse_auth(cfg: &HashMap<String, String>) -> anyhow::Result<NatsAuth> {
    let creds = get(cfg, "creds").or_else(|| get(cfg, "creds-file"));
    let jwt = get(cfg, "jwt");
    let seed = get(cfg, "nkey-seed").or_else(|| get(cfg, "nkey"));
    let username = get(cfg, "username").or_else(|| get(cfg, "user"));
    let password = get(cfg, "password");
    let token = get(cfg, "token");

    let mut selected: Vec<&str> = Vec::new();
    if creds.is_some() {
        selected.push("creds");
    }
    if jwt.is_some() || seed.is_some() {
        selected.push("jwt/nkey-seed");
    }
    if username.is_some() || password.is_some() {
        selected.push("username/password");
    }
    if token.is_some() {
        selected.push("token");
    }

    if selected.len() > 1 {
        bail!(
            "conflicting NATS credentials: {} were all supplied; set exactly one",
            selected.join(", ")
        );
    }

    if let Some(path) = creds {
        return Ok(NatsAuth::CredsFile(PathBuf::from(path)));
    }
    match (jwt, seed) {
        (Some(jwt), Some(seed)) => {
            return Ok(NatsAuth::JwtNkey {
                jwt: jwt.to_string(),
                seed: Zeroizing::new(seed.to_string()),
            });
        }
        (Some(_), None) => bail!("`jwt` requires `nkey-seed`"),
        (None, Some(seed)) => return Ok(NatsAuth::NkeySeed(Zeroizing::new(seed.to_string()))),
        (None, None) => {}
    }
    match (username, password) {
        (Some(username), Some(password)) => {
            return Ok(NatsAuth::UserPassword {
                username: username.to_string(),
                password: Zeroizing::new(password.to_string()),
            });
        }
        (Some(_), None) => bail!("`username` requires `password`"),
        (None, Some(_)) => bail!("`password` requires `username`"),
        (None, None) => {}
    }
    if let Some(token) = token {
        return Ok(NatsAuth::Token(Zeroizing::new(token.to_string())));
    }
    Ok(NatsAuth::Anonymous)
}

impl NatsConfig {
    /// Parses and validates a workload's config map.
    pub fn from_map(cfg: &HashMap<String, String>) -> anyhow::Result<Self> {
        let servers = list(cfg, "servers");
        if servers.is_empty() {
            bail!("wasmcloud:nats requires a `servers` config value");
        }
        for server in &servers {
            if server.contains('@') {
                bail!(
                    "credentials embedded in `servers` are not applied by the NATS client and \
                     would connect unauthenticated; use `creds`, `jwt`/`nkey-seed`, \
                     `username`/`password`, or `token` instead"
                );
            }
        }

        let ack_mode = match get(cfg, "ack-mode") {
            None | Some("auto") => AckMode::Auto,
            Some("manual") => AckMode::Manual,
            Some(other) => bail!("`ack-mode` must be `auto` or `manual`, got `{other}`"),
        };

        let request_timeout = match get(cfg, "request-timeout-ms") {
            Some(raw) => Some(Duration::from_millis(raw.parse().with_context(|| {
                format!("`request-timeout-ms` must be a positive integer, got `{raw}`")
            })?)),
            None => None,
        };

        let limits = Limits {
            max_in_flight: parse_usize(cfg, "max-in-flight", DEFAULT_MAX_IN_FLIGHT)?,
            subscription_capacity: parse_usize(
                cfg,
                "subscription-capacity",
                DEFAULT_SUBSCRIPTION_CAPACITY,
            )?,
        };
        if limits.max_in_flight == 0 {
            bail!("`max-in-flight` must be greater than zero");
        }

        let tls = TlsConfig {
            ca: get(cfg, "tls-ca").map(PathBuf::from),
            cert: get(cfg, "tls-cert").map(PathBuf::from),
            key: get(cfg, "tls-key").map(PathBuf::from),
            first: parse_bool(cfg, "tls-first")?,
        };
        if tls.cert.is_some() != tls.key.is_some() {
            bail!("`tls-cert` and `tls-key` must be set together");
        }

        Ok(Self {
            servers,
            name: get(cfg, "name").map(String::from),
            jetstream_domain: get(cfg, "jetstream-domain").map(String::from),
            inbox_prefix: get(cfg, "inbox-prefix").map(String::from),
            auth: parse_auth(cfg)?,
            tls,
            policy: PolicySpec {
                subject_allow: list(cfg, "subject-allow"),
                stream_allow: list(cfg, "stream-allow"),
                bucket_allow: list(cfg, "bucket-allow"),
            },
            limits,
            ack_mode,
            request_timeout,
        })
    }

    /// Stable key for connection sharing within one workload.
    ///
    /// Credentials are represented by a hash, never by value, so the key can be
    /// held in a map and logged without leaking material.
    pub fn connection_key(&self) -> ConnKey {
        use std::hash::{Hash as _, Hasher as _};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        match &self.auth {
            NatsAuth::Anonymous => 0u8.hash(&mut hasher),
            NatsAuth::CredsFile(path) => {
                1u8.hash(&mut hasher);
                path.hash(&mut hasher);
            }
            NatsAuth::JwtNkey { jwt, seed } => {
                2u8.hash(&mut hasher);
                jwt.hash(&mut hasher);
                seed.as_str().hash(&mut hasher);
            }
            NatsAuth::NkeySeed(seed) => {
                3u8.hash(&mut hasher);
                seed.as_str().hash(&mut hasher);
            }
            NatsAuth::UserPassword { username, password } => {
                4u8.hash(&mut hasher);
                username.hash(&mut hasher);
                password.as_str().hash(&mut hasher);
            }
            NatsAuth::Token(token) => {
                5u8.hash(&mut hasher);
                token.as_str().hash(&mut hasher);
            }
        }
        self.tls.ca.hash(&mut hasher);
        self.tls.cert.hash(&mut hasher);
        self.tls.key.hash(&mut hasher);
        self.tls.first.hash(&mut hasher);

        ConnKey {
            servers: self.servers.clone(),
            name: self.name.clone(),
            jetstream_domain: self.jetstream_domain.clone(),
            inbox_prefix: self.inbox_prefix.clone(),
            credential_fingerprint: hasher.finish(),
        }
    }
}

/// Identity of a connection within a workload. Never shared across workloads.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConnKey {
    pub servers: Vec<String>,
    pub name: Option<String>,
    pub jetstream_domain: Option<String>,
    pub inbox_prefix: Option<String>,
    pub credential_fingerprint: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn servers_are_required() {
        let err = NatsConfig::from_map(&map(&[])).unwrap_err();
        assert!(err.to_string().contains("`servers`"));
    }

    #[test]
    fn accepts_snake_case_aliases() {
        let cfg = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("subject_allow", "orders.>"),
            ("inbox_prefix", "_INBOX_x"),
        ]))
        .unwrap();
        assert_eq!(cfg.policy.subject_allow, vec!["orders.>"]);
        assert_eq!(cfg.inbox_prefix.as_deref(), Some("_INBOX_x"));
    }

    #[test]
    fn url_userinfo_is_rejected_not_ignored() {
        let err = NatsConfig::from_map(&map(&[("servers", "nats://user:pass@localhost:4222")]))
            .unwrap_err();
        assert!(err.to_string().contains("unauthenticated"));
    }

    #[test]
    fn conflicting_credentials_fail() {
        let err = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("token", "t"),
            ("username", "u"),
            ("password", "p"),
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("conflicting NATS credentials"));
    }

    #[test]
    fn jwt_without_seed_fails() {
        let err = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("jwt", "eyJ0"),
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("requires `nkey-seed`"));
    }

    #[test]
    fn username_without_password_fails() {
        let err = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("username", "u"),
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("requires `password`"));
    }

    #[test]
    fn tls_cert_requires_key() {
        let err = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("tls-cert", "/tmp/c.pem"),
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("must be set together"));
    }

    #[test]
    fn bad_ack_mode_fails() {
        let err = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("ack-mode", "sometimes"),
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("`ack-mode`"));
    }

    #[test]
    fn defaults_are_deny_all() {
        let cfg = NatsConfig::from_map(&map(&[("servers", "nats://localhost:4222")])).unwrap();
        assert!(cfg.policy.subject_allow.is_empty());
        assert!(cfg.policy.stream_allow.is_empty());
        assert!(cfg.policy.bucket_allow.is_empty());
        assert_eq!(cfg.ack_mode, AckMode::Auto);
    }

    #[test]
    fn connection_key_separates_credentials() {
        let base = [("servers", "nats://localhost:4222")];
        let a = NatsConfig::from_map(&map(&[base[0], ("token", "one")]))
            .unwrap()
            .connection_key();
        let b = NatsConfig::from_map(&map(&[base[0], ("token", "two")]))
            .unwrap()
            .connection_key();
        assert_ne!(a, b);
    }

    #[test]
    fn connection_key_matches_for_identical_config() {
        let pairs = [("servers", "nats://localhost:4222"), ("token", "same")];
        let a = NatsConfig::from_map(&map(&pairs)).unwrap().connection_key();
        let b = NatsConfig::from_map(&map(&pairs)).unwrap().connection_key();
        assert_eq!(a, b);
    }

    #[test]
    fn auth_debug_never_leaks() {
        let cfg = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("token", "super-secret"),
        ]))
        .unwrap();
        let rendered = format!("{cfg:?}");
        assert!(!rendered.contains("super-secret"));
        assert!(rendered.contains("token"));
    }
}
