//! Identifiers minted on hot paths.

use uuid::timestamp::context::ThreadLocalContext;
use uuid::{ContextV7, Timestamp, Uuid};

thread_local! {
    static CONTEXT_V7: ContextV7 = const { ContextV7::new() };
}

/// A UUIDv7 from a per-thread context, without a process-wide sequence lock.
/// IDs are monotonic per thread; random bits distinguish IDs across threads.
pub fn now_v7() -> Uuid {
    Uuid::new_v7(Timestamp::now(ThreadLocalContext::new(&CONTEXT_V7)))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    // Start every worker before joining any.
    #[allow(clippy::needless_collect)]
    fn ids_are_v7_and_unique_across_threads() -> anyhow::Result<()> {
        let ids = std::thread::scope(|s| {
            let handles: Vec<_> = (0..8)
                .map(|_| s.spawn(|| (0..10_000).map(|_| now_v7()).collect::<Vec<_>>()))
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .map_err(|_| anyhow::anyhow!("UUID worker panicked"))
                })
                .collect::<anyhow::Result<Vec<_>>>()
        })?;
        let ids: Vec<Uuid> = ids.into_iter().flatten().collect();
        assert!(ids.iter().all(|id| id.get_version_num() == 7));
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        Ok(())
    }

    #[test]
    fn ids_are_monotonic_on_one_thread() {
        let ids: Vec<Uuid> = (0..10_000).map(|_| now_v7()).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
    }
}
