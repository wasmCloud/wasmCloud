use anyhow::Result;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError, Weak},
    time::{Duration, Instant},
};

/// Folds concurrent identical requests into one: the first caller does the
/// work and the others wait for its result. Completed-result retention is
/// supplied by the selected cache backend.
///
/// A failure answers only callers that joined while the work ran. Later callers
/// retry immediately, since the registry, credentials or trust may have changed.
pub(crate) struct Coalescer<K, V> {
    /// Work in flight, held weakly: an entry lives only while some caller is
    /// still waiting on it. Not subject to cache eviction, which would stop
    /// coalescing exactly when many callers want the same thing.
    in_flight: Mutex<HashMap<K, InFlight<V>>>,
    /// The longest a request in flight is joined by identical requests.
    join: Duration,
}

pub(crate) type CoalescedCell<V> =
    tokio::sync::OnceCell<std::result::Result<V, Arc<anyhow::Error>>>;

struct InFlight<V> {
    started: Instant,
    cell: Weak<CoalescedCell<V>>,
    waiters: usize,
}

/// Remove a cancelled flight when its last caller leaves, without needing
/// another request to sweep dead keys. Count owners under the same lock as
/// registration so concurrent drops cannot both miss being the last owner.
struct InFlightGuard<'a, K: std::hash::Hash + Eq, V> {
    in_flight: &'a Mutex<HashMap<K, InFlight<V>>>,
    key: K,
    cell: Arc<CoalescedCell<V>>,
}

impl<K: std::hash::Hash + Eq, V> Drop for InFlightGuard<'_, K, V> {
    fn drop(&mut self) {
        let mut in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = in_flight.get_mut(&self.key)
            && std::ptr::eq(entry.cell.as_ptr(), Arc::as_ptr(&self.cell))
        {
            if entry.waiters > 1 {
                entry.waiters -= 1;
            } else {
                in_flight.remove(&self.key);
            }
        }
    }
}

impl<K, V> Coalescer<K, V>
where
    K: std::hash::Hash + Eq + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// Coordinate only work in flight. Retention belongs to the chosen backend.
    pub(crate) fn flights(join: Duration) -> Self {
        Self {
            in_flight: Mutex::new(HashMap::new()),
            join,
        }
    }

    /// Requests in flight now, for tests.
    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub(crate) fn in_flight_count(&self) -> usize {
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Callers sharing the request in flight for `key`, for the OCI tests.
    #[cfg(all(test, feature = "oci"))]
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub(crate) fn waiters(&self, key: &K) -> usize {
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .map_or(0, |entry| entry.waiters)
    }

    /// Join the identical request in flight, or start one. Completed results
    /// answer existing waiters only; a subsequent request starts afresh.
    pub(crate) async fn run<F, Fut>(&self, key: K, work: F) -> Result<V>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<V>>,
    {
        let flight = self.join_or_start(&key);
        let cell = &flight.cell;
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
            // New callers start again and consult their chosen cache.
            let mut in_flight = self
                .in_flight
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            // A fresh caller may already have replaced this cell, past the
            // join window. Do not overwrite its work with an older result.
            if in_flight
                .get(&key)
                .is_some_and(|entry| std::ptr::eq(entry.cell.as_ptr(), Arc::as_ptr(cell)))
            {
                in_flight.remove(&key);
            }
        }
        // Shared rather than reformatted, so each caller can still walk the
        // chain (a timeout, an auth failure) under the top message.
        result.map_err(|err| anyhow::Error::new(crate::engine::SharedError(err)))
    }

    fn join_or_start(&self, key: &K) -> InFlightGuard<'_, K, V> {
        let guard = |cell| InFlightGuard {
            in_flight: &self.in_flight,
            key: key.clone(),
            cell,
        };
        let mut in_flight = self
            .in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let joinable = in_flight.get_mut(key).and_then(|entry| {
            if entry.started.elapsed() >= self.join {
                return None;
            }
            let cell = entry.cell.upgrade()?;
            // Finished between the leader's result and its bookkeeping.
            if cell.get().is_some() {
                return None;
            }
            entry.waiters += 1;
            Some(cell)
        });
        if let Some(cell) = joinable {
            return guard(cell);
        }
        let cell = Arc::new(CoalescedCell::default());
        in_flight.insert(
            key.clone(),
            InFlight {
                started: Instant::now(),
                cell: Arc::downgrade(&cell),
                waiters: 1,
            },
        );
        guard(cell)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelling_a_burst_releases_keys_without_another_request() {
        let cache = Arc::new(Coalescer::<_, u32>::flights(Duration::from_secs(60)));
        let (started, mut ready) = tokio::sync::mpsc::channel(32);
        let mut callers = tokio::task::JoinSet::new();
        for key in 0..32 {
            let cache = cache.clone();
            let started = started.clone();
            callers.spawn(async move {
                cache
                    .run(key, || async {
                        started.send(()).await.unwrap();
                        std::future::pending().await
                    })
                    .await
            });
        }
        for _ in 0..32 {
            ready.recv().await.unwrap();
        }
        assert_eq!(cache.in_flight_count(), 32);
        callers.abort_all();
        while let Some(result) = callers.join_next().await {
            assert!(result.unwrap_err().is_cancelled());
        }
        assert_eq!(cache.in_flight_count(), 0);
    }

    #[test]
    fn only_the_last_waiter_removes_its_flight() {
        let cache = Coalescer::<_, u32>::flights(Duration::from_secs(60));
        let first = cache.join_or_start(&"image");
        let second = cache.join_or_start(&"image");
        assert!(Arc::ptr_eq(&first.cell, &second.cell));
        drop(first);
        assert_eq!(cache.in_flight_count(), 1);
        drop(second);
        assert_eq!(cache.in_flight_count(), 0);
    }

    #[test]
    fn cancelling_an_expired_flight_keeps_its_replacement() {
        let cache = Coalescer::<_, u32>::flights(Duration::ZERO);
        let old = cache.join_or_start(&"image");
        let replacement = cache.join_or_start(&"image");
        assert!(!Arc::ptr_eq(&old.cell, &replacement.cell));
        drop(old);
        assert_eq!(cache.in_flight_count(), 1);
        drop(replacement);
        assert_eq!(cache.in_flight_count(), 0);
    }
}
