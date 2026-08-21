//! Bucket naming and creation policy for the NATS JetStream keyvalue backends.
//!
//! A guest passes a *logical* name to `wasi:keyvalue/store.open`. This module
//! turns that name into the *physical* JetStream KV bucket the host reads and
//! writes, and decides whether the host may create that bucket at all. Both the
//! standard NATS plugin and the multiplexed NATS backend resolve their opens
//! through it, so the two agree on naming and on who may create streams.
//!
//! The guest never supplies connection details or creation settings: it names a
//! store, and the policy — owned by whoever configured the host — decides which
//! physical bucket that is and what its limits are.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use async_nats::jetstream::Context;
use async_nats::jetstream::kv::{Config, Store};
use async_nats::jetstream::stream::StorageType;
use tracing::instrument;

/// `config` key holding an explicit physical bucket name.
pub const BUCKET_KEY: &str = "bucket";
/// `config` key holding a prefix applied to every physical bucket name.
pub const BUCKET_PREFIX_KEY: &str = "bucket_prefix";
/// `config` key holding the [`CreatePolicy`].
pub const CREATE_KEY: &str = "create";
/// `config` keys for the settings a created bucket is given.
pub const REPLICAS_KEY: &str = "replicas";
pub const STORAGE_KEY: &str = "storage";
pub const MAX_AGE_KEY: &str = "max_age";
pub const HISTORY_KEY: &str = "history";
pub const MAX_BYTES_KEY: &str = "max_bytes";

/// JetStream's ceiling on a KV bucket's per-key history.
const MAX_HISTORY: i64 = 64;

/// Whether the host may create a JetStream KV bucket that a guest opens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CreatePolicy {
    /// Open pre-existing buckets only. An identifier with no bucket behind it
    /// is `no-such-store`, which makes the set of configured buckets an
    /// allowlist: a guest cannot bring new JetStream streams into existence,
    /// and the host's NATS credentials need no stream-create permission.
    #[default]
    Never,
    /// Create the bucket on first open when it is missing, with the settings
    /// on the [`BucketPolicy`].
    Missing,
}

impl CreatePolicy {
    /// Parse the `create` config value.
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "never" => Ok(Self::Never),
            "missing" => Ok(Self::Missing),
            other => Err(anyhow::anyhow!(
                "invalid keyvalue bucket create policy '{other}', expected 'never' or 'missing'"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::Missing => "missing",
        }
    }

    /// The policy in force when an interface asks for `self` under an
    /// embedder's `ceiling`.
    ///
    /// An interface may tighten (ask for `never` on a host that allows
    /// creation) but never loosen: a workload manifest cannot hand itself
    /// stream-creation on a host whose operator withheld it.
    pub fn clamped_to(self, ceiling: Self) -> Self {
        match (self, ceiling) {
            (Self::Missing, Self::Missing) => Self::Missing,
            _ => Self::Never,
        }
    }
}

/// Parse the `storage` config value into a JetStream storage type.
pub fn parse_storage(value: &str) -> anyhow::Result<StorageType> {
    match value {
        "file" => Ok(StorageType::File),
        "memory" => Ok(StorageType::Memory),
        other => Err(anyhow::anyhow!(
            "invalid keyvalue bucket storage '{other}', expected 'file' or 'memory'"
        )),
    }
}

/// How a logical bucket identifier maps onto a JetStream KV bucket, and what
/// that bucket looks like if the host is allowed to create it.
///
/// The defaults are the conservative ones: no name rewriting and
/// [`CreatePolicy::Never`], so an unconfigured policy can only reach buckets
/// that already exist.
#[derive(Clone, Debug, Default)]
pub struct BucketPolicy {
    /// Prefix prepended to every physical bucket name. The namespacing knob:
    /// two hosts (or tenants) pointed at one NATS cluster can use distinct
    /// prefixes so identical guest identifiers do not collide.
    pub prefix: String,
    /// Pins this policy to one physical bucket: when set, every identifier a
    /// guest opens resolves to this bucket and the identifier is ignored. Use
    /// it to bind an import to an operator-chosen store; leave it unset to let
    /// each identifier name its own bucket.
    pub bucket: Option<String>,
    /// Whether a missing bucket may be created.
    pub create: CreatePolicy,
    /// Settings applied to a bucket this policy creates. Unset fields keep
    /// JetStream's own defaults, and none of them affect a bucket that already
    /// exists.
    pub replicas: Option<usize>,
    pub storage: Option<StorageType>,
    pub max_age: Option<Duration>,
    pub history: Option<i64>,
    pub max_bytes: Option<i64>,
}

/// Buckets a refusal has already been logged for, so an operator gets one
/// warning per missing bucket instead of one per guest call.
///
/// Deliberately not a field on [`BucketPolicy`]: the policy is a plain
/// configuration value an embedder builds with a struct literal, and this is
/// bookkeeping. Capped because the names are guest-supplied — past the cap the
/// set is cleared, which starts a fresh window rather than growing without
/// bound or going permanently quiet.
static REFUSED_BUCKETS: std::sync::LazyLock<Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashSet::new()));

/// How many distinct refused buckets are remembered before the set resets.
const REFUSED_BUCKETS_CAP: usize = 256;

/// Whether this is the first refusal for `physical`, and so the one that warns.
fn first_refusal(physical: &str) -> bool {
    let mut refused = REFUSED_BUCKETS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if refused.len() >= REFUSED_BUCKETS_CAP {
        refused.clear();
    }
    refused.insert(physical.to_string())
}

/// What an [`BucketPolicy::open`] did, for the caller to record. The physical
/// bucket is on the span; this is what a metric needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenOutcome {
    /// The bucket was already there.
    Existing,
    /// The bucket was missing and this open took the create path.
    ///
    /// `create_key_value` is idempotent, which is what makes two opens racing
    /// to create the same bucket both succeed — so in that race both report
    /// `Created`. Read the metric as "opens that had to create", not as a
    /// count of streams brought into existence.
    Created,
}

impl OpenOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Existing => "existing",
            Self::Created => "created",
        }
    }
}

/// Why an open failed. Kept separate from the WIT `store.error` so both the
/// standard and multiplexed backends can map it into their own bindings.
#[derive(Clone, Debug)]
pub enum OpenError {
    /// No bucket behind this identifier (and the policy does not allow
    /// creating one).
    NoSuchStore,
    /// A real failure: transport, permissions, JetStream disabled, ...
    Other(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchStore => write!(f, "no such store"),
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

impl BucketPolicy {
    /// A policy that creates missing buckets, with JetStream's default
    /// settings. The development default: a guest's first `open` works with no
    /// configuration at all.
    pub fn create_missing() -> Self {
        Self {
            create: CreatePolicy::Missing,
            ..Self::default()
        }
    }

    /// Read a policy from a named host interface's `config`, falling back to
    /// `defaults` key by key.
    ///
    /// `defaults` is what the embedder configured (a `wash dev` project's
    /// `dev.wasi_keyvalue_nats`, a `wash host`'s `--keyvalue-nats-*` flags);
    /// an interface that sets a key overrides it for itself alone. Two keys
    /// are deliberately not plain overrides:
    ///
    /// - `create` is a ceiling, not a default: an interface may tighten it to
    ///   `never`, but asking for `missing` on a host that withheld creation
    ///   does not grant it. Otherwise a workload manifest could hand itself
    ///   stream-creation the operator declined to give.
    /// - `bucket` (the pin) is never inherited. A pin names one store, so
    ///   inheriting it would silently collapse every named interface — a
    ///   `sessions` and a `cache` import alike — onto the embedder's single
    ///   bucket. An interface that wants a pin states it.
    pub fn from_config(config: &HashMap<String, String>, defaults: &Self) -> anyhow::Result<Self> {
        let create = match config.get(CREATE_KEY) {
            Some(v) => CreatePolicy::parse(v)?.clamped_to(defaults.create),
            None => defaults.create,
        };
        let storage = match config.get(STORAGE_KEY) {
            Some(v) => Some(parse_storage(v)?),
            None => defaults.storage,
        };
        let max_age = match config.get(MAX_AGE_KEY) {
            Some(v) => Some(
                humantime::parse_duration(v)
                    .map_err(|e| anyhow::anyhow!("invalid keyvalue bucket max_age '{v}': {e}"))?,
            ),
            None => defaults.max_age,
        };

        Self {
            prefix: config
                .get(BUCKET_PREFIX_KEY)
                .cloned()
                .unwrap_or_else(|| defaults.prefix.clone()),
            bucket: config.get(BUCKET_KEY).cloned(),
            create,
            replicas: parse_number(config, REPLICAS_KEY)?.or(defaults.replicas),
            storage,
            max_age,
            history: parse_number(config, HISTORY_KEY)?.or(defaults.history),
            max_bytes: parse_number(config, MAX_BYTES_KEY)?.or(defaults.max_bytes),
        }
        .validated()
    }

    /// Reject creation settings JetStream would refuse, so a bad value fails
    /// where it was written — host startup, or a workload's bind — instead of
    /// at some guest's first `open`.
    pub fn validated(self) -> anyhow::Result<Self> {
        // JetStream accepts `A-Za-z0-9_-` in a bucket name. A prefix or pin
        // outside that can never name a real bucket, so every open would come
        // back as `no-such-store` — the least informative way possible to
        // learn about a typo in a host flag.
        // An empty pin is not "no pin": it resolves every identifier to the
        // prefix alone, which JetStream rejects, so every open would come back
        // as `no-such-store` and the warning would name a bucket that cannot
        // exist. Leave `bucket` unset to opt out.
        if self.bucket.as_deref() == Some("") {
            anyhow::bail!(
                "keyvalue bucket name is empty; omit `{BUCKET_KEY}` to resolve each identifier \
                 to its own bucket"
            );
        }
        for (key, value) in [
            (BUCKET_PREFIX_KEY, Some(self.prefix.as_str())),
            (BUCKET_KEY, self.bucket.as_deref()),
        ] {
            if let Some(value) = value
                && let Some(bad) = value
                    .chars()
                    .find(|c| !(c.is_ascii_alphanumeric() || *c == '_' || *c == '-'))
            {
                anyhow::bail!(
                    "keyvalue bucket {key} '{value}' contains '{bad}'; JetStream bucket names \
                     allow only letters, digits, '_' and '-'"
                );
            }
        }
        if let Some(replicas) = self.replicas
            && replicas == 0
        {
            anyhow::bail!("keyvalue bucket replicas must be at least 1");
        }
        if let Some(history) = self.history
            && !(1..=MAX_HISTORY).contains(&history)
        {
            anyhow::bail!("keyvalue bucket history must be between 1 and {MAX_HISTORY}");
        }
        if let Some(max_bytes) = self.max_bytes
            && max_bytes < -1
        {
            anyhow::bail!("keyvalue bucket max_bytes must be -1 (unlimited) or greater");
        }
        Ok(self)
    }

    /// A fingerprint of the *resolved* policy, for pool keys.
    ///
    /// Taken after merging with the embedder's defaults, so two interfaces
    /// share a pooled backend only when they truly resolve buckets the same
    /// way — an interface that spells out what it inherits, or one that opts
    /// out of an inherited prefix with an empty string, is not confused with a
    /// neighbour that says nothing.
    pub fn fingerprint(&self) -> String {
        format!(
            "{}\u{1}{}\u{1}{}\u{1}{:?}\u{1}{:?}\u{1}{:?}\u{1}{:?}\u{1}{:?}",
            self.prefix,
            self.bucket.as_deref().unwrap_or("\u{2}"),
            self.create.as_str(),
            self.replicas,
            self.storage.map(|s| format!("{s:?}")),
            self.max_age,
            self.history,
            self.max_bytes,
        )
    }

    /// The physical JetStream bucket an identifier resolves to.
    pub fn physical_name(&self, identifier: &str) -> String {
        match &self.bucket {
            Some(bucket) => format!("{}{bucket}", self.prefix),
            None => format!("{}{identifier}", self.prefix),
        }
    }

    /// The JetStream config a created bucket is given.
    fn kv_config(&self, bucket: String) -> Config {
        let mut config = Config {
            bucket,
            ..Default::default()
        };
        if let Some(replicas) = self.replicas {
            config.num_replicas = replicas;
        }
        if let Some(storage) = self.storage {
            config.storage = storage;
        }
        if let Some(max_age) = self.max_age {
            config.max_age = max_age;
        }
        if let Some(history) = self.history {
            config.history = history;
        }
        if let Some(max_bytes) = self.max_bytes {
            config.max_bytes = max_bytes;
        }
        config
    }

    /// Look up an existing bucket by its physical name, without creating
    /// anything. Every operation after `open` resolves through here, so a
    /// bucket is only ever created by [`BucketPolicy::open`].
    #[instrument(
        name = "wasi.keyvalue.bucket.get",
        level = "debug",
        skip(self, context),
        fields(bucket = %physical)
    )]
    pub async fn get(&self, context: &Context, physical: &str) -> Result<Store, OpenError> {
        use async_nats::jetstream::ErrorCode;
        use async_nats::jetstream::context::{
            GetStreamError, GetStreamErrorKind, KeyValueErrorKind,
        };
        use std::error::Error as _;

        context.get_key_value(physical).await.map_err(|e| {
            let other = || OpenError::Other(format!("JetStream error: {e}"));
            match e.kind() {
                // An invalid name is never a real store.
                KeyValueErrorKind::InvalidStoreName => OpenError::NoSuchStore,
                // `GetBucket` wraps a `get_stream` failure. Only a stream the
                // server reports as missing (or a name it would never accept)
                // is absence. Every other outcome — a transport/timeout
                // `Request` failure, JetStream disabled for the account, an
                // account without `$JS.API.STREAM.INFO` permission — is a real
                // error and must propagate: reporting those as `no-such-store`
                // would tell an operator to go create a bucket when the actual
                // fault is the connection, the account, or its permissions.
                KeyValueErrorKind::GetBucket => {
                    match e.source().and_then(|s| s.downcast_ref::<GetStreamError>()) {
                        Some(g) => match g.kind() {
                            GetStreamErrorKind::EmptyName
                            | GetStreamErrorKind::InvalidStreamName => OpenError::NoSuchStore,
                            GetStreamErrorKind::JetStream(api)
                                if api.error_code() == ErrorCode::STREAM_NOT_FOUND =>
                            {
                                OpenError::NoSuchStore
                            }
                            GetStreamErrorKind::JetStream(_) | GetStreamErrorKind::Request => {
                                other()
                            }
                        },
                        // No `get_stream` failure underneath to classify.
                        None => other(),
                    }
                }
                // A JetStream/transport failure is a real error, not "not found".
                KeyValueErrorKind::JetStream => other(),
            }
        })
    }

    /// Resolve `identifier` to its bucket, creating it when the policy allows
    /// and it does not exist.
    ///
    /// The lookup comes first so an already-existing bucket is opened without
    /// stream-create permission and without re-sending a config that would
    /// conflict with how it was originally created. `create_key_value` is
    /// idempotent for an identical config, so two opens racing to create the
    /// same bucket both succeed.
    ///
    /// The span carries the mapping an operator otherwise cannot see: the
    /// identifier the guest asked for, the physical bucket it resolved to, the
    /// create policy in force, and — once known — whether the bucket already
    /// existed or was created here.
    #[instrument(
        name = "wasi.keyvalue.bucket.open",
        level = "debug",
        skip(self, context),
        fields(
            %identifier,
            bucket = tracing::field::Empty,
            create = self.create.as_str(),
            outcome = tracing::field::Empty,
        )
    )]
    pub async fn open(
        &self,
        context: &Context,
        identifier: &str,
    ) -> Result<(Store, OpenOutcome), OpenError> {
        let span = tracing::Span::current();
        let physical = self.physical_name(identifier);
        span.record("bucket", physical.as_str());

        match self.get(context, &physical).await {
            Ok(store) => {
                span.record("outcome", OpenOutcome::Existing.as_str());
                Ok((store, OpenOutcome::Existing))
            }
            Err(OpenError::NoSuchStore) if self.create == CreatePolicy::Never => {
                span.record("outcome", "refused");
                // A guest may open per request, so warn once per bucket rather
                // than once per call: the operator needs the name and the fix,
                // not a line per request. `wasi_keyvalue_bucket_opens_total`
                // with `outcome = refused` counts the rest.
                if first_refusal(&physical) {
                    tracing::warn!(
                        bucket = %physical,
                        %identifier,
                        "keyvalue bucket does not exist and this host does not create buckets; \
                         create it in JetStream, or start the host with \
                         `--keyvalue-nats-create missing`"
                    );
                } else {
                    tracing::debug!(bucket = %physical, %identifier, "keyvalue bucket refused");
                }
                Err(OpenError::NoSuchStore)
            }
            Err(OpenError::NoSuchStore) => {
                let store = context
                    .create_key_value(self.kv_config(physical.clone()))
                    .await
                    .map_err(|e| {
                        tracing::error!(
                            error = ?e,
                            bucket = %physical,
                            "failed to create keyvalue bucket in JetStream"
                        );
                        OpenError::Other(format!(
                            "failed to create keyvalue bucket in JetStream({physical}): {e}"
                        ))
                    })?;
                span.record("outcome", OpenOutcome::Created.as_str());
                tracing::info!(bucket = %physical, "created keyvalue bucket in JetStream");
                Ok((store, OpenOutcome::Created))
            }
            Err(e) => Err(e),
        }
    }
}

fn parse_number<T: std::str::FromStr>(
    config: &HashMap<String, String>,
    key: &str,
) -> anyhow::Result<Option<T>>
where
    T::Err: std::fmt::Display,
{
    config
        .get(key)
        .map(|v| {
            v.parse::<T>()
                .map_err(|e| anyhow::anyhow!("invalid keyvalue bucket {key} '{v}': {e}"))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// With no config, an identifier is its own bucket name.
    #[test]
    fn identifier_is_the_bucket_by_default() {
        let policy = BucketPolicy::default();
        assert_eq!(policy.physical_name("counters"), "counters");
    }

    /// A prefix namespaces every identifier.
    #[test]
    fn prefix_namespaces_identifiers() {
        let policy = BucketPolicy::from_config(
            &config(&[("bucket_prefix", "team-a_")]),
            &BucketPolicy::default(),
        )
        .expect("must parse");
        assert_eq!(policy.physical_name("counters"), "team-a_counters");
    }

    /// An explicit bucket pins the interface: the guest's identifier no longer
    /// selects the store, and the prefix still applies.
    #[test]
    fn explicit_bucket_pins_and_is_prefixed() {
        let policy = BucketPolicy::from_config(
            &config(&[("bucket", "SESSIONS"), ("bucket_prefix", "prod_")]),
            &BucketPolicy::default(),
        )
        .expect("must parse");
        assert_eq!(policy.physical_name("counters"), "prod_SESSIONS");
        assert_eq!(policy.physical_name(""), "prod_SESSIONS");
    }

    /// The create policy defaults to the caller's default and is overridden by
    /// config.
    #[test]
    fn create_policy_defaults_and_overrides() {
        let policy = BucketPolicy::from_config(&config(&[]), &BucketPolicy::create_missing())
            .expect("must parse");
        assert_eq!(policy.create, CreatePolicy::Missing);

        let policy = BucketPolicy::from_config(
            &config(&[("create", "never")]),
            &BucketPolicy::create_missing(),
        )
        .expect("must parse");
        assert_eq!(policy.create, CreatePolicy::Never);
    }

    /// Creation settings are parsed into their typed forms.
    #[test]
    fn creation_settings_are_parsed() {
        let policy = BucketPolicy::from_config(
            &config(&[
                ("create", "missing"),
                ("replicas", "3"),
                ("storage", "memory"),
                ("max_age", "24h"),
                ("history", "5"),
                ("max_bytes", "1048576"),
            ]),
            // A `missing` ceiling, since `create` may only be tightened.
            &BucketPolicy::create_missing(),
        )
        .expect("must parse");

        assert_eq!(policy.create, CreatePolicy::Missing);
        assert_eq!(policy.replicas, Some(3));
        assert!(matches!(policy.storage, Some(StorageType::Memory)));
        assert_eq!(policy.max_age, Some(Duration::from_secs(24 * 60 * 60)));
        assert_eq!(policy.history, Some(5));
        assert_eq!(policy.max_bytes, Some(1_048_576));

        let kv = policy.kv_config("b".to_string());
        assert_eq!(kv.num_replicas, 3);
        assert_eq!(kv.history, 5);
        assert_eq!(kv.max_bytes, 1_048_576);
        assert_eq!(kv.max_age, Duration::from_secs(24 * 60 * 60));
    }

    /// Unset creation settings leave JetStream's defaults alone.
    #[test]
    fn unset_creation_settings_keep_jetstream_defaults() {
        let policy = BucketPolicy::create_missing();
        let kv = policy.kv_config("b".to_string());
        let defaults = Config {
            bucket: "b".to_string(),
            ..Default::default()
        };
        assert_eq!(kv.num_replicas, defaults.num_replicas);
        assert_eq!(kv.history, defaults.history);
        assert_eq!(kv.max_bytes, defaults.max_bytes);
        assert_eq!(kv.max_age, defaults.max_age);
    }

    /// A bad value is rejected at config time rather than at the first open.
    #[test]
    fn invalid_values_are_rejected() {
        for pair in [
            ("create", "sometimes"),
            ("storage", "tape"),
            ("max_age", "many moons"),
            ("replicas", "three"),
            ("max_bytes", "lots"),
        ] {
            BucketPolicy::from_config(&config(&[pair]), &BucketPolicy::default())
                .expect_err(&format!("{pair:?} must be rejected"));
        }
    }

    /// An interface inherits every key the embedder configured, and overrides
    /// only the ones it sets itself.
    #[test]
    fn interface_config_overrides_defaults_key_by_key() {
        let defaults = BucketPolicy {
            prefix: "host_".to_string(),
            create: CreatePolicy::Missing,
            replicas: Some(3),
            max_age: Some(Duration::from_secs(60)),
            ..BucketPolicy::default()
        };

        let inherited = BucketPolicy::from_config(&config(&[]), &defaults).expect("must parse");
        assert_eq!(inherited.prefix, "host_");
        assert_eq!(inherited.create, CreatePolicy::Missing);
        assert_eq!(inherited.replicas, Some(3));
        assert_eq!(inherited.max_age, Some(Duration::from_secs(60)));

        let overridden = BucketPolicy::from_config(
            &config(&[("bucket_prefix", "iface_"), ("replicas", "1")]),
            &defaults,
        )
        .expect("must parse");
        assert_eq!(overridden.prefix, "iface_");
        assert_eq!(overridden.replicas, Some(1));
        // Untouched keys still come from the defaults.
        assert_eq!(overridden.create, CreatePolicy::Missing);
        assert_eq!(overridden.max_age, Some(Duration::from_secs(60)));
    }

    /// Policies that differ produce different fingerprints, so they never share
    /// a pooled backend; identical ones collapse onto the same key.
    #[test]
    fn fingerprint_separates_distinct_policies() {
        let fingerprint = |pairs: &[(&str, &str)], defaults: &BucketPolicy| {
            BucketPolicy::from_config(&config(pairs), defaults)
                .expect("must parse")
                .fingerprint()
        };
        let none = BucketPolicy::default();

        assert_ne!(
            fingerprint(&[("bucket_prefix", "a_")], &none),
            fingerprint(&[("bucket_prefix", "b_")], &none)
        );
        assert_eq!(
            fingerprint(&[("bucket_prefix", "a_")], &none),
            fingerprint(&[("bucket_prefix", "a_")], &none)
        );

        // A key that is not part of the policy does not split the pool.
        assert_eq!(
            fingerprint(&[("bucket_prefix", "a_")], &none),
            fingerprint(
                &[("bucket_prefix", "a_"), ("url", "nats://127.0.0.1:4222")],
                &none
            )
        );

        // Inheriting a value and spelling it out are the same policy, so they
        // share a backend...
        let inherited = BucketPolicy {
            prefix: "host_".to_string(),
            ..BucketPolicy::default()
        };
        assert_eq!(
            fingerprint(&[], &inherited),
            fingerprint(&[("bucket_prefix", "host_")], &inherited)
        );

        // ...while opting out of an inherited prefix with an empty value is a
        // different policy, and must not be pooled with one that inherits it.
        assert_ne!(
            fingerprint(&[], &inherited),
            fingerprint(&[("bucket_prefix", "")], &inherited)
        );
    }

    /// An interface may tighten the embedder's create policy but never loosen
    /// it: a workload cannot grant itself stream creation the host withheld.
    #[test]
    fn create_policy_is_a_ceiling() {
        let host_allows = BucketPolicy::create_missing();
        let host_withholds = BucketPolicy::default();

        let tightened = BucketPolicy::from_config(&config(&[("create", "never")]), &host_allows)
            .expect("must parse");
        assert_eq!(tightened.create, CreatePolicy::Never);

        let attempted_escalation =
            BucketPolicy::from_config(&config(&[("create", "missing")]), &host_withholds)
                .expect("must parse");
        assert_eq!(attempted_escalation.create, CreatePolicy::Never);
    }

    /// A pin names one store, so it is never inherited: only the interface
    /// that states it is pinned.
    #[test]
    fn pin_is_not_inherited() {
        let pinned_host = BucketPolicy {
            bucket: Some("SHARED".to_string()),
            ..BucketPolicy::default()
        };
        let policy = BucketPolicy::from_config(&config(&[]), &pinned_host).expect("must parse");
        assert_eq!(policy.bucket, None);
        assert_eq!(policy.physical_name("sessions"), "sessions");
    }

    /// An empty pin is refused rather than resolving every identifier to a
    /// name JetStream cannot accept.
    #[test]
    fn an_empty_pin_is_refused() {
        BucketPolicy::from_config(&config(&[("bucket", "")]), &BucketPolicy::default())
            .expect_err("an empty bucket must be rejected");

        // Omitting it is how you opt out, and an empty *prefix* stays valid.
        let policy =
            BucketPolicy::from_config(&config(&[("bucket_prefix", "")]), &BucketPolicy::default())
                .expect("an empty prefix means no prefix");
        assert_eq!(policy.physical_name("counters"), "counters");
    }

    /// A refusal warns once per bucket, however many times a guest opens it.
    #[test]
    fn refusals_warn_once_per_bucket() {
        // Unique names: the bookkeeping is process-wide, so a fixed name would
        // couple this to whatever else has run.
        let a = format!("bucket-a-{}", uuid::Uuid::new_v4());
        let b = format!("bucket-b-{}", uuid::Uuid::new_v4());

        assert!(first_refusal(&a), "the first refusal warns");
        assert!(!first_refusal(&a), "a repeat does not");
        assert!(first_refusal(&b), "a different bucket warns");
    }

    /// The set of remembered buckets is bounded: guests choose those names, so
    /// it must not grow forever.
    #[test]
    fn refusal_bookkeeping_is_bounded() {
        for _ in 0..(REFUSED_BUCKETS_CAP + 10) {
            first_refusal(&format!("bucket-{}", uuid::Uuid::new_v4()));
        }
        let remembered = REFUSED_BUCKETS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len();
        assert!(
            remembered <= REFUSED_BUCKETS_CAP,
            "remembered {remembered} buckets, cap is {REFUSED_BUCKETS_CAP}"
        );
    }

    /// A bucket name JetStream would never accept is refused where it was
    /// written, not turned into a permanent `no-such-store`.
    #[test]
    fn bucket_names_are_checked() {
        for pair in [("bucket_prefix", "my.app_"), ("bucket", "my bucket")] {
            BucketPolicy::from_config(&config(&[pair]), &BucketPolicy::default())
                .expect_err(&format!("{pair:?} must be rejected"));
        }

        BucketPolicy::from_config(
            &config(&[("bucket_prefix", "team-a_"), ("bucket", "APP_STORE")]),
            &BucketPolicy::default(),
        )
        .expect("letters, digits, '_' and '-' must be accepted");
    }

    /// Settings JetStream would refuse fail where they were written.
    #[test]
    fn creation_settings_are_range_checked() {
        for pair in [
            ("replicas", "0"),
            ("history", "0"),
            ("history", "65"),
            ("max_bytes", "-2"),
        ] {
            BucketPolicy::from_config(&config(&[pair]), &BucketPolicy::default())
                .expect_err(&format!("{pair:?} must be rejected"));
        }

        // -1 is JetStream's "unlimited", and the top of the history range is
        // allowed.
        BucketPolicy::from_config(
            &config(&[("max_bytes", "-1"), ("history", "64")]),
            &BucketPolicy::default(),
        )
        .expect("the boundary values must be accepted");
    }
}
