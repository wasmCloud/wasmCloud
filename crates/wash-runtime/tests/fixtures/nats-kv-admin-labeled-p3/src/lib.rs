//! `wasmcloud:nats/kv-admin` imported under the `tenant` label.
//!
//! A message names a bucket. The component creates it through the label and
//! writes `status=created` through the returned handle, whose methods belong
//! to the plain `kv` import.

wit_bindgen::generate!({
    path: "wit",
    world: "nats-kv-admin-labeled-p3",
    generate_all,
});

use exports::wasmcloud::nats::core_handler::Guest as CoreGuest;
use wasi::clocks::monotonic_clock;
use wasmcloud::nats::types::NatsMessage;

/// Keeps the component genuinely P3: a handler that awaits no `@0.3` import
/// lets that import tree-shake away.
const YIELD_NANOS: u64 = 1_000_000;

struct Component;

impl CoreGuest for Component {
    async fn handle_message(msg: NatsMessage) -> Result<(), String> {
        monotonic_clock::wait_for(YIELD_NANOS).await;
        let bucket = String::from_utf8_lossy(&msg.body).to_string();
        let handle = tenant::create_bucket(tenant::BucketConfig {
            bucket,
            history: 1,
            max_age_ms: 0,
            max_bytes: 0,
            max_value_size: 0,
            storage: tenant::StorageType::Memory,
            limit_marker_ttl_ms: None,
        })
        .await
        .map_err(|e| format!("create failed: {e:?}"))?;
        handle
            .put("status".to_string(), b"created".to_vec())
            .await
            .map(|_| ())
            .map_err(|e| format!("put failed: {e:?}"))
    }
}

export!(Component);
