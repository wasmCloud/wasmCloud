use anyhow::Result;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError, Weak},
    time::{Duration, Instant},
};

/// Folds concurrent identical requests into one: the first caller does the
/// work and the others wait for its result, which then answers the same
/// request until `reuse` after it was produced.
///
/// A failure answers only callers that joined while the work ran. Later callers
/// retry immediately, since the registry, credentials or trust may have changed.
pub(super) struct Coalescer<K, V> {
    /// Work in flight, held weakly: an entry lives only while some caller is
    /// still waiting on it. Kept apart from `finished` so it is never refused
    /// or evicted for space, which would stop coalescing exactly when many
    /// callers want the same thing.
    in_flight: Mutex<HashMap<K, InFlight<V>>>,
    /// Finished successes still within their reuse window, bounded by weight.
    pub(super) finished: moka::sync::Cache<K, V>,
    /// The longest a request in flight is joined by identical requests.
    join: Duration,
}

pub(super) type CoalescedCell<V> =
    tokio::sync::OnceCell<std::result::Result<V, Arc<anyhow::Error>>>;

struct InFlight<V> {
    started: Instant,
    cell: Weak<CoalescedCell<V>>,
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
            in_flight: Mutex::new(HashMap::new()),
            finished: moka::sync::Cache::builder()
                .max_capacity(capacity)
                .weigher(move |_key: &K, value: &V| weigh(value).max(minimum.max(1)))
                .time_to_live(reuse)
                .build(),
            join,
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

    /// Requests in flight now, for tests.
    #[cfg(test)]
    pub(super) fn in_flight_count(&self) -> usize {
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Join the identical request in flight, or start one. With
    /// `reuse_finished` false a finished result is never returned, and none is
    /// kept: nothing could reuse it.
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
        let cell = self.join_or_start(&key, reuse_finished);
        let mut ran = false;
        let result = cell
            .get_or_init(|| {
                ran = true;
                async { work().await.map_err(Arc::new) }
            })
            .await
            .clone();
        if ran {
            // Callers already waiting hold the cell and still get this result.
            // New callers either reuse a success or start again.
            let mut in_flight = self
                .in_flight
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            // A fresh caller may already have replaced this cell, past the
            // join window. Do not overwrite its work with an older result.
            if in_flight
                .get(&key)
                .is_some_and(|entry| std::ptr::eq(entry.cell.as_ptr(), Arc::as_ptr(&cell)))
            {
                in_flight.remove(&key);
                if reuse_finished && let Ok(value) = &result {
                    self.finished.insert(key, value.clone());
                }
            }
        }
        // Shared rather than reformatted, so each caller can still walk the
        // chain (a timeout, an auth failure) under the top message.
        result.map_err(|err| anyhow::Error::new(crate::engine::SharedError(err)))
    }

    fn join_or_start(&self, key: &K, reuse_finished: bool) -> Arc<CoalescedCell<V>> {
        let mut in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // Looked up under the lock, so a result moved here by a leader that
        // just finished is not missed.
        if reuse_finished && let Some(value) = self.finished.get(key) {
            return Arc::new(CoalescedCell::new_with(Some(Ok(value))));
        }
        let joinable = in_flight.get(key).and_then(|entry| {
            if entry.started.elapsed() >= self.join {
                return None;
            }
            let cell = entry.cell.upgrade()?;
            // Finished between the leader's result and its bookkeeping. A
            // failure answers only its waiters, and a caller that may not
            // reuse a finished result starts afresh.
            match cell.get() {
                Some(Err(_)) => None,
                Some(Ok(_)) if !reuse_finished => None,
                _ => Some(cell),
            }
        });
        if let Some(cell) = joinable {
            return cell;
        }
        // Entries whose callers were all cancelled are dropped here, so the map
        // holds no more than the requests actually in flight.
        in_flight.retain(|_, entry| entry.cell.strong_count() > 0);
        let cell = Arc::new(CoalescedCell::default());
        in_flight.insert(
            key.clone(),
            InFlight {
                started: Instant::now(),
                cell: Arc::downgrade(&cell),
            },
        );
        cell
    }
}
