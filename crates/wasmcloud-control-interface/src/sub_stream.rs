//! Stream interface for async nats subscription
//!

pub use crate::events::ControlEvent;
use crate::json_deserialize;
use log::error;
use nats::asynk::Subscription;
use serde::de::DeserializeOwned;
use std::time::{Duration, Instant};

/// Result of waiting for next subscription item
#[doc(hidden)]
pub enum SubscriptionNextResult<T: serde::de::DeserializeOwned> {
    /// Item received and deserialized
    Item(T),
    /// Timeout
    Timeout,
    /// Subscription cancelled or connection closed
    Cancelled,
    /// Deserialization error
    Err(String),
}

/// Stream wrapper for nats subscription
#[doc(hidden)]
pub struct SubscriptionStream {
    sub: Subscription,
}

impl SubscriptionStream {
    /// Construct stream wrapper for nats async subscription
    pub fn new(sub: Subscription) -> SubscriptionStream {
        SubscriptionStream { sub }
    }

    /// Wait for next subscription item and attempt to deserialize
    pub async fn next<T: DeserializeOwned>(
        &mut self,
        timeout: Duration,
    ) -> SubscriptionNextResult<T> {
        match tokio::time::timeout(timeout, self.sub.next()).await {
            // Error return means timeout occurred before a new value was received
            Err(_) => SubscriptionNextResult::Timeout,
            // Ok(Some(_)) means subscription got a value
            Ok(Some(msg)) => match json_deserialize::<T>(&msg.data) {
                Ok(item) => SubscriptionNextResult::Item(item),
                Err(e) => SubscriptionNextResult::Err(e.to_string()),
            },
            // Ok(None) means subscription was cancelled
            Ok(None) => SubscriptionNextResult::Cancelled,
        }
    }

    /// Collect results until timeout has elapsed
    pub async fn collect<T: DeserializeOwned>(
        &mut self,
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
            match self.next(timeout - elapsed).await {
                SubscriptionNextResult::Item(item) => items.push(item),
                SubscriptionNextResult::Cancelled | SubscriptionNextResult::Timeout => break,
                SubscriptionNextResult::Err(s) => {
                    // log corrupt messages but continue receiving until timeout
                    error!("corrupt message received {}: {}", reason, s);
                }
            }
        }
        items
    }
}
