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

use crate::nats_credentials::{AuthHook, Credentials, Secret};

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
/// Per subscription. The host's own budget bounds the *sum* separately and
/// dynamically — see [`super::subscriber::HostBacklogBudget`] — so this is
/// never narrowed behind an operator's back.
pub const DEFAULT_SUBSCRIPTION_CAPACITY_BYTES: usize = 32 * 1024 * 1024;

/// Default ceiling on how many times a JetStream delivery is retried.
///
/// Unset, the server retries forever, so a handler that traps deterministically
/// — the same input, the same trap — never makes progress and never stops. This
/// is what turns that livelock into a bounded ladder ending in a term, so a
/// poison message reaches a dead-letter state instead of being retried until
/// the stream ages it out. `0` restores the server's own default (unlimited).
const DEFAULT_MAX_DELIVER: usize = 32;

/// The smallest `max_ack_pending` the byte derivation may produce on its own.
///
/// Below this a consumer is not being paced, it is being serialised: at 1 the
/// server will not send a second message until the first settles, so every
/// per-message cost — instantiation included — is on the critical path and
/// throughput collapses silently. 16 is the value the campaign measured clean
/// at the payload that motivated the byte bound in the first place
/// (`mif=8 -> ack_pending 16 -> ~14MiB in flight`).
const MIN_DERIVED_MAX_ACK_PENDING: usize = 16;

/// How a workload authenticates to NATS.
///
/// Seeds are held in `Zeroizing` so they are wiped when the config is dropped.
/// The signing callback built from them stays host-side; the seed never
/// crosses the sandbox boundary. A secret given as a file is reread on every
/// connect, so a rotated one is presented on the next reconnect.
///
/// `jwt_as_token` repeats the user JWT as `auth_token`, for an auth callout
/// behind a server without trusted operators, which never sees the `jwt`.
pub enum NatsAuth {
    Anonymous,
    CredsFile {
        path: PathBuf,
        jwt_as_token: bool,
    },
    JwtNkey {
        jwt: String,
        seed: Secret,
        jwt_as_token: bool,
    },
    NkeySeed(Secret),
    UserPassword {
        username: String,
        password: Secret,
    },
    Token(Secret),
}

impl NatsAuth {
    /// Short label for logs. Never includes credential material.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Anonymous => "anonymous",
            Self::CredsFile { .. } => "creds-file",
            Self::JwtNkey {
                seed: Secret::Value(_),
                ..
            } => "jwt+nkey",
            Self::JwtNkey {
                seed: Secret::File(_),
                ..
            } => "jwt+nkey-file",
            Self::NkeySeed(Secret::Value(_)) => "nkey",
            Self::NkeySeed(Secret::File(_)) => "nkey-file",
            Self::UserPassword {
                password: Secret::Value(_),
                ..
            } => "user-password",
            Self::UserPassword {
                password: Secret::File(_),
                ..
            } => "user-password-file",
            Self::Token(Secret::Value(_)) => "token",
            Self::Token(Secret::File(_)) => "token-file",
        }
    }

    /// What a connection on this binding presents.
    pub fn credentials(&self) -> Credentials {
        match self {
            Self::Anonymous => Credentials::default(),
            Self::CredsFile { path, jwt_as_token } => Credentials {
                credentials: Some(path.clone()),
                hook: jwt_as_token.then(AuthHook::jwt_as_token),
                ..Default::default()
            },
            Self::JwtNkey {
                jwt,
                seed,
                jwt_as_token,
            } => Credentials {
                jwt: Some(jwt.clone()),
                nkey: Some(seed.clone()),
                hook: jwt_as_token.then(AuthHook::jwt_as_token),
                ..Default::default()
            },
            Self::NkeySeed(seed) => Credentials {
                nkey: Some(seed.clone()),
                ..Default::default()
            },
            Self::UserPassword { username, password } => Credentials {
                user: Some(username.clone()),
                password: Some(password.clone()),
                ..Default::default()
            },
            Self::Token(token) => Credentials {
                token: Some(token.clone()),
                ..Default::default()
            },
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
    pub stream_delete_allow: Vec<String>,
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
    /// deliveries on a host that may only have 256MiB.
    ///
    /// `per_message_bytes` is the denominator, and choosing it badly is worse
    /// than not bounding at all. The server's `max_payload` is a *limit*, not
    /// a prediction: a rig raising it to 64MiB for one XL workload made
    /// `32MiB / 64MiB` floor to zero, and every JetStream consumer on that
    /// host silently serialised at `max_ack_pending = 1`. So the caller passes
    /// the stream's own `max_msg_size` where the stream sets one — that *is* a
    /// statement about this stream's messages — and the derivation never
    /// returns less than [`MIN_DERIVED_MAX_ACK_PENDING`]. Below that a
    /// consumer stops being paced and starts being serialised, which is a
    /// throughput cliff an operator did not ask for and cannot see.
    ///
    /// [`Limits::ack_pending_advisory`] reports the case where the floor is
    /// what decided, since a bound the host quietly declined to apply is
    /// exactly the kind of thing that has to be said out loud.
    ///
    /// `0` hands the decision back to the server, whose default is far above
    /// either.
    pub fn effective_max_ack_pending(&self, per_message_bytes: u64) -> i64 {
        if let Some(explicit) = self.max_ack_pending {
            return i64::try_from(explicit).unwrap_or(i64::MAX);
        }
        let by_count = self
            .max_in_flight
            .saturating_mul(2)
            .min(self.subscription_capacity);
        let derived = by_count
            .min(self.max_ack_pending_by_bytes(per_message_bytes))
            .max(MIN_DERIVED_MAX_ACK_PENDING.min(by_count));
        i64::try_from(derived).unwrap_or(i64::MAX)
    }

    /// Says when the byte derivation was overruled by
    /// [`MIN_DERIVED_MAX_ACK_PENDING`], so the bind can name it.
    ///
    /// `None` when the byte budget genuinely covers the floor, or when the
    /// operator set `max-ack-pending` themselves.
    pub fn ack_pending_advisory(&self, per_message_bytes: u64) -> Option<String> {
        if self.max_ack_pending.is_some() {
            return None;
        }
        let by_bytes = self.max_ack_pending_by_bytes(per_message_bytes);
        let by_count = self
            .max_in_flight
            .saturating_mul(2)
            .min(self.subscription_capacity);
        if by_bytes >= MIN_DERIVED_MAX_ACK_PENDING.min(by_count) {
            return None;
        }
        let applied = self.effective_max_ack_pending(per_message_bytes);
        Some(format!(
            "`subscription-capacity-bytes` of {} against a {}-byte per-message size derives \
             max-ack-pending {by_bytes}, which would serialise delivery; using {applied} \
             instead. Worst-case unsettled bytes are therefore up to {}. Set `max-ack-pending` \
             explicitly, raise `subscription-capacity-bytes`, or set the stream's \
             `max_msg_size` if its messages are smaller than the server's `max_payload`.",
            self.subscription_capacity_bytes,
            per_message_bytes,
            (applied as u64).saturating_mul(per_message_bytes),
        ))
    }

    /// The unsettled-delivery count at which the worst case still fits
    /// `subscription-capacity-bytes`.
    fn max_ack_pending_by_bytes(&self, per_message_bytes: u64) -> usize {
        let per_message = per_message_bytes.max(1);
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

/// Reads a key through the plugin's key table, in whichever spelling — or
/// alias — a manifest used.
///
/// Going through [`super::keys`] rather than reading the map directly is what
/// keeps this reader and [`super::binding_schema`] from drifting: a key asked
/// for here that the table does not name trips a debug assertion, and a key in
/// the table nothing asks for is dead weight the table's own tests name.
fn get<'a>(cfg: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    super::keys::get(cfg, key)
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

/// A secret given inline as `key` or as a file named by `file_key`, but not
/// both.
fn secret(
    cfg: &HashMap<String, String>,
    key: &str,
    file_key: &str,
) -> anyhow::Result<Option<Secret>> {
    match (get(cfg, key), get(cfg, file_key)) {
        (Some(_), Some(_)) => bail!("`{key}` and `{file_key}` were both supplied; set one"),
        (Some(value), None) => Ok(Some(Secret::Value(Zeroizing::new(value.to_string())))),
        (None, Some(path)) => Ok(Some(Secret::File(PathBuf::from(path)))),
        (None, None) => Ok(None),
    }
}

/// Selects exactly one auth mechanism, or fails.
///
/// Silently preferring one when several are set hides a misconfiguration until
/// the connection is refused, so a conflict is an error at bind time.
fn parse_auth(cfg: &HashMap<String, String>) -> anyhow::Result<NatsAuth> {
    // One `get` per setting: aliases are resolved by the key table, so
    // `creds-file` and `creds` are the same lookup rather than two.
    let creds = get(cfg, "creds");
    let jwt = get(cfg, "jwt");
    let seed = secret(cfg, "nkey-seed", "nkey-seed-file")?;
    let username = get(cfg, "username");
    let password = secret(cfg, "password", "password-file")?;
    let token = secret(cfg, "token", "token-file")?;
    let jwt_as_token = parse_bool(cfg, "jwt-as-token")?;

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
        return Ok(NatsAuth::CredsFile {
            path: PathBuf::from(path),
            jwt_as_token,
        });
    }
    if jwt_as_token && jwt.is_none() {
        bail!("`jwt-as-token` requires `creds` or `jwt`");
    }
    match (jwt, seed) {
        (Some(jwt), Some(seed)) => {
            return Ok(NatsAuth::JwtNkey {
                jwt: jwt.to_string(),
                seed,
                jwt_as_token,
            });
        }
        (Some(_), None) => bail!("`jwt` requires `nkey-seed` or `nkey-seed-file`"),
        (None, Some(seed)) => return Ok(NatsAuth::NkeySeed(seed)),
        (None, None) => {}
    }
    match (username, password) {
        (Some(username), Some(password)) => {
            return Ok(NatsAuth::UserPassword {
                username: username.to_string(),
                password,
            });
        }
        (Some(_), None) => bail!("`username` requires `password` or `password-file`"),
        (None, Some(_)) => bail!("`password` requires `username`"),
        (None, None) => {}
    }
    if let Some(token) = token {
        return Ok(NatsAuth::Token(token));
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
                     `username`/`password`, `token`, or their `-file` keys instead"
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
        // Each sizes a semaphore or a bounded channel, which panic above this.
        const MAX_PERMITS: usize = tokio::sync::Semaphore::MAX_PERMITS;
        for (key, value) in [
            ("max-in-flight", limits.max_in_flight),
            ("subscription-capacity", limits.subscription_capacity),
        ] {
            if value > MAX_PERMITS {
                bail!("`{key}` must be at most {MAX_PERMITS}, got {value}");
            }
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
                stream_delete_allow: list(cfg, "stream-delete-allow"),
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
            NatsAuth::CredsFile { path, jwt_as_token } => {
                1u8.hash(&mut hasher);
                path.hash(&mut hasher);
                jwt_as_token.hash(&mut hasher);
            }
            NatsAuth::JwtNkey {
                jwt,
                seed,
                jwt_as_token,
            } => {
                2u8.hash(&mut hasher);
                jwt.hash(&mut hasher);
                hash_secret(seed, &mut hasher);
                jwt_as_token.hash(&mut hasher);
            }
            NatsAuth::NkeySeed(seed) => {
                3u8.hash(&mut hasher);
                hash_secret(seed, &mut hasher);
            }
            NatsAuth::UserPassword { username, password } => {
                4u8.hash(&mut hasher);
                username.hash(&mut hasher);
                hash_secret(password, &mut hasher);
            }
            NatsAuth::Token(token) => {
                5u8.hash(&mut hasher);
                hash_secret(token, &mut hasher);
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
            &self.policy.stream_delete_allow,
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

/// A file's path stands in for its contents, which change under rotation
/// without changing the binding.
fn hash_secret(secret: &Secret, hasher: &mut impl std::hash::Hasher) {
    use std::hash::Hash as _;
    match secret {
        Secret::Value(value) => {
            0u8.hash(hasher);
            value.as_str().hash(hasher);
        }
        Secret::File(path) => {
            1u8.hash(hasher);
            path.hash(hasher);
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

/// Refuses a subject or filter NATS itself would reject.
///
/// Nothing else checks these, so an illegal subject reaches the server, the
/// subscribe fails, and the subscription parks under a workload that reports
/// running — the same failure the queue-group checks above exist to prevent.
fn validate_subject(entry: &str, subject: &str, what: &str) -> anyhow::Result<()> {
    if subject.chars().any(char::is_whitespace) {
        anyhow::bail!("{what} `{entry}` has a subject containing whitespace")
    }
    if subject.split('.').any(str::is_empty) {
        anyhow::bail!("{what} `{entry}` has a subject with an empty token")
    }
    Ok(())
}

/// Parses `STREAM:filter[:policy[:queue]]`, comma separated. An empty policy
/// slot is the default, so a queue group does not force it to be spelled out.
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
            validate_subject(entry, filter_subject, "jetstream subscription")?;
            // An empty slot is the default, so naming a queue group does not
            // force the policy to be spelled out: `STREAM:filter::group`.
            let deliver_policy = match parts.get(2).copied() {
                None | Some("") => "new",
                Some(policy) => policy,
            };
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
            // `:` is not special to NATS, so `orders:eu` is a legal subject
            // that this grammar reads as `orders` in queue group `eu`. That
            // one cannot be told apart here; a second `:` always can, and is
            // never what was meant.
            if entry.matches(':').count() > 1 {
                anyhow::bail!(
                    "core subscription `{entry}` has more than one `:`; the grammar is \
                     subject[:queue], so a subject containing `:` cannot be expressed"
                )
            }
            let mut parts = entry.splitn(2, ':');
            let subject = parts.next().unwrap_or_default();
            if subject.is_empty() {
                anyhow::bail!("core subscription `{entry}` has an empty subject")
            }
            validate_subject(entry, subject, "core subscription")?;
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

/// Parses `bucket[:filter]`, comma separated. An omitted filter is `>`.
pub(super) fn parse_kv_watches(binding: &str, raw: &str) -> anyhow::Result<Vec<KvWatchConfig>> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            // A bare bucket watches all of it, which is what `>` means.
            let (bucket, filter) = match entry.split_once(':') {
                Some((bucket, filter)) => (bucket, filter),
                None => (entry, ">"),
            };
            if bucket.is_empty() || filter.is_empty() {
                anyhow::bail!("kv watch `{entry}` has an empty bucket or filter")
            }
            validate_subject(entry, filter, "kv watch")?;
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn stream_delete_grant_is_deny_by_default_and_part_of_connection_identity() {
        let base = NatsConfig::from_map(&map(&[("servers", "nats://localhost:4222")])).unwrap();
        assert!(base.policy.stream_delete_allow.is_empty());
        let grant = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("stream-delete-allow", "ORDERS"),
        ]))
        .unwrap();
        assert_eq!(grant.policy.stream_delete_allow, ["ORDERS"]);
        assert_ne!(base.connection_key(), grant.connection_key());
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

    /// Each inline secret has a `-file` key, so a Secret can be mounted and
    /// rotated rather than resolved into the binding once.
    #[test]
    fn secrets_may_be_files() {
        let auth = |pairs: &[(&str, &str)]| {
            let mut pairs = pairs.to_vec();
            pairs.push(("servers", "nats://localhost:4222"));
            NatsConfig::from_map(&map(&pairs)).map(|cfg| cfg.auth)
        };
        let file = |secret: &Secret| match secret {
            Secret::File(path) => Some(path.clone()),
            Secret::Value(_) => None,
        };

        let token = auth(&[("token-file", "/s/token")]).unwrap();
        assert_eq!(token.kind(), "token-file");
        assert!(matches!(&token, NatsAuth::Token(t) if file(t) == Some("/s/token".into())));

        let seed = auth(&[("nkey-file", "/s/seed")]).unwrap();
        assert_eq!(seed.kind(), "nkey-file");
        assert!(matches!(&seed, NatsAuth::NkeySeed(s) if file(s) == Some("/s/seed".into())));

        let jwt = auth(&[("jwt", "eyJ0"), ("nkey-seed-file", "/s/seed")]).unwrap();
        assert_eq!(jwt.kind(), "jwt+nkey-file");
        assert!(matches!(&jwt, NatsAuth::JwtNkey { seed, .. } if file(seed).is_some()));

        let user = auth(&[("username", "u"), ("password-file", "/s/pass")]).unwrap();
        assert_eq!(user.kind(), "user-password-file");
        assert!(
            matches!(&user, NatsAuth::UserPassword { password, .. } if file(password).is_some())
        );
    }

    #[test]
    fn jwt_as_token_needs_a_jwt() {
        let parse = |pairs: &[(&str, &str)]| {
            let mut pairs = pairs.to_vec();
            pairs.push(("servers", "nats://localhost:4222"));
            pairs.push(("jwt-as-token", "true"));
            NatsConfig::from_map(&map(&pairs))
        };
        let creds = parse(&[("creds", "/s/creds")]).unwrap();
        assert!(matches!(
            creds.auth,
            NatsAuth::CredsFile {
                jwt_as_token: true,
                ..
            }
        ));
        let jwt = parse(&[("jwt", "eyJ0"), ("nkey-seed-file", "/s/seed")]).unwrap();
        assert!(matches!(
            jwt.auth,
            NatsAuth::JwtNkey {
                jwt_as_token: true,
                ..
            }
        ));
        for pairs in [&[("token", "t")][..], &[("nkey-seed-file", "/s/seed")], &[]] {
            let err = parse(pairs).unwrap_err();
            assert!(
                err.to_string().contains("requires `creds` or `jwt`"),
                "{err}"
            );
        }
    }

    #[test]
    fn connection_key_separates_jwt_as_token() {
        let key = |pairs: &[(&str, &str)]| {
            let mut pairs = pairs.to_vec();
            pairs.push(("servers", "nats://localhost:4222"));
            pairs.push(("creds", "/s/creds"));
            NatsConfig::from_map(&map(&pairs)).unwrap().connection_key()
        };
        assert_eq!(key(&[]), key(&[("jwt-as-token", "false")]));
        assert_ne!(key(&[]), key(&[("jwt-as-token", "true")]));
    }

    #[test]
    fn a_secret_and_its_file_conflict() {
        for (key, file_key) in [
            ("token", "token-file"),
            ("password", "password-file"),
            ("nkey-seed", "nkey-seed-file"),
        ] {
            let err = NatsConfig::from_map(&map(&[
                ("servers", "nats://localhost:4222"),
                ("username", "u"),
                (key, "inline"),
                (file_key, "/s/file"),
            ]))
            .unwrap_err();
            assert!(
                err.to_string().contains("were both supplied"),
                "{key}: {err}"
            );
        }
        let err = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("creds", "/s/creds"),
            ("token-file", "/s/token"),
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("conflicting NATS credentials"));
    }

    /// A file's path is its identity: two bindings reading one file share a
    /// connection, and a value is never mistaken for a path that spells it.
    #[test]
    fn connection_key_separates_secret_files() {
        let key = |pairs: &[(&str, &str)]| {
            let mut pairs = pairs.to_vec();
            pairs.push(("servers", "nats://localhost:4222"));
            NatsConfig::from_map(&map(&pairs)).unwrap().connection_key()
        };
        assert_eq!(key(&[("token-file", "/a")]), key(&[("token-file", "/a")]));
        assert_ne!(key(&[("token-file", "/a")]), key(&[("token-file", "/b")]));
        assert_ne!(key(&[("token-file", "/a")]), key(&[("token", "/a")]));
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
        assert!(cfg.limits.effective_max_ack_pending(64 * 1024 * 1024) > 0);
    }

    #[test]
    fn max_ack_pending_never_derives_to_a_serialised_consumer() {
        // The regression the floor exists for: a rig raising the server's
        // `max_payload` to 64MiB for one XL workload made `32MiB / 64MiB`
        // floor to zero, and `.max(1)` turned that into `max_ack_pending=1` —
        // the server sending one message at a time, on every JetStream
        // consumer on that host, silently.
        let cfg = NatsConfig::from_map(&map(&[("servers", "nats://localhost:4222")])).unwrap();
        assert_eq!(
            cfg.limits.effective_max_ack_pending(64 * 1024 * 1024),
            MIN_DERIVED_MAX_ACK_PENDING as i64,
            "a raised server max_payload must not serialise delivery"
        );
    }

    #[test]
    fn a_derivation_overruled_by_the_floor_says_so() {
        let cfg = NatsConfig::from_map(&map(&[("servers", "nats://localhost:4222")])).unwrap();
        // 32MiB / 1MiB = 32, comfortably above the floor: nothing to say.
        assert!(cfg.limits.ack_pending_advisory(1024 * 1024).is_none());

        // 32MiB / 64MiB = 0: the floor decided, and the operator has to know
        // both that it did and what the resulting worst case is.
        let advisory = cfg
            .limits
            .ack_pending_advisory(64 * 1024 * 1024)
            .expect("the floor overruled the byte budget");
        assert!(advisory.contains("serialise"), "{advisory}");
        assert!(advisory.contains("max-ack-pending"), "{advisory}");

        // An operator who set it themselves is not second-guessed.
        let explicit = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("max-ack-pending", "4"),
        ]))
        .unwrap();
        assert!(
            explicit
                .limits
                .ack_pending_advisory(64 * 1024 * 1024)
                .is_none()
        );
        assert_eq!(
            explicit.limits.effective_max_ack_pending(64 * 1024 * 1024),
            4
        );
    }

    #[test]
    fn the_floor_never_raises_a_deliberately_small_handler_pool() {
        // `max-in-flight=1` is a request for one delivery at a time. The floor
        // exists to stop the *byte* derivation serialising a consumer, not to
        // overrule a count the operator chose.
        let cfg = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("max-in-flight", "1"),
        ]))
        .unwrap();
        assert_eq!(cfg.limits.effective_max_ack_pending(64 * 1024 * 1024), 2);
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

    /// The configured byte budget reaches the subscription untouched. The
    /// host-wide ceiling is a second bound applied at admission (see
    /// `HostBacklogBudget`); nothing narrows what the operator wrote, because
    /// the version that did chose 1MiB over a configured 65,536-message
    /// buffer by bind order and said only that it had "clamped".
    #[test]
    fn a_configured_byte_budget_is_never_narrowed() {
        let cfg = NatsConfig::from_map(&map(&[
            ("servers", "nats://localhost:4222"),
            ("subscription-capacity-bytes", "67108864"),
            ("subscription-capacity", "65536"),
        ]))
        .unwrap();
        assert_eq!(cfg.limits.subscription_capacity_bytes, 64 * 1024 * 1024);
        assert_eq!(cfg.limits.subscription_capacity, 65536);
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

    /// Workload config, so a huge value must fail the bind rather than panic
    /// the host in `Semaphore::new` or `mpsc::channel`.
    #[test]
    fn limits_above_what_a_semaphore_holds_fail() {
        let max = tokio::sync::Semaphore::MAX_PERMITS;
        for key in ["max-in-flight", "subscription-capacity"] {
            for value in [(max + 1).to_string(), usize::MAX.to_string()] {
                let err = NatsConfig::from_map(&map(&[
                    ("servers", "nats://localhost:4222"),
                    (key, &value),
                ]))
                .unwrap_err();
                assert!(err.to_string().contains("at most"), "{key}={value}: {err}");
            }
            NatsConfig::from_map(&map(&[
                ("servers", "nats://localhost:4222"),
                (key, &max.to_string()),
            ]))
            .unwrap_or_else(|e| panic!("{key}={max}: {e}"));
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

    /// A queue group must not force the policy to be spelled out, which is
    /// what an empty slot being an error amounted to.
    #[test]
    fn an_empty_deliver_policy_slot_is_the_default() {
        let subs = parse_jetstream_subscriptions("", "ORDERS:orders.>::workers").unwrap();
        assert_eq!(subs[0].deliver_policy, "new");
        assert_eq!(subs[0].queue_group.as_deref(), Some("workers"));
    }

    /// `:` is legal in a NATS subject, so this grammar cannot express one that
    /// contains it. A second `:` is the case that can always be told apart.
    #[test]
    fn a_core_subject_with_two_colons_is_refused() {
        let err = parse_core_subscriptions("", "orders:eu:west").unwrap_err();
        assert!(err.to_string().contains("more than one `:`"));
    }

    #[test]
    fn subjects_nats_would_reject_are_refused() {
        assert!(parse_core_subscriptions("", "orders..eu").is_err());
        assert!(parse_core_subscriptions("", "orders eu").is_err());
        assert!(parse_jetstream_subscriptions("", "ORDERS:orders..eu").is_err());
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

    /// A bare bucket watches all of it: `>` is the only thing it could mean,
    /// and `subscriptions` defaults its own optional slot the same way.
    #[test]
    fn a_kv_watch_without_a_filter_watches_the_whole_bucket() {
        let watches = parse_kv_watches("", "config").unwrap();
        assert_eq!(watches.len(), 1);
        assert_eq!(watches[0].bucket, "config");
        assert_eq!(watches[0].filter, ">");
    }

    #[test]
    fn kv_watches_reject_malformed() {
        // An explicit `:` still promises a filter, so an empty one is a typo.
        assert!(parse_kv_watches("", "config:").is_err());
        assert!(parse_kv_watches("", "config:a..b").is_err());
        assert!(parse_kv_watches("", "config:a b").is_err());
    }
}
