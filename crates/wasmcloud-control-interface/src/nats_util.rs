//! subscription utilities
//!

// TODO: This code is nearly identical to nats_kvcache/src/util - where should it/they live?

use crate::deserialize;
use futures::StreamExt;
use std::time::Duration;

/// Result of next_with_timeout
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

pub(crate) struct SubscriptionStream {
    sub: nats::asynk::Subscription,
    timeout: Duration,
}
/*
   Implementation for async_nats ...

/// Wait for next subscription result and attempt to deserialize
pub async fn next_with_timeout<T: serde::de::DeserializeOwned>(
    sub: &async_nats::Subscription,
    timeout: Duration,
) -> SubscriptionNextResult<T> {
    match tokio::time::timeout(timeout, sub.next()).await {
        Err(_) => SubscriptionNextResult::Timeout,
        Ok(None) => SubscriptionNextResult::Cancelled,
        Ok(Some(msg)) => match deserialize::<T>(&msg.data) {
            Ok(item) => SubscriptionNextResult::Item(item),
            Err(e) => SubscriptionNextResult::Err(e.to_string()),
        },
    }
}
*/

impl SubscriptionStream {
    pub fn new(sub: nats::asynk::Subscription, timeout: Duration) -> SubscriptionStream {
        SubscriptionStream { sub, timeout }
    }

    /// Wait for next subscription result and attempt to deserialize
    pub async fn next<T: serde::de::DeserializeOwned>(&mut self) -> SubscriptionNextResult<T> {
        match tokio::time::timeout(self.timeout, &mut self.sub.next()).await {
            Err(_) => SubscriptionNextResult::Timeout,
            Ok(Some(msg)) => match deserialize::<T>(&msg.data) {
                Ok(item) => SubscriptionNextResult::Item(item),
                Err(e) => SubscriptionNextResult::Err(e.to_string()),
            },
            Ok(None) => SubscriptionNextResult::Cancelled,
        }
    }
}
