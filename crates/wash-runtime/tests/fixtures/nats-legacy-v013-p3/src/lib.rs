//! A guest built against `wasmcloud:nats@0.1.3`.
//!
//! A message names a granted bucket. The component reads a missing key and
//! opens an ungranted bucket, then writes what the two errors were to the
//! granted bucket's `status` key, so the test sees `nats-error` cross the
//! version boundary intact.

wit_bindgen::generate!({
    path: "wit",
    world: "nats-legacy-v013-p3",
    generate_all,
});

use exports::wasmcloud::nats::core_handler::Guest as CoreGuest;
use wasi::clocks::monotonic_clock;
use wasmcloud::nats::kv;
use wasmcloud::nats::types::{NatsError, NatsMessage};

/// Keeps the component genuinely P3: a handler that awaits no `@0.3` import
/// lets that import tree-shake away.
const YIELD_NANOS: u64 = 1_000_000;

fn describe<T>(result: Result<T, NatsError>) -> String {
    match result {
        Ok(_) => "ok".to_string(),
        Err(NatsError::KeyNotFound) => "key-not-found".to_string(),
        Err(NatsError::Denied(d)) => format!("denied:{}", d.name),
        Err(e) => format!("other:{e:?}"),
    }
}

struct Component;

impl CoreGuest for Component {
    async fn handle_message(msg: NatsMessage) -> Result<(), String> {
        monotonic_clock::wait_for(YIELD_NANOS).await;
        let name = String::from_utf8_lossy(&msg.body).to_string();
        let bucket = kv::open(name)
            .await
            .map_err(|e| format!("open failed: {e:?}"))?;
        let missing = describe(bucket.get("missing".to_string()).await);
        let denied = describe(kv::open("legacy-denied".to_string()).await);
        bucket
            .put("status".to_string(), format!("{missing}|{denied}").into_bytes())
            .await
            .map(|_| ())
            .map_err(|e| format!("put failed: {e:?}"))
    }
}

export!(Component);
