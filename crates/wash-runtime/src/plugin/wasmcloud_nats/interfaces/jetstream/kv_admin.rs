//! `wasmcloud:nats/kv-admin@0.1.4` — KV bucket create, inspect and delete.
//!
//! Gated by `bucket-allow` plus `bucket-create-allow` or `bucket-delete-allow`.
//! A create is further limited by `bucket-max-bytes` (each bucket's size) and
//! `bucket-storage-allow` (file, memory, or both).

use std::time::{Duration, SystemTime};

use async_nats::jetstream;
use async_nats::jetstream::context::{
    AccountError, AccountErrorKind, CreateKeyValueError, CreateKeyValueErrorKind,
    CreateStreamError, CreateStreamErrorKind, GetStreamError, GetStreamErrorKind, KeyValueError,
    KeyValueErrorKind,
};
use wasmtime::component::{Accessor, Resource};

use crate::engine::ctx::SharedCtx;

use super::kv::message_ttl_seconds;
use crate::plugin::wasmcloud_nats::config::BucketStorage;
use crate::plugin::wasmcloud_nats::conn::{ConnHandle, server_at_least};
use crate::plugin::wasmcloud_nats::interfaces::{
    NatsId, bucket_lookup_err, chain_timed_out, denied, kv_admin, kv_err, labeled_kv_admin, types,
};
use crate::plugin::wasmcloud_nats::jetstream::BucketHandle;
use crate::plugin::wasmcloud_nats::policy::PolicyEngine;

use kv_admin::CreateBucketError;

/// The server's cap on revisions kept per key.
const MAX_HISTORY: u8 = 64;

/// Server refusals of a new stream because the account or server is out of
/// streams, storage or memory.
const ACCOUNT_LIMIT_CODES: &[jetstream::ErrorCode] = &[
    jetstream::ErrorCode::ACCOUNT_RESOURCES_EXCEEDED,
    jetstream::ErrorCode::INSUFFICIENT_RESOURCES,
    jetstream::ErrorCode::MAXIMUM_STREAMS_LIMIT,
    jetstream::ErrorCode::MEMORY_RESOURCES_EXCEEDED,
    jetstream::ErrorCode::STORAGE_RESOURCES_EXCEEDED,
    STREAM_MAX_BYTES_REQUIRED,
    NO_LIMITS,
    STREAM_MAX_BYTES_OVER_ACCOUNT,
];

// Account refusals async-nats has no names for (nats-server `errors.json`).
/// The account requires every stream to set `max_bytes`.
const STREAM_MAX_BYTES_REQUIRED: jetstream::ErrorCode = jetstream::ErrorCode(10113);
/// The account has no limit for the stream's replica tier.
const NO_LIMITS: jetstream::ErrorCode = jetstream::ErrorCode(10120);
/// The stream's `max_bytes` is over the account's per-stream limit.
const STREAM_MAX_BYTES_OVER_ACCOUNT: jetstream::ErrorCode = jetstream::ErrorCode(10122);

/// The longest `max-age-ms` the server's signed nanosecond duration holds.
const MAX_AGE_MS: u64 = i64::MAX as u64 / 1_000_000;

impl<T: 'static + Send> labeled_kv_admin::HostWithStore<T> for SharedCtx {
    async fn create_bucket(
        accessor: &Accessor<T, Self>,
        id: NatsId,
        config: kv_admin::BucketConfig,
    ) -> wasmtime::Result<Result<Resource<BucketHandle>, CreateBucketError>> {
        let conn = id;
        let bucket = config.bucket.clone();
        if let Err(reason) = conn.policy.check_bucket_create(&bucket) {
            return Ok(Err(CreateBucketError::Nats(denied(
                reason,
                types::DeniedResource::Bucket,
                &bucket,
            ))));
        }
        let server = conn.server_version();
        // async-nats cannot parse a 2.9 server's account info, which create reads.
        if !server_at_least(&server, (2, 10, 0)) {
            return Ok(Err(CreateBucketError::Nats(
                types::NatsError::UnsupportedByServer(
                    "create-bucket requires NATS server 2.10 or newer".to_string(),
                ),
            )));
        }
        let requested = config.clone();
        let kv_config = match to_kv_config(&conn.policy, &server, config) {
            Ok(c) => c,
            // Over `bucket-max-bytes`. The bucket may exist from before the
            // ceiling was lowered, which makes it a conflict.
            Err(types::NatsError::LimitExceeded(m))
                if conn.policy.bucket_max_bytes(requested.max_bytes).is_err() =>
            {
                return Ok(Err(over_ceiling(&conn, &bucket, m).await));
            }
            Err(e) => return Ok(Err(CreateBucketError::Nats(e))),
        };
        // Create first, so an ensure needs only `STREAM.CREATE`. The create is
        // idempotent and async-nats drops the server's `did_create`, so the
        // returned stream info tells a new bucket from a matching live one.
        let sent = SystemTime::now();
        let store = match conn.jetstream.create_key_value(kv_config).await {
            Ok(store) => {
                if is_new(&store, sent) {
                    tracing::info!(bucket, "wasmcloud:nats created KV bucket");
                } else {
                    tracing::debug!(bucket, "wasmcloud:nats opened existing KV bucket");
                }
                store
            }
            Err(e) => match classify_create_err(&bucket, e) {
                CreateFailure::Conflict => match adopt_existing(&conn, &requested).await {
                    Ok(store) => store,
                    Err(e) => return Ok(Err(e)),
                },
                CreateFailure::Other(e) => return Ok(Err(CreateBucketError::Nats(e))),
            },
        };
        let resource = accessor.with(|mut a| a.get().table.push(BucketHandle { store, conn }))?;
        Ok(Ok(resource))
    }

    async fn get_bucket_config(
        _accessor: &Accessor<T, Self>,
        id: NatsId,
        bucket: String,
    ) -> wasmtime::Result<Result<kv_admin::BucketConfig, types::NatsError>> {
        let conn = id;
        if let Err(reason) = conn.policy.check_bucket(&bucket) {
            return Ok(Err(denied(reason, types::DeniedResource::Bucket, &bucket)));
        }
        Ok(read_config(&conn, &bucket).await)
    }

    async fn delete_bucket(
        _accessor: &Accessor<T, Self>,
        id: NatsId,
        bucket: String,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let conn = id;
        if let Err(reason) = conn.policy.check_bucket_delete(&bucket) {
            return Ok(Err(denied(reason, types::DeniedResource::Bucket, &bucket)));
        }
        match conn.jetstream.delete_key_value(&bucket).await {
            Ok(_) => {
                tracing::info!(bucket, "wasmcloud:nats deleted KV bucket");
                Ok(Ok(()))
            }
            Err(e) => Ok(Err(delete_err(&bucket, e))),
        }
    }
}

impl labeled_kv_admin::Host for crate::engine::ctx::ActiveCtx<'_> {}
impl kv_admin::Host for crate::engine::ctx::ActiveCtx<'_> {}

async fn read_config(
    conn: &ConnHandle,
    bucket: &str,
) -> Result<kv_admin::BucketConfig, types::NatsError> {
    // `get_key_value` fetched the stream info, so the cached copy is fresh.
    let store = conn
        .jetstream
        .get_key_value(bucket)
        .await
        .map_err(|e| bucket_lookup_err(bucket, e))?;
    Ok(from_stream_config(
        store.name.clone(),
        &store.stream.cached_info().config,
    ))
}

/// How far the server's clock may trail the host's before a bucket it has
/// just created looks older than the request.
const CLOCK_SKEW: Duration = Duration::from_secs(5);

/// Whether a successful create made the bucket rather than matched a live one.
///
/// A bucket that has held a message is not new. An empty one is new when the
/// server created it after `sent`, within [`CLOCK_SKEW`], so an empty bucket
/// ensured again inside that window still logs as created.
fn is_new(store: &jetstream::kv::Store, sent: SystemTime) -> bool {
    let info = store.stream.cached_info();
    info.state.last_sequence == 0 && SystemTime::from(info.created) + CLOCK_SKEW >= sent
}

/// A create that hit an existing bucket: opens it when it matches what the
/// guest asked for, otherwise hands back what it has.
async fn adopt_existing(
    conn: &ConnHandle,
    requested: &kv_admin::BucketConfig,
) -> Result<jetstream::kv::Store, CreateBucketError> {
    let bucket = &requested.bucket;
    // A timeout stays a timeout, and `not-found` means it was deleted since
    // the create saw it, so a retry creates it.
    let store = conn
        .jetstream
        .get_key_value(bucket)
        .await
        .map_err(|e| CreateBucketError::Nats(bucket_lookup_err(bucket, e)))?;
    let info = store.stream.cached_info();
    let existing = from_stream_config(store.name.clone(), &info.config);
    if !matches_request(&conn.policy, requested, &existing) {
        return Err(CreateBucketError::ConfigConflict(existing));
    }
    let replicas = info.config.num_replicas.max(1);
    if let Some(wanted) = conn.policy.bucket_replicas()
        && wanted != replicas
    {
        tracing::info!(
            bucket,
            replicas,
            bucket_replicas = wanted,
            "wasmcloud:nats opened an existing KV bucket whose replica count differs from \
             `bucket-replicas`; it keeps the layout it was created with"
        );
    }
    Ok(store)
}

/// A create over `bucket-max-bytes`: `config-conflict` with what the bucket
/// has when it exists, `limit-exceeded` when it does not.
async fn over_ceiling(conn: &ConnHandle, bucket: &str, limit: String) -> CreateBucketError {
    match conn.jetstream.get_key_value(bucket).await {
        Ok(store) => CreateBucketError::ConfigConflict(from_stream_config(
            store.name.clone(),
            &store.stream.cached_info().config,
        )),
        Err(e) => match bucket_lookup_err(bucket, e) {
            types::NatsError::NotFound(_) => {
                CreateBucketError::Nats(types::NatsError::LimitExceeded(limit))
            }
            other => CreateBucketError::Nats(other),
        },
    }
}

/// Whether an existing bucket is what `requested` asks for.
///
/// Every field the guest set must match. `max-bytes: 0` asks for the host's
/// choice, so any size within the current `bucket-max-bytes` matches, and any
/// size at all when there is none. Replicas are the host's and never compared:
/// a bucket keeps the layout it was created with.
fn matches_request(
    policy: &PolicyEngine,
    requested: &kv_admin::BucketConfig,
    existing: &kv_admin::BucketConfig,
) -> bool {
    let size = match (requested.max_bytes, policy.bucket_max_bytes(0)) {
        (0, Ok(0)) => true,
        (0, Ok(ceiling)) => existing.max_bytes != 0 && existing.max_bytes <= ceiling,
        (n, _) => existing.max_bytes == n,
    };
    let marker = requested
        .limit_marker_ttl_ms
        .and_then(message_ttl_seconds)
        .map(|s| s * 1_000);
    size && existing.history == requested.history.max(1)
        && existing.max_age_ms == requested.max_age_ms
        && existing.max_value_size == requested.max_value_size
        && existing.storage == requested.storage
        && existing.limit_marker_ttl_ms == marker
}

/// Validates a guest's request and applies the host's size ceiling and
/// replica count. `server` is the connected server's version.
fn to_kv_config(
    policy: &PolicyEngine,
    server: &str,
    config: kv_admin::BucketConfig,
) -> Result<jetstream::kv::Config, types::NatsError> {
    let invalid = |msg: String| {
        types::NatsError::Jetstream(format!("create bucket '{}': {msg}", config.bucket))
    };
    if config.history > MAX_HISTORY {
        return Err(invalid(format!(
            "history {} is over the server's maximum of {MAX_HISTORY}",
            config.history
        )));
    }
    if config.max_age_ms > MAX_AGE_MS {
        return Err(invalid(format!(
            "max-age-ms {} is over {MAX_AGE_MS}",
            config.max_age_ms
        )));
    }
    let max_value_size = match config.max_value_size {
        0 => -1,
        n => i32::try_from(n)
            .map_err(|_| invalid(format!("max-value-size {n} is over {}", i32::MAX)))?,
    };
    let max_bytes = match policy.bucket_max_bytes(config.max_bytes) {
        Ok(0) => -1,
        Ok(n) => {
            i64::try_from(n).map_err(|_| invalid(format!("max-bytes {n} is over {}", i64::MAX)))?
        }
        Err(ceiling) => {
            return Err(types::NatsError::LimitExceeded(format!(
                "create bucket '{}': max-bytes {} is over this binding's bucket-max-bytes \
                 ({ceiling})",
                config.bucket, config.max_bytes
            )));
        }
    };
    let storage = match config.storage {
        kv_admin::StorageType::File => BucketStorage::File,
        kv_admin::StorageType::Memory => BucketStorage::Memory,
    };
    if let Err(allowed) = policy.check_bucket_storage(storage) {
        return Err(types::NatsError::LimitExceeded(format!(
            "create bucket '{}': {} storage is not in this binding's bucket-storage-allow \
             ({allowed})",
            config.bucket,
            storage.as_str()
        )));
    }
    let limit_markers = match config.limit_marker_ttl_ms {
        None => None,
        Some(_) if !server_at_least(server, (2, 11, 0)) => {
            return Err(types::NatsError::UnsupportedByServer(
                "limit-marker-ttl-ms (per-key TTL) requires NATS server 2.11 or newer".to_string(),
            ));
        }
        Some(ms) => Some(Duration::from_secs(message_ttl_seconds(ms).ok_or_else(
            || invalid(format!("limit-marker-ttl-ms {ms} must be at least 1000")),
        )?)),
    };
    Ok(jetstream::kv::Config {
        bucket: config.bucket,
        history: i64::from(config.history.max(1)),
        max_age: Duration::from_millis(config.max_age_ms),
        max_bytes,
        max_value_size,
        storage: match storage {
            BucketStorage::File => jetstream::stream::StorageType::File,
            BucketStorage::Memory => jetstream::stream::StorageType::Memory,
        },
        limit_markers,
        // 0 lets async-nats ask for the server's default of one.
        num_replicas: policy.bucket_replicas().unwrap_or(0),
        ..Default::default()
    })
}

/// Reads a bucket's backing stream config back into the WIT shape.
fn from_stream_config(bucket: String, c: &jetstream::stream::Config) -> kv_admin::BucketConfig {
    kv_admin::BucketConfig {
        bucket,
        history: c.max_messages_per_subject.clamp(0, u8::MAX as i64) as u8,
        max_age_ms: c.max_age.as_millis().try_into().unwrap_or(u64::MAX),
        max_bytes: c.max_bytes.max(0) as u64,
        max_value_size: c.max_message_size.max(0) as u32,
        storage: match c.storage {
            jetstream::stream::StorageType::File => kv_admin::StorageType::File,
            jetstream::stream::StorageType::Memory => kv_admin::StorageType::Memory,
        },
        limit_marker_ttl_ms: c
            .subject_delete_marker_ttl
            .filter(|_| c.allow_message_ttl)
            .map(|d| d.as_millis().try_into().unwrap_or(u64::MAX)),
    }
}

/// Finds the first error of type `E` in `e`'s source chain.
fn find_in_chain<'a, E: std::error::Error + 'static>(
    e: &'a (dyn std::error::Error + 'static),
) -> Option<&'a E> {
    let mut current = Some(e);
    while let Some(err) = current {
        if let Some(found) = err.downcast_ref::<E>() {
            return Some(found);
        }
        current = err.source();
    }
    None
}

/// What a failed create means to the guest.
#[derive(Debug)]
enum CreateFailure {
    /// The bucket exists with a different configuration.
    Conflict,
    Other(types::NatsError),
}

fn classify_create_err(bucket: &str, e: CreateKeyValueError) -> CreateFailure {
    let what = format!("create bucket '{bucket}'");
    CreateFailure::Other(match e.kind() {
        CreateKeyValueErrorKind::TimedOut => types::NatsError::Timeout(format!("{what}: {e}")),
        CreateKeyValueErrorKind::LimitMarkersNotSupported => types::NatsError::UnsupportedByServer(
            format!("{what}: limit markers need NATS server 2.11 or newer: {e}"),
        ),
        CreateKeyValueErrorKind::BucketCreate => {
            match find_in_chain::<CreateStreamError>(&e).map(|s| s.kind()) {
                Some(CreateStreamErrorKind::JetStream(err))
                    if err.error_code() == jetstream::ErrorCode::STREAM_NAME_EXIST =>
                {
                    return CreateFailure::Conflict;
                }
                Some(CreateStreamErrorKind::TimedOut) => {
                    types::NatsError::Timeout(format!("{what}: {e}"))
                }
                Some(CreateStreamErrorKind::JetStream(err))
                    if ACCOUNT_LIMIT_CODES.contains(&err.error_code()) =>
                {
                    types::NatsError::LimitExceeded(format!(
                        "{what}: refused by the JetStream account's limits: {e}"
                    ))
                }
                _ => kv_err(&what, chain_timed_out(&e), &e),
            }
        }
        // Create reads the account first; its timeout carries no source, so
        // `chain_timed_out` cannot see it.
        CreateKeyValueErrorKind::JetStream => {
            match find_in_chain::<AccountError>(&e).map(|a| a.kind()) {
                Some(AccountErrorKind::TimedOut) => {
                    types::NatsError::Timeout(format!("{what}: {e}"))
                }
                Some(AccountErrorKind::JetStreamUnavailable) => {
                    types::NatsError::Connection(format!(
                        "{what}: JetStream API unavailable (JetStream disabled or not responding)"
                    ))
                }
                _ => kv_err(&what, chain_timed_out(&e), &e),
            }
        }
        CreateKeyValueErrorKind::InvalidStoreName | CreateKeyValueErrorKind::TooLongHistory => {
            kv_err(&what, chain_timed_out(&e), &e)
        }
    })
}

fn delete_err(bucket: &str, e: KeyValueError) -> types::NatsError {
    let what = format!("delete bucket '{bucket}'");
    // An invalid name names nothing that can exist.
    if e.kind() == KeyValueErrorKind::InvalidStoreName {
        return types::NatsError::NotFound(format!("{what}: {e}"));
    }
    match find_in_chain::<GetStreamError>(&e).map(|s| s.kind()) {
        Some(GetStreamErrorKind::JetStream(err))
            if err.error_code() == jetstream::ErrorCode::STREAM_NOT_FOUND =>
        {
            types::NatsError::NotFound(format!("{what}: {e}"))
        }
        _ => kv_err(&what, chain_timed_out(&e), &e),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::plugin::wasmcloud_nats::config::PolicySpec;

    const MODERN: &str = "2.12.0";

    fn stream_config() -> jetstream::stream::Config {
        jetstream::stream::Config {
            max_messages_per_subject: 5,
            max_age: Duration::from_secs(60),
            max_bytes: -1,
            max_message_size: -1,
            storage: jetstream::stream::StorageType::Memory,
            ..Default::default()
        }
    }

    fn request() -> kv_admin::BucketConfig {
        kv_admin::BucketConfig {
            bucket: "cache".into(),
            history: 1,
            max_age_ms: 0,
            max_bytes: 0,
            max_value_size: 0,
            storage: kv_admin::StorageType::Memory,
            limit_marker_ttl_ms: None,
        }
    }

    fn policy(max_bytes: Option<u64>, replicas: Option<usize>) -> PolicyEngine {
        PolicyEngine::new(
            &PolicySpec {
                bucket_max_bytes: max_bytes,
                bucket_replicas: replicas,
                bucket_storage_allow: vec![BucketStorage::File, BucketStorage::Memory],
                ..Default::default()
            },
            Vec::new(),
        )
    }

    fn convert(
        policy: &PolicyEngine,
        server: &str,
        edit: impl FnOnce(&mut kv_admin::BucketConfig),
    ) -> Result<jetstream::kv::Config, types::NatsError> {
        let mut config = request();
        edit(&mut config);
        to_kv_config(policy, server, config)
    }

    fn rejected(result: Result<jetstream::kv::Config, types::NatsError>, needle: &str) {
        match result {
            Err(types::NatsError::Jetstream(m)) => assert!(m.contains(needle), "{m}"),
            other => panic!("expected a jetstream rejection naming {needle}, got {other:?}"),
        }
    }

    #[test]
    fn unlimited_server_values_read_back_as_zero() {
        let c = from_stream_config("cache".into(), &stream_config());
        assert_eq!(c.history, 5);
        assert_eq!(c.max_age_ms, 60_000);
        assert_eq!(c.max_bytes, 0);
        assert_eq!(c.max_value_size, 0);
        assert!(matches!(c.storage, kv_admin::StorageType::Memory));
        assert_eq!(c.limit_marker_ttl_ms, None);
    }

    #[test]
    fn limits_and_file_storage_read_back() {
        let s = jetstream::stream::Config {
            max_messages_per_subject: 1_000,
            max_bytes: 4096,
            max_message_size: 512,
            storage: jetstream::stream::StorageType::File,
            ..stream_config()
        };
        let c = from_stream_config("cache".into(), &s);
        // A bucket made out of band can keep more history than a u8 says.
        assert_eq!(c.history, u8::MAX);
        assert_eq!(c.max_bytes, 4096);
        assert_eq!(c.max_value_size, 512);
        assert!(matches!(c.storage, kv_admin::StorageType::File));
    }

    #[test]
    fn marker_ttl_reads_back_only_when_message_ttl_is_allowed() {
        let mut s = stream_config();
        s.subject_delete_marker_ttl = Some(Duration::from_secs(5));
        assert_eq!(
            from_stream_config("cache".into(), &s).limit_marker_ttl_ms,
            None
        );
        s.allow_message_ttl = true;
        assert_eq!(
            from_stream_config("cache".into(), &s).limit_marker_ttl_ms,
            Some(5_000)
        );
    }

    #[test]
    fn a_plain_request_maps_onto_the_kv_config() {
        let kv = convert(&policy(None, None), MODERN, |c| {
            c.history = 0;
            c.max_age_ms = 90_000;
            c.max_value_size = 1024;
            c.storage = kv_admin::StorageType::File;
        })
        .unwrap();
        assert_eq!(kv.bucket, "cache");
        assert_eq!(kv.history, 1, "history 0 means 1");
        assert_eq!(kv.max_age, Duration::from_secs(90));
        // No ceiling and 0 requested: unlimited.
        assert_eq!(kv.max_bytes, -1);
        assert_eq!(kv.max_value_size, 1024);
        assert_eq!(kv.storage, jetstream::stream::StorageType::File);
        assert_eq!(kv.limit_markers, None);
        assert_eq!(kv.num_replicas, 0, "unset leaves the server's default");

        let kv = convert(&policy(None, None), MODERN, |c| c.max_bytes = 2048).unwrap();
        assert_eq!(kv.max_bytes, 2048);
        assert_eq!(kv.max_value_size, -1, "0 is unlimited");
        assert_eq!(kv.storage, jetstream::stream::StorageType::Memory);
    }

    #[test]
    fn the_host_sets_size_and_replicas() {
        let host = policy(Some(1 << 20), Some(3));
        let kv = convert(&host, MODERN, |_| {}).unwrap();
        assert_eq!(kv.max_bytes, 1 << 20, "0 takes the ceiling");
        assert_eq!(kv.num_replicas, 3);
        assert_eq!(
            convert(&host, MODERN, |c| c.max_bytes = 1 << 20)
                .unwrap()
                .max_bytes,
            1 << 20
        );
        match convert(&host, MODERN, |c| c.max_bytes = (1 << 20) + 1) {
            Err(types::NatsError::LimitExceeded(m)) => {
                assert!(m.contains("bucket-max-bytes"), "{m}")
            }
            other => panic!("expected limit-exceeded, got {other:?}"),
        }
    }

    #[test]
    fn storage_must_be_one_the_host_allows() {
        let file_only = PolicyEngine::new(
            &PolicySpec {
                bucket_storage_allow: vec![BucketStorage::File],
                ..Default::default()
            },
            Vec::new(),
        );
        let mut request = request();
        request.storage = kv_admin::StorageType::Memory;
        match to_kv_config(&file_only, MODERN, request.clone()) {
            Err(types::NatsError::LimitExceeded(m)) => {
                assert!(m.contains("memory storage") && m.contains("(file)"), "{m}")
            }
            other => panic!("expected memory to be refused, got {other:?}"),
        }
        request.storage = kv_admin::StorageType::File;
        let config = to_kv_config(&file_only, MODERN, request).unwrap();
        assert_eq!(config.storage, jetstream::stream::StorageType::File);
    }

    #[test]
    fn history_is_capped_at_the_servers_maximum() {
        let p = policy(None, None);
        assert_eq!(
            convert(&p, MODERN, |c| c.history = MAX_HISTORY)
                .unwrap()
                .history,
            64
        );
        rejected(
            convert(&p, MODERN, |c| c.history = MAX_HISTORY + 1),
            "history 65",
        );
    }

    #[test]
    fn sizes_must_fit_the_servers_signed_fields() {
        let p = policy(None, None);
        assert_eq!(
            convert(&p, MODERN, |c| c.max_value_size = i32::MAX as u32)
                .unwrap()
                .max_value_size,
            i32::MAX
        );
        rejected(
            convert(&p, MODERN, |c| c.max_value_size = i32::MAX as u32 + 1),
            "max-value-size",
        );
        assert_eq!(
            convert(&p, MODERN, |c| c.max_bytes = i64::MAX as u64)
                .unwrap()
                .max_bytes,
            i64::MAX
        );
        rejected(
            convert(&p, MODERN, |c| c.max_bytes = i64::MAX as u64 + 1),
            "max-bytes",
        );
    }

    /// Past this the nanosecond duration wraps, and a huge age would arrive
    /// at the server as a tiny or negative one.
    #[test]
    fn max_age_must_fit_the_servers_nanosecond_duration() {
        let p = policy(None, None);
        let longest = convert(&p, MODERN, |c| c.max_age_ms = MAX_AGE_MS).unwrap();
        assert!(i64::try_from(longest.max_age.as_nanos()).is_ok());
        rejected(
            convert(&p, MODERN, |c| c.max_age_ms = MAX_AGE_MS + 1),
            "max-age-ms",
        );
        rejected(
            convert(&p, MODERN, |c| c.max_age_ms = u64::MAX),
            "max-age-ms",
        );
    }

    #[test]
    fn marker_ttl_is_rounded_up_and_needs_2_11() {
        let p = policy(None, None);
        let kv = convert(&p, MODERN, |c| c.limit_marker_ttl_ms = Some(1_500)).unwrap();
        assert_eq!(kv.limit_markers, Some(Duration::from_secs(2)));
        rejected(
            convert(&p, MODERN, |c| c.limit_marker_ttl_ms = Some(999)),
            "at least 1000",
        );
        assert!(matches!(
            convert(&p, "2.10.29", |c| c.limit_marker_ttl_ms = Some(1_500)),
            Err(types::NatsError::UnsupportedByServer(_))
        ));
        // Without markers a 2.10 server is fine.
        assert!(convert(&p, "2.10.29", |_| {}).is_ok());
    }

    #[test]
    fn an_existing_bucket_matches_on_every_field_the_guest_set() {
        let p = policy(None, None);
        let existing = kv_admin::BucketConfig {
            history: 3,
            max_age_ms: 60_000,
            max_bytes: 4096,
            max_value_size: 512,
            limit_marker_ttl_ms: Some(2_000),
            ..request()
        };
        let asked = kv_admin::BucketConfig {
            limit_marker_ttl_ms: Some(1_500),
            ..existing.clone()
        };
        assert!(
            matches_request(&p, &asked, &existing),
            "1500ms rounds to 2000"
        );
        let differs: [fn(&mut kv_admin::BucketConfig); 7] = [
            |c| c.history = 4,
            |c| c.max_age_ms = 0,
            |c| c.max_bytes = 8192,
            |c| c.max_value_size = 0,
            |c| c.storage = kv_admin::StorageType::File,
            |c| c.limit_marker_ttl_ms = None,
            |c| c.limit_marker_ttl_ms = Some(3_000),
        ];
        for (i, edit) in differs.into_iter().enumerate() {
            let mut other = asked.clone();
            edit(&mut other);
            assert!(!matches_request(&p, &other, &existing), "edit {i}");
        }
        // History 0 means 1.
        let one = kv_admin::BucketConfig {
            history: 1,
            ..existing.clone()
        };
        assert!(matches_request(
            &p,
            &kv_admin::BucketConfig {
                history: 0,
                ..one.clone()
            },
            &one
        ));
    }

    /// `max-bytes: 0` leaves the size to the host, so it survives the host
    /// changing its ceiling, as long as the bucket still fits under it.
    #[test]
    fn a_host_chosen_size_matches_anything_within_the_ceiling() {
        let asked = request();
        let sized = |max_bytes| kv_admin::BucketConfig {
            max_bytes,
            ..request()
        };
        let none = policy(None, None);
        assert!(matches_request(&none, &asked, &sized(0)));
        assert!(matches_request(&none, &asked, &sized(1 << 20)));

        let capped = policy(Some(2 << 20), None);
        assert!(matches_request(&capped, &asked, &sized(1 << 20)));
        assert!(matches_request(&capped, &asked, &sized(2 << 20)));
        assert!(!matches_request(&capped, &asked, &sized(3 << 20)));
        assert!(
            !matches_request(&capped, &asked, &sized(0)),
            "an unlimited bucket is over any ceiling"
        );
    }

    #[test]
    fn replicas_never_decide_a_match() {
        let asked = request();
        assert!(matches_request(&policy(None, Some(3)), &asked, &request()));
    }

    fn api_error(err_code: u64) -> jetstream::Error {
        serde_json::from_value(serde_json::json!({
            "code": 400,
            "err_code": err_code,
            "description": "from the server",
        }))
        .unwrap()
    }

    fn bucket_create(kind: CreateStreamErrorKind) -> CreateKeyValueError {
        CreateKeyValueError::with_source(
            CreateKeyValueErrorKind::BucketCreate,
            CreateStreamError::new(kind),
        )
    }

    #[test]
    fn create_errors_classify_by_cause() {
        let conflict = bucket_create(CreateStreamErrorKind::JetStream(api_error(
            jetstream::ErrorCode::STREAM_NAME_EXIST.0,
        )));
        assert!(matches!(
            classify_create_err("cache", conflict),
            CreateFailure::Conflict
        ));

        // Keep the expected server codes independent of the classifier's list,
        // so removing an account refusal from that list fails this contract.
        for (name, code) in [
            ("account resources", 10002),
            ("server resources", 10023),
            ("stream count", 10027),
            ("memory storage", 10028),
            ("file storage", 10047),
            ("required max bytes", 10113),
            ("replica tier", 10120),
            ("per-stream bytes", 10122),
        ] {
            let quota = bucket_create(CreateStreamErrorKind::JetStream(api_error(code)));
            assert!(
                matches!(
                    classify_create_err("cache", quota),
                    CreateFailure::Other(types::NatsError::LimitExceeded(_))
                ),
                "{name} ({code})"
            );
        }

        let other_api = bucket_create(CreateStreamErrorKind::JetStream(api_error(10074)));
        assert!(matches!(
            classify_create_err("cache", other_api),
            CreateFailure::Other(types::NatsError::Jetstream(_))
        ));

        for timed_out in [
            CreateKeyValueError::new(CreateKeyValueErrorKind::TimedOut),
            bucket_create(CreateStreamErrorKind::TimedOut),
        ] {
            assert!(matches!(
                classify_create_err("cache", timed_out),
                CreateFailure::Other(types::NatsError::Timeout(_))
            ));
        }

        assert!(matches!(
            classify_create_err(
                "cache",
                CreateKeyValueError::new(CreateKeyValueErrorKind::LimitMarkersNotSupported)
            ),
            CreateFailure::Other(types::NatsError::UnsupportedByServer(_))
        ));

        // The account lookup create starts with.
        let account = |kind| {
            CreateKeyValueError::with_source(
                CreateKeyValueErrorKind::JetStream,
                AccountError::new(kind),
            )
        };
        assert!(matches!(
            classify_create_err("cache", account(AccountErrorKind::TimedOut)),
            CreateFailure::Other(types::NatsError::Timeout(_))
        ));
        assert!(matches!(
            classify_create_err("cache", account(AccountErrorKind::JetStreamUnavailable)),
            CreateFailure::Other(types::NatsError::Connection(_))
        ));
        assert!(matches!(
            classify_create_err("cache", account(AccountErrorKind::Other)),
            CreateFailure::Other(types::NatsError::Jetstream(_))
        ));

        for kind in [
            CreateKeyValueErrorKind::InvalidStoreName,
            CreateKeyValueErrorKind::TooLongHistory,
            CreateKeyValueErrorKind::JetStream,
        ] {
            match classify_create_err("cache", CreateKeyValueError::new(kind)) {
                CreateFailure::Other(types::NatsError::Jetstream(m)) => {
                    assert!(m.contains("create bucket 'cache'"), "{m}")
                }
                other => panic!("{kind:?}: expected jetstream, got {other:?}"),
            }
        }
    }

    #[test]
    fn delete_errors_classify_by_cause() {
        let missing = KeyValueError::with_source(
            KeyValueErrorKind::JetStream,
            GetStreamError::new(GetStreamErrorKind::JetStream(api_error(
                jetstream::ErrorCode::STREAM_NOT_FOUND.0,
            ))),
        );
        assert!(matches!(
            delete_err("cache", missing),
            types::NatsError::NotFound(_)
        ));
        assert!(matches!(
            delete_err(
                "cache",
                KeyValueError::new(KeyValueErrorKind::InvalidStoreName)
            ),
            types::NatsError::NotFound(_)
        ));
        let refused = KeyValueError::with_source(
            KeyValueErrorKind::JetStream,
            GetStreamError::new(GetStreamErrorKind::JetStream(api_error(10039))),
        );
        assert!(matches!(
            delete_err("cache", refused),
            types::NatsError::Jetstream(_)
        ));
    }
}
