use anyhow::Result;
use std::{sync::Arc, time::Duration};

/// Folds concurrent identical requests into one: the first caller does the
/// work and the others wait for its result, which then answers the same
/// request until `reuse` after it was produced.
///
/// A failure is shared the same way. Callers queued behind a pull that timed
/// out would otherwise each retry in turn, and each wait out the timeout.
pub(super) struct Coalescer<K, V> {
    pub(super) cells: moka::sync::Cache<K, Arc<CoalescedCell<V>>>,
}

pub(super) type CoalescedCell<V> =
    tokio::sync::OnceCell<std::result::Result<V, Arc<anyhow::Error>>>;

/// Keeps an entry while its work is in flight, then only for `reuse`.
struct CoalescedExpiry {
    join: Duration,
    reuse: Duration,
}

impl<K, V> moka::Expiry<K, Arc<CoalescedCell<V>>> for CoalescedExpiry {
    fn expire_after_create(
        &self,
        _key: &K,
        value: &Arc<CoalescedCell<V>>,
        _created_at: std::time::Instant,
    ) -> Option<Duration> {
        Some(if value.initialized() {
            self.reuse
        } else {
            self.join
        })
    }

    fn expire_after_update(
        &self,
        key: &K,
        value: &Arc<CoalescedCell<V>>,
        updated_at: std::time::Instant,
        _duration_until_expiry: Option<Duration>,
    ) -> Option<Duration> {
        self.expire_after_create(key, value, updated_at)
    }
}

impl<K, V> Coalescer<K, V>
where
    K: std::hash::Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    #[cfg(test)]
    pub(super) fn new(
        capacity: u64,
        join: Duration,
        reuse: Duration,
        weigh: fn(&V) -> u32,
    ) -> Self {
        Self::with_min_weight(capacity, join, reuse, 1, weigh)
    }

    pub(super) fn with_min_weight(
        capacity: u64,
        join: Duration,
        reuse: Duration,
        minimum: u32,
        weigh: fn(&V) -> u32,
    ) -> Self {
        Self {
            cells: moka::sync::Cache::builder()
                .max_capacity(capacity)
                // Weighed again when `run` re-inserts the finished cell.
                .weigher(
                    move |_key: &K, cell: &Arc<CoalescedCell<V>>| match cell.get() {
                        Some(Ok(value)) => weigh(value).max(minimum.max(1)),
                        _ => minimum.max(1),
                    },
                )
                .expire_after(CoalescedExpiry { join, reuse })
                .build(),
        }
    }

    #[cfg(test)]
    pub(super) async fn run<F, Fut>(&self, key: K, work: F) -> Result<V>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<V>>,
    {
        self.run_with_reuse(key, true, work).await
    }

    pub(super) async fn run_with_reuse<F, Fut>(
        &self,
        key: K,
        reuse_finished: bool,
        work: F,
    ) -> Result<V>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<V>>,
    {
        let cell = self
            .cells
            .entry(key.clone())
            .or_insert_with_if(Default::default, |cell| {
                !reuse_finished && cell.initialized()
            })
            .into_value();
        let mut ran = false;
        let result = cell
            .get_or_init(|| {
                ran = true;
                async { work().await.map_err(Arc::new) }
            })
            .await
            .clone();
        if ran {
            // Re-inserting restarts the entry's clock, now at `reuse`, so a
            // slow pull is still reused for the full window after it lands.
            self.cells.entry(key).and_compute_with(|current| {
                use moka::ops::compute::Op;
                if current.is_some_and(|entry| Arc::ptr_eq(entry.value(), &cell)) {
                    Op::Put(cell)
                } else {
                    // A fresh caller may already have replaced this completed
                    // cell. Do not overwrite its work with an older result.
                    Op::Nop
                }
            });
        }
        // Shared rather than reformatted, so each caller can still walk the
        // chain (a timeout, an auth failure) under the top message.
        result.map_err(|err| anyhow::Error::new(crate::engine::SharedError(err)))
    }
}
