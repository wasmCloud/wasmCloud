use crate::json_deserialize;
use log::error;
use serde::de::DeserializeOwned;
use std::time::{Duration, Instant};
use wasmbus_rpc::anats::Subscription;

/// Collect results until timeout has elapsed
pub async fn collect_timeout<T: DeserializeOwned>(
    sub: Subscription,
    timeout: Duration,
    reason: &str,
) -> Vec<T> {
    let start = Instant::now();
    let mut items = Vec::new();
    loop {
        let elapsed = start.elapsed();
        if elapsed >= timeout {
            break;
        }
        // keep collecting while there is time remaining
        match sub.next_timeout(timeout - elapsed).await {
            Ok(msg) => {
                if msg.data.is_empty() {
                    break;
                }
                let item = match json_deserialize::<T>(&msg.data) {
                    Ok(item) => item,
                    Err(e) => {
                        error!(
                            "deserialization error in auction ({}) - results may be incomplete: {}",
                            reason, e
                        );
                        break;
                    }
                };
                items.push(item);
            }
            Err(_) => {
                // timeout, we're done
                break;
            }
        }
    }
    items
}
