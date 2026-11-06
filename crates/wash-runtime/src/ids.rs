//! Identifiers minted on hot paths.

use uuid::timestamp::context::ThreadLocalContext;
use uuid::{ContextV7, Timestamp, Uuid};

thread_local! {
    static CONTEXT_V7: ContextV7 = const { ContextV7::new() };
}

/// A version 7 UUID, like [`Uuid::now_v7`], from a per-thread context.
///
/// `Uuid::now_v7` keeps one process-wide clock sequence behind a mutex, so
/// every store a host creates (three ids each) queues every thread behind
/// that lock. Ids made here are still time-ordered and monotonic on each
/// thread, and unique across threads by their random bits; only the ordering
/// of ids minted in the same millisecond on different threads is lost, which
/// nothing relies on.
pub(crate) fn now_v7() -> Uuid {
    Uuid::new_v7(Timestamp::now(ThreadLocalContext::new(&CONTEXT_V7)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    // Collecting the handles spawns every thread before joining any, so the
    // threads mint ids concurrently.
    #[allow(clippy::needless_collect)]
    fn ids_are_v7_and_unique_across_threads() {
        let ids: Vec<Uuid> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..8)
                .map(|_| s.spawn(|| (0..10_000).map(|_| now_v7()).collect::<Vec<_>>()))
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect()
        });
        assert!(ids.iter().all(|id| id.get_version_num() == 7));
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
    }

    #[test]
    fn ids_are_monotonic_on_one_thread() {
        let ids: Vec<Uuid> = (0..10_000).map(|_| now_v7()).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
    }
}
