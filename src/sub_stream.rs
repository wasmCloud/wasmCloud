use crate::json_deserialize;
use futures::StreamExt;
use serde::de::DeserializeOwned;
use std::time::Duration;
use tracing::error;

/// Collect results until timeout has elapsed
pub async fn collect_timeout<T: DeserializeOwned>(
    mut sub: async_nats::Subscriber,
    timeout: Duration,
    reason: &str,
) -> Vec<T> {
    let mut items = Vec::new();
    let sleep = tokio::time::sleep(timeout);
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            maybe_msg = sub.next() => {
                if let Some(msg) = maybe_msg {
                    if msg.payload.is_empty() { break; }
                    let item = match json_deserialize::<T>(&msg.payload) {
                        Ok(item) => item,
                        Err(error) => {
                            error!(%reason, %error,
                                "deserialization error in auction - results may be incomplete",
                            );
                            break;
                        }
                    };
                    items.push(item);
                } else { break; }
            },
            _ = &mut sleep => { /* timeout */ break; }
        }
    }
    items
}
