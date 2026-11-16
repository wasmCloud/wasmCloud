//! `wasmcloud:nats/kv-admin@0.1.4` — KV bucket create, inspect and delete.
//!
//! Gated by `bucket-allow` plus `bucket-create-allow` or `bucket-delete-allow`,
//! with `bucket-max-bytes` capping what a create may ask for.

use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::context::{
    CreateKeyValueError, CreateKeyValueErrorKind, CreateStreamError, CreateStreamErrorKind,
    GetStreamError, GetStreamErrorKind, KeyValueError, KeyValueErrorKind,
};
use wasmtime::component::{Accessor, Resource};

use crate::engine::ctx::SharedCtx;

use super::kv::message_ttl_seconds;
use crate::plugin::wasmcloud_nats::conn::ConnHandle;
use crate::plugin::wasmcloud_nats::interfaces::{
    NatsId, bucket_lookup_err, chain_timed_out, denied, kv_admin, kv_err, labeled_kv_admin, types,
};
use crate::plugin::wasmcloud_nats::jetstream::BucketHandle;

/// The server's cap on revisions kept per key.
const MAX_HISTORY: u8 = 64;

impl<T: 'static + Send> labeled_kv_admin::HostWithStore<T> for SharedCtx {
    async fn create_bucket(
        accessor: &Accessor<T, Self>,
        id: NatsId,
        config: kv_admin::BucketConfig,
    ) -> wasmtime::Result<Result<Resource<BucketHandle>, types::NatsError>> {
        let conn = id;
        let bucket = config.bucket.clone();
        if let Err(reason) = conn.policy.check_bucket_create(&bucket) {
            return Ok(Err(denied(reason, types::DeniedResource::Bucket, &bucket)));
        }
        // async-nats cannot parse a 2.9 server's account info, which create reads.
        if !conn.server_at_least(2, 10) {
            return Ok(Err(types::NatsError::UnsupportedByServer(
                "create-bucket requires NATS server 2.10 or newer".to_string(),
            )));
        }
        let kv_config = match to_kv_config(&conn, config) {
            Ok(c) => c,
            Err(e) => return Ok(Err(e)),
        };
        let store = match conn.jetstream.create_key_value(kv_config).await {
            Ok(store) => store,
            Err(e) => return Ok(Err(create_err(&bucket, e))),
        };
        tracing::info!(bucket, "wasmcloud:nats created KV bucket");
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
        // `get_key_value` fetched the stream info, so the cached copy is fresh.
        let store = match conn.jetstream.get_key_value(&bucket).await {
            Ok(store) => store,
            Err(e) => return Ok(Err(bucket_lookup_err(&bucket, e))),
        };
        Ok(Ok(from_stream_config(
            store.name.clone(),
            &store.stream.cached_info().config,
        )))
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

/// Validates a guest's request and applies the host's size ceiling.
fn to_kv_config(
    conn: &ConnHandle,
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
    let max_value_size = match config.max_value_size {
        0 => -1,
        n => i32::try_from(n)
            .map_err(|_| invalid(format!("max-value-size {n} is over {}", i32::MAX)))?,
    };
    let max_bytes = match conn.policy.bucket_max_bytes(config.max_bytes) {
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
    let limit_markers = match config.limit_marker_ttl_ms {
        None => None,
        Some(_) if !conn.server_at_least(2, 11) => {
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
        storage: match config.storage {
            kv_admin::StorageType::File => jetstream::stream::StorageType::File,
            kv_admin::StorageType::Memory => jetstream::stream::StorageType::Memory,
        },
        limit_markers,
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

fn create_err(bucket: &str, e: CreateKeyValueError) -> types::NatsError {
    let what = format!("create bucket '{bucket}'");
    match e.kind() {
        CreateKeyValueErrorKind::TimedOut => types::NatsError::Timeout(format!("{what}: {e}")),
        CreateKeyValueErrorKind::LimitMarkersNotSupported => types::NatsError::UnsupportedByServer(
            format!("{what}: limit markers need NATS server 2.11 or newer: {e}"),
        ),
        CreateKeyValueErrorKind::BucketCreate => {
            let stream = find_in_chain::<CreateStreamError>(&e);
            match stream.map(|s| s.kind()) {
                Some(CreateStreamErrorKind::JetStream(err))
                    if err.error_code() == jetstream::ErrorCode::STREAM_NAME_EXIST =>
                {
                    types::NatsError::Jetstream(format!(
                        "{what}: the bucket already exists with a different configuration; \
                         read it with get-bucket-config"
                    ))
                }
                Some(CreateStreamErrorKind::TimedOut) => {
                    types::NatsError::Timeout(format!("{what}: {e}"))
                }
                _ => kv_err(&what, chain_timed_out(&e), &e),
            }
        }
        CreateKeyValueErrorKind::InvalidStoreName
        | CreateKeyValueErrorKind::TooLongHistory
        | CreateKeyValueErrorKind::JetStream => kv_err(&what, chain_timed_out(&e), &e),
    }
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
}
