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
/// Default byte ceiling on a core subscription's host-side backlog.
///
/// `subscription-capacity` counts messages, so it bounds memory only if
/// payloads are small. This is the companion bound in bytes, and binds only
/// when large payloads make the message count a poor proxy.
///
/// Per subscription, and clamped down at bind time when the host's own memory
/// budget cannot carry every subscription at this size — see
/// [`Limits::clamp_capacity_bytes`].
pub const DEFAULT_SUBSCRIPTION_CAPACITY_BYTES: usize = 32 * 1024 * 1024;

/// Floor a bind-time clamp will not take a subscription below.
///
/// Below this the byte budget stops being backpressure and starts being a
/// throughput ceiling, and a subscription that admits one message at a time is
/// worse than one that sheds.
pub const MIN_SUBSCRIPTION_CAPACITY_BYTES: usize = 1024 * 1024;

/// Default ceiling on how many times a JetStream delivery is retried.
///
/// Unset, the server retries forever, so a handler that traps deterministically
/// — the same input, the same trap — never makes progress and never stops. This
/// is what turns that livelock into a bounded ladder ending in a term, so a
/// poison message reaches a dead-letter state instead of being retried until
/// the stream ages it out. `0` restores the server's own default (unlimited).
const DEFAULT_MAX_DELIVER: usize = 32;

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
    pub subscription_capacity_bytes: usize,
    /// What the JetStream push consumer asks the server for, or `None` to
    /// derive it. See [`Limits::effective_max_ack_pending`].
    pub max_ack_pending: Option<usize>,
    /// Ceiling on JetStream redeliveries, or `None` for
    /// [`DEFAULT_MAX_DELIVER`]. See [`Limits::effective_max_deliver`].
    pub max_deliver: Option<usize>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_in_flight: DEFAULT_MAX_IN_FLIGHT,
            subscription_capacity: DEFAULT_SUBSCRIPTION_CAPACITY,
            subscription_capacity_bytes: DEFAULT_SUBSCRIPTION_CAPACITY_BYTES,
            max_ack_pending: None,
            max_deliver: None,
        }
    }
}

impl Limits {
    /// How many deliveries a JetStream push consumer may leave unacked, as the
    /// server's `max_ack_pending`.
    ///
    /// The only backpressure in the stack the server enforces: past this many
    /// outstanding deliveries it stops sending until some settle.
    ///
    /// Unset, it is `max-in-flight` doubled (capped at
    /// `subscription-capacity`) and then **bounded in bytes**: a message count
    /// cannot bound memory, and `128 x 896KiB` is 112MiB of unsettled
    /// deliveries on a host that may only have 256MiB. `server_max_payload` is
    /// the largest a single message can be, so
    /// `subscription-capacity-bytes / max_payload` is the count at which the
    /// worst case still fits the byte budget the core path already honours.
    ///
    /// `0` hands the decision back to the server, whose default is far above
    /// either.
    pub fn effective_max_ack_pending(&self, server_max_payload: u64) -> i64 {
        if let Some(explicit) = self.max_ack_pending {
            return i64::try_from(explicit).unwrap_or(i64::MAX);
        }
        let derived = self
            .max_in_flight
            .saturating_mul(2)
            .min(self.subscription_capacity)
            .min(self.max_ack_pending_by_bytes(server_max_payload))
            // A byte budget smaller than one max-size message would derive
            // zero, which is the server's "unlimited" — the opposite of what
            // the budget asked for.
            .max(1);
        i64::try_from(derived).unwrap_or(i64::MAX)
    }

    /// The unsettled-delivery count at which the worst case still fits
    /// `subscription-capacity-bytes`.
    fn max_ack_pending_by_bytes(&self, server_max_payload: u64) -> usize {
        let per_message = server_max_payload.max(1);
        usize::try_from(self.subscription_capacity_bytes as u64 / per_message).unwrap_or(usize::MAX)
    }

    /// How many times a JetStream delivery may be retried before the server
    /// stops offering it.
    ///
    /// `-1` is the server's unlimited, which is what an unset `max_deliver`
    /// meant before this: a deterministically trapping handler retried until
    /// the stream aged the message out.
    pub fn effective_max_deliver(&self) -> i64 {
        match self.max_deliver.unwrap_or(DEFAULT_MAX_DELIVER) {
            0 => -1,
            n => i64::try_from(n).unwrap_or(i64::MAX),
        }
    }

    /// Narrows this binding's per-subscription byte budget to `ceiling`.
    ///
    /// Called at bind, once the host knows how many subscriptions will share
    /// its memory. Never raises the configured value, and never goes below
    /// [`MIN_SUBSCRIPTION_CAPACITY_BYTES`]. Returns the value actually applied
    /// so the caller can say when it clamped.
    pub fn clamp_capacity_bytes(&self, ceiling: usize) -> usize {
        self.subscription_capacity_bytes
            .min(ceiling.max(MIN_SUBSCRIPTION_CAPACITY_BYTES))
    }
}

/// Who acknowledges a JetStream delivery.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
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

/// The single spelling a key is stored under once several entries are folded
/// into one map.
///
/// [`get`] reads kebab-case and falls back to snake_case, so `subject-allow`
/// and `subject_allow` are one setting with two spellings. Anything deciding
/// whether two entries speak about the same key has to agree with that, or an
/// entry writing the other spelling would look like it were setting a key
/// nothing had claimed — and `get` would then quietly honour only one of them.
pub fn canonical_key(key: &str) -> String {
    key.replace('_', "-")
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
            subscription_capacity_bytes: parse_usize(
                cfg,
                "subscription-capacity-bytes",
                DEFAULT_SUBSCRIPTION_CAPACITY_BYTES,
            )?,
            // Zero is meaningful here -- it is how an operator says "server
            // default" -- so unlike the other two it is not rejected.
            max_ack_pending: match get(cfg, "max-ack-pending") {
                Some(raw) => Some(raw.parse().with_context(|| {
                    format!("`max-ack-pending` must be a non-negative integer, got `{raw}`")
                })?),
                None => None,
            },
            // Zero is meaningful here too: it is how an operator asks for the
            // server's unlimited redelivery back.
            max_deliver: match get(cfg, "max-deliver") {
                Some(raw) => Some(raw.parse().with_context(|| {
                    format!("`max-deliver` must be a non-negative integer, got `{raw}`")
                })?),
                None => None,
            },
        };
        if limits.max_in_flight == 0 {
            bail!("`max-in-flight` must be greater than zero");
        }
        if limits.subscription_capacity == 0 {
            bail!("`subscription-capacity` must be greater than zero");
        }
        if limits.subscription_capacity_bytes == 0 {
            bail!("`subscription-capacity-bytes` must be greater than zero");
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
    /// Two bindings share a connection only when everything that governs a call
    /// on it matches — not just where it points, but the grant it is checked
    /// against and how deliveries are acknowledged. Named bindings made that
    /// load-bearing: `hub` and `leaf` may name the same server with different
    /// `subject-allow`, and sharing there would hand the second binding the
    /// first one's grant.
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

        // Everything a shared connection would carry into a call: the grant,
        // the ack ownership, the backpressure bounds, the request timeout.
        //
        // An allow-list is a set, not a sequence: two bindings naming the same
        // subjects in a different order describe the same grant, and refusing
        // that deploy would be a false alarm. Sorting a copy keeps ordering out
        // of the identity while leaving the config itself — which the policy
        // engine reads in declaration order — untouched.
        for allow in [
            &self.policy.subject_allow,
            &self.policy.stream_allow,
            &self.policy.bucket_allow,
        ] {
            let mut sorted = allow.clone();
            sorted.sort_unstable();
            sorted.hash(&mut hasher);
        }
        self.ack_mode.hash(&mut hasher);
        self.limits.max_in_flight.hash(&mut hasher);
        self.limits.subscription_capacity.hash(&mut hasher);
        self.limits.subscription_capacity_bytes.hash(&mut hasher);
        self.limits.max_ack_pending.hash(&mut hasher);
        self.limits.max_deliver.hash(&mut hasher);
        self.request_timeout.hash(&mut hasher);

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

#[derive(Clone, Debug)]
pub(super) struct JetStreamSubscriptionConfig {
    /// The binding this subscription was declared on, and whose connection and
    /// grant it runs under. Empty for a plain, unlabeled binding.
    pub binding: String,
    pub stream: String,
    pub filter_subject: String,
    pub deliver_policy: String,
    pub queue_group: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct CoreSubscriptionConfig {
    pub binding: String,
    pub subject: String,
    pub queue_group: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct KvWatchConfig {
    pub binding: String,
    pub bucket: String,
    pub filter: String,
}

/// Refuses a queue group no NATS server would accept.
///
/// Nothing downstream can recover from an illegal group: the core client
/// rejects the subscribe outright, and JetStream fails the create-consumer call
/// on every attempt, which the delivery loop can only keep retrying while the
/// workload reports running and receives nothing. The deploy is the last moment
/// the author is still watching, so the group is checked here.
fn validate_queue_group(entry: &str, group: &str, jetstream: bool) -> anyhow::Result<()> {
    if group.is_empty() {
        anyhow::bail!(
            "subscription `{entry}` has an empty queue group; drop the trailing `:` to \
             subscribe without one"
        )
    }
    if group.chars().any(|c| c.is_ascii_whitespace()) {
        anyhow::bail!("subscription `{entry}` has a queue group containing whitespace")
    }
    // A JetStream group is carried into a durable name and into one token of
    // the push deliver subject, neither of which admits subject syntax or a
    // path separator.
    if jetstream
        && let Some(bad) = group
            .chars()
            .find(|c| matches!(c, '.' | '*' | '>' | '/' | '\\'))
    {
        anyhow::bail!(
            "jetstream subscription `{entry}` has a queue group containing `{bad}`; the group \
             names a durable consumer, so it cannot contain `.`, `*`, `>`, `/` or `\\`"
        )
    }
    Ok(())
}

/// Parses `STREAM:filter[:policy[:queue]]`, comma separated.
pub(super) fn parse_jetstream_subscriptions(
    binding: &str,
    raw: &str,
) -> anyhow::Result<Vec<JetStreamSubscriptionConfig>> {
    let subs: Vec<JetStreamSubscriptionConfig> = raw
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let parts: Vec<&str> = entry.splitn(4, ':').collect();
            let (Some(stream), Some(filter_subject)) = (parts.first(), parts.get(1)) else {
                anyhow::bail!(
                    "invalid jetstream subscription `{entry}`, expected STREAM:filter[:policy[:queue]]"
                )
            };
            if stream.is_empty() || filter_subject.is_empty() {
                anyhow::bail!("jetstream subscription `{entry}` has an empty stream or filter")
            }
            let deliver_policy = parts.get(2).copied().unwrap_or("new");
            if !matches!(deliver_policy, "all" | "last" | "last-per-subject" | "new") {
                anyhow::bail!(
                    "jetstream subscription `{entry}` has an unknown deliver policy \
                     `{deliver_policy}`; expected all, last, last-per-subject, or new"
                )
            }
            let queue_group = parts.get(3).copied();
            if let Some(group) = queue_group {
                validate_queue_group(entry, group, true)?;
            }
            Ok(JetStreamSubscriptionConfig {
                binding: binding.to_string(),
                stream: (*stream).to_string(),
                filter_subject: (*filter_subject).to_string(),
                deliver_policy: deliver_policy.to_string(),
                queue_group: queue_group.map(str::to_string),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    // A queue group on a stream names one shared consumer, and the filter and
    // deliver policy belong to that consumer rather than to the entry that
    // declared it. Two entries that disagree about either can never both get
    // the consumer they asked for — whichever is created second either fails or
    // rewrites the first — so the pair is refused rather than left to a
    // retry loop under a workload that reports running.
    for (position, sub) in subs.iter().enumerate() {
        let Some(group) = sub.queue_group.as_deref() else {
            continue;
        };
        for earlier in subs.iter().take(position) {
            if earlier.stream != sub.stream || earlier.queue_group.as_deref() != Some(group) {
                continue;
            }
            if earlier.filter_subject != sub.filter_subject
                || earlier.deliver_policy != sub.deliver_policy
            {
                anyhow::bail!(
                    "jetstream subscriptions on stream `{}` share the queue group `{group}` but \
                     ask for different deliveries (`{}:{}` and `{}:{}`); one group is one \
                     consumer, so give each delivery its own group",
                    sub.stream,
                    earlier.filter_subject,
                    earlier.deliver_policy,
                    sub.filter_subject,
                    sub.deliver_policy
                )
            }
        }
    }

    Ok(subs)
}

/// Parses `subject[:queue]`, comma separated.
pub(super) fn parse_core_subscriptions(
    binding: &str,
    raw: &str,
) -> anyhow::Result<Vec<CoreSubscriptionConfig>> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let mut parts = entry.splitn(2, ':');
            let subject = parts.next().unwrap_or_default();
            if subject.is_empty() {
                anyhow::bail!("core subscription `{entry}` has an empty subject")
            }
            let queue_group = parts.next();
            if let Some(group) = queue_group {
                validate_queue_group(entry, group, false)?;
            }
            Ok(CoreSubscriptionConfig {
                binding: binding.to_string(),
                subject: subject.to_string(),
                queue_group: queue_group.map(str::to_string),
            })
        })
        .collect()
}

/// Parses `bucket:filter`, comma separated.
pub(super) fn parse_kv_watches(binding: &str, raw: &str) -> anyhow::Result<Vec<KvWatchConfig>> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let Some((bucket, filter)) = entry.split_once(':') else {
                anyhow::bail!("invalid kv watch `{entry}`, expected bucket:filter")
            };
            if bucket.is_empty() || filter.is_empty() {
                anyhow::bail!("kv watch `{entry}` has an empty bucket or filter")
            }
            Ok(KvWatchConfig {
                binding: binding.to_string(),
                bucket: bucket.to_string(),
                filter: filter.to_string(),
            })
        })
        .collect()
}

/// One declared subscription as the author wrote it, used both to recognise the
/// same subscription arriving on a second component and to name it in the error
/// that refuses the pair.
pub(super) fn subscription_spec(key: &str, binding: &str, value: String) -> String {
    if binding.is_empty() {
        format!("`{key}: {value}`")
    } else {
        format!("`{key}: {value}` on binding `{binding}`")
    }
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

    /// A server whose `max_payload` is small enough that the message-count
    /// derivation is the binding one.
    const SMALL_PAYLOAD: u64 = 16 * 1024;

    #[test]
    fn max_ack_pending_defaults_to_twice_max_in_flight() {
        let cfg = NatsConfig::from_map(&map(&[("servers", "nats://localhost:4222")])).unwrap();
        // 32MiB / 16KiB = 2048, so the count derivation is what binds here.
        assert_eq!(cfg.limits.effective_max_ack_pending(SMALL_PAYLOAD), 128);
    }

    #[test]
    fn max_ack_pending_is_bounded_in_bytes_not_only_in_messages() {
        // The regression this exists for: 128 unsettled deliveries of a
        // 896KiB payload is 112MiB in flight, on a host that may have 256MiB
        // in total. The byte budget has to be what decides, not the count.
        let cfg = NatsConfig::from_map(&map(&[("servers", "nats://localhost:4222")])).unwrap();
        let max_payload = 1024 * 1024;
        let derived = cfg.limits.effective_max_ack_pending(max_payload);
        assert_eq!(derived, 32, "32MiB of budget / 1MiB max payload");
        assert!(
            derived as u64 * max_payload <= cfg.limits.subscription_capacity_bytes as u64,
            "worst-case in-flight bytes must fit the byte budget"
        );
    }

    #[test]
    fn max_ack_pending_never_derives_to_the_servers_unlimited() {
        // A byte budget smaller than one max-size message divides to zero, and
        // zero is how the server spells "no limit" — the opposite of the ask.
        let cfg = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("subscription-capacity-bytes", "1048576"),
        ]))
        .unwrap();
        assert_eq!(cfg.limits.effective_max_ack_pending(64 * 1024 * 1024), 1);
    }

    #[test]
    fn max_deliver_defaults_to_a_bounded_ladder_and_zero_restores_unlimited() {
        let cfg = NatsConfig::from_map(&map(&[("servers", "nats://localhost:4222")])).unwrap();
        assert_eq!(cfg.limits.effective_max_deliver(), 32);

        let unlimited = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("max-deliver", "0"),
        ]))
        .unwrap();
        assert_eq!(unlimited.limits.effective_max_deliver(), -1);

        let explicit = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("max-deliver", "5"),
        ]))
        .unwrap();
        assert_eq!(explicit.limits.effective_max_deliver(), 5);
    }

    #[test]
    fn a_clamped_capacity_never_rises_and_never_starves() {
        let cfg = NatsConfig::from_map(&map(&[("servers", "nats://localhost:4222")])).unwrap();
        // A ceiling below the configured value binds.
        assert_eq!(
            cfg.limits.clamp_capacity_bytes(8 * 1024 * 1024),
            8 * 1024 * 1024
        );
        // A ceiling above it does not raise it.
        assert_eq!(
            cfg.limits.clamp_capacity_bytes(1024 * 1024 * 1024),
            DEFAULT_SUBSCRIPTION_CAPACITY_BYTES
        );
        // And no share of a small host takes a subscription below the floor.
        assert_eq!(
            cfg.limits.clamp_capacity_bytes(1024),
            MIN_SUBSCRIPTION_CAPACITY_BYTES
        );
    }

    #[test]
    fn max_ack_pending_never_exceeds_the_buffer_it_lands_in() {
        // Doubling 2000 would let the server keep 4000 deliveries outstanding
        // for a subscription buffer that holds 1024, which is exactly the
        // overflow this bound exists to prevent.
        let cfg = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("max-in-flight", "2000"),
            ("subscription-capacity", "1024"),
        ]))
        .unwrap();
        assert_eq!(cfg.limits.effective_max_ack_pending(SMALL_PAYLOAD), 1024);
    }

    #[test]
    fn max_ack_pending_is_overridable_and_zero_means_server_default() {
        let explicit = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("max-ack-pending", "5000"),
        ]))
        .unwrap();
        assert_eq!(
            explicit.limits.effective_max_ack_pending(SMALL_PAYLOAD),
            5000
        );

        let deferred = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("max-ack-pending", "0"),
        ]))
        .unwrap();
        assert_eq!(deferred.limits.effective_max_ack_pending(SMALL_PAYLOAD), 0);
    }

    #[test]
    fn backpressure_knobs_are_part_of_connection_identity() {
        // Two bindings that buffer differently cannot share a connection:
        // `subscription-capacity` is a connect option, and the byte budget and
        // ack ceiling are read off the handle every subscription uses.
        let base = &[("servers", "nats://localhost:4222")][..];
        let key = |extra: &[(&str, &str)]| {
            let mut all = base.to_vec();
            all.extend_from_slice(extra);
            NatsConfig::from_map(&map(&all)).unwrap().connection_key()
        };
        assert_ne!(key(&[]), key(&[("subscription-capacity-bytes", "1048576")]));
        assert_ne!(key(&[]), key(&[("max-ack-pending", "128")]));
    }

    #[test]
    fn zero_limits_fail() {
        for key in [
            "max-in-flight",
            "subscription-capacity",
            "subscription-capacity-bytes",
        ] {
            let err =
                NatsConfig::from_map(&map(&[("servers", "nats://localhost:4222"), (key, "0")]))
                    .unwrap_err();
            assert!(
                err.to_string().contains("greater than zero"),
                "{key}: {err}"
            );
        }
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
    fn connection_key_separates_subject_grants() {
        let a = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("subject-allow", "orders.>"),
        ]))
        .unwrap()
        .connection_key();
        let b = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("subject-allow", ">"),
        ]))
        .unwrap()
        .connection_key();
        assert_ne!(a, b);
    }

    #[test]
    fn connection_key_separates_ack_modes() {
        let a = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("ack-mode", "auto"),
        ]))
        .unwrap()
        .connection_key();
        let b = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("ack-mode", "manual"),
        ]))
        .unwrap()
        .connection_key();
        assert_ne!(a, b);
    }

    #[test]
    fn connection_key_ignores_allow_list_order() {
        let a = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("subject-allow", "orders.>,payments.>"),
            ("stream-allow", "ORDERS,PAYMENTS"),
            ("bucket-allow", "CONFIG,SECRETS"),
        ]))
        .unwrap()
        .connection_key();
        let b = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("subject-allow", "payments.>,orders.>"),
            ("stream-allow", "PAYMENTS,ORDERS"),
            ("bucket-allow", "SECRETS,CONFIG"),
        ]))
        .unwrap()
        .connection_key();
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
    #[test]
    fn jetstream_subs_basic() {
        let subs = parse_jetstream_subscriptions("", "ORDERS:orders.*:new").unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].stream, "ORDERS");
        assert_eq!(subs[0].filter_subject, "orders.*");
        assert_eq!(subs[0].deliver_policy, "new");
        assert_eq!(subs[0].queue_group, None);
    }

    #[test]
    fn jetstream_subs_with_queue() {
        let subs = parse_jetstream_subscriptions(
            "",
            "ORDERS:orders.*:new:workers,EVENTS:evt.>:all:group-a",
        )
        .unwrap();
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].queue_group.as_deref(), Some("workers"));
        assert_eq!(subs[1].stream, "EVENTS");
        assert_eq!(subs[1].queue_group.as_deref(), Some("group-a"));
    }

    #[test]
    fn jetstream_subs_reject_illegal_queue_groups() {
        for entry in [
            "S:f:new:team.a",
            "S:f:new:has space",
            "S:f:new:a>b",
            "S:f:new:a/b",
        ] {
            let err = parse_jetstream_subscriptions("", entry)
                .unwrap_err()
                .to_string();
            assert!(err.contains("queue group"), "{entry}: {err}");
        }
    }

    #[test]
    fn subs_reject_trailing_colon_queue_group() {
        let err = parse_jetstream_subscriptions("", "STREAM:filter:new:")
            .unwrap_err()
            .to_string();
        assert!(err.contains("empty queue group"), "{err}");

        let err = parse_core_subscriptions("", "subject:")
            .unwrap_err()
            .to_string();
        assert!(err.contains("empty queue group"), "{err}");

        assert!(parse_core_subscriptions("", "subject:workers").is_ok());
    }

    #[test]
    fn core_subs_reject_queue_group_with_whitespace() {
        let err = parse_core_subscriptions("", "subj:bad group")
            .unwrap_err()
            .to_string();
        assert!(err.contains("whitespace"), "{err}");
    }

    #[test]
    fn jetstream_subs_reject_conflicting_queue_group() {
        let err = parse_jetstream_subscriptions(
            "",
            "ORDERS:orders.us.*:new:workers,ORDERS:orders.eu.*:new:workers",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("share the queue group `workers`"), "{err}");

        let err = parse_jetstream_subscriptions(
            "",
            "ORDERS:orders.*:new:workers,ORDERS:orders.*:all:workers",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("share the queue group `workers`"), "{err}");

        // The same delivery declared twice is a duplicate, not a conflict, and
        // a group on another stream is a different consumer entirely.
        assert!(
            parse_jetstream_subscriptions(
                "",
                "ORDERS:orders.*:new:workers,ORDERS:orders.*:new:workers,EVENTS:evt.>:new:workers"
            )
            .is_ok()
        );
    }

    #[test]
    fn jetstream_subs_reject_malformed() {
        assert!(parse_jetstream_subscriptions("", "ORDERS").is_err());
        assert!(parse_jetstream_subscriptions("", ":orders.*").is_err());
    }

    #[test]
    fn jetstream_subs_reject_unknown_deliver_policy() {
        let err = parse_jetstream_subscriptions("", "ORDERS:orders.*:evrything").unwrap_err();
        assert!(err.to_string().contains("unknown deliver policy"));
    }

    #[test]
    fn core_subs_basic() {
        let subs = parse_core_subscriptions("", "events.*,metrics.>:stats").unwrap();
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].subject, "events.*");
        assert_eq!(subs[0].queue_group, None);
        assert_eq!(subs[1].subject, "metrics.>");
        assert_eq!(subs[1].queue_group.as_deref(), Some("stats"));
    }

    #[test]
    fn kv_watches_basic() {
        let watches = parse_kv_watches("", "config:*,secrets:prod.>").unwrap();
        assert_eq!(watches.len(), 2);
        assert_eq!(watches[0].bucket, "config");
        assert_eq!(watches[0].filter, "*");
        assert_eq!(watches[1].bucket, "secrets");
        assert_eq!(watches[1].filter, "prod.>");
    }

    #[test]
    fn kv_watches_reject_malformed() {
        assert!(parse_kv_watches("", "configonly").is_err());
        assert!(parse_kv_watches("", "config:").is_err());
    }
}
