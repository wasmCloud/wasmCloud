//! Identifiers minted on hot paths.
//!
//! Every UUIDv7 here takes its counter from one process-wide sequence:
//!
//! - Each thread reserves [`BLOCK`] counter values at a time under one shared
//!   lock, and renews its reservation when the clock advances, so IDs are
//!   never stale.
//! - Reservations bind counters to a millisecond. When the 42-bit counter
//!   wraps, the next cycle starts after the latest millisecond the last cycle
//!   handed out, so even retained blocks cannot reuse a (millisecond, counter)
//!   pair.
//! - Each thread keeps its own last millisecond through clock rollback, so IDs
//!   are monotonic per thread. A clock step on one thread does not hold back
//!   the timestamps of other threads.
//!
//! Counter seeds and batched tails come from the OS. Application PRNGs can
//! repeat after explicit seeding, and time/thread-based seeds can repeat across
//! processes; neither provides independent cross-process entropy. Randomness
//! only reduces cross-process collision probability: the deterministic
//! uniqueness guarantee here is confined to one process lifetime.

use std::cell::RefCell;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::{Builder, Uuid};

/// Counter values a thread reserves from [`sequence`] at a time.
const BLOCK: u64 = 64;
/// Width of the v7 counter field; the 32 bits below it are random.
const COUNTER_BITS: u32 = 42;
const COUNTER_MASK: u64 = (1 << COUNTER_BITS) - 1;

thread_local! {
    static RESERVED: RefCell<Block> = const { RefCell::new(Block::new()) };
}

/// A UUIDv7 unique within this process, locking only to renew a reservation.
pub fn now_v7() -> Uuid {
    RESERVED.with_borrow_mut(|block| block.take(unix_millis(), sequence()))
}

/// The process-wide sequence, starting at a random value so that separate
/// processes don't walk the same counters.
fn sequence() -> &'static Mutex<Sequence> {
    static SEQUENCE: OnceLock<Mutex<Sequence>> = OnceLock::new();
    SEQUENCE.get_or_init(|| {
        let mut seed = [0; 8];
        random_bytes(&mut seed);
        Mutex::new(Sequence::new(u64::from_ne_bytes(seed)))
    })
}

/// Like `Uuid::new_v4`, fail closed if the OS cannot provide randomness.
/// Never substitute time, a fixed seed, or partially filled bytes.
fn random_bytes(bytes: &mut [u8]) {
    let result = getrandom::fill(bytes);
    assert!(
        result.is_ok(),
        "OS randomness unavailable for runtime IDs: {result:?}"
    );
}

/// Allocates disjoint counter ranges. A reserved range keeps its timestamp,
/// even if another thread completes a counter cycle.
struct Sequence {
    next: u64,
    /// The earliest timestamp a reservation in the current cycle may take.
    floor: u64,
    /// The latest timestamp any reservation in the current cycle took.
    latest: u64,
}

impl Sequence {
    const fn new(seed: u64) -> Self {
        Self {
            next: seed & COUNTER_MASK,
            floor: 0,
            latest: 0,
        }
    }

    /// Reserves [`BLOCK`] counters at a timestamp no earlier than `now_ms`
    /// or the caller's previous reservation, `after_ms`.
    fn reserve(&mut self, now_ms: u64, after_ms: u64) -> (u64, u64) {
        // Keep each block entirely within one counter cycle. Any remaining
        // values are discarded when fewer than BLOCK remain before wrap.
        if self.next > COUNTER_MASK - (BLOCK - 1) {
            self.next = 0;
            self.floor = self.latest.saturating_add(1);
        }
        let millis = now_ms.max(after_ms).max(self.floor);
        // The UUID layout stores only 48 timestamp bits. Fail on exhaustion
        // rather than silently truncate the timestamp and reuse identifiers.
        assert!(millis < (1 << 48), "UUIDv7 timestamp exhausted");
        self.latest = self.latest.max(millis);
        let counter = self.next;
        self.next += BLOCK;
        (millis, counter)
    }
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// One thread's reserved counter values.
struct Block {
    next: u64,
    end: u64,
    /// The timestamp assigned to every counter in this reservation.
    millis: u64,
    /// Independent of counter reservations: a millisecond tick must not waste
    /// the rest of the entropy batch or require another OS read.
    tails: Option<std::array::IntoIter<[u8; 4], 64>>,
}

impl Block {
    const fn new() -> Self {
        Self {
            next: 0,
            end: 0,
            millis: 0,
            tails: None,
        }
    }

    fn take(&mut self, now_ms: u64, sequence: &Mutex<Sequence>) -> Uuid {
        if self.next == self.end || now_ms > self.millis {
            let mut sequence = sequence.lock().unwrap_or_else(PoisonError::into_inner);
            (self.millis, self.next) = sequence.reserve(now_ms, self.millis);
            self.end = self.next + BLOCK;
        }
        let counter = self.next;
        self.next += 1;
        // The (timestamp, counter) pair ensures process-local uniqueness;
        // independent OS entropy reduces collisions between processes.
        build(self.millis, counter, self.random_tail())
    }

    fn random_tail(&mut self) -> u32 {
        loop {
            if let Some(tail) = self.tails.as_mut().and_then(Iterator::next) {
                return u32::from_ne_bytes(tail);
            }
            let mut tails = [[0; 4]; 64];
            random_bytes(tails.as_flattened_mut());
            self.tails = Some(tails.into_iter());
        }
    }
}

/// Lay out the counter above the random tail, as uuid's `ContextV7` does.
fn build(ms: u64, counter: u64, tail: u32) -> Uuid {
    let bits = (u128::from(counter) << 32) | u128::from(tail);
    let mut bytes = [0; 10];
    // rand_a: the top 12 of the 74 bits; rand_b: the low 62.
    let (rand_a, rand_b) = bytes.split_at_mut(2);
    rand_a.copy_from_slice(&((bits >> 62) as u16).to_be_bytes());
    rand_b.copy_from_slice(&((bits as u64) & (u64::MAX >> 2)).to_be_bytes());
    Builder::from_unix_timestamp_millis(ms, &bytes).into_uuid()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The millisecond and counter of an ID, ignoring its random tail.
    fn ms_and_counter(id: &Uuid) -> (u64, u64) {
        let value = id.as_u128();
        let ms = (value >> 80) as u64;
        let rand_a = ((value >> 64) & 0xFFF) as u64;
        let rand_b = (value & (u128::MAX >> 66)) as u64;
        (ms, (rand_a << 30) | (rand_b >> 32))
    }

    /// Forces the next reservation to start a new counter cycle.
    fn exhaust_cycle(sequence: &Mutex<Sequence>) {
        sequence.lock().unwrap_or_else(PoisonError::into_inner).next = 1 << COUNTER_BITS;
    }

    #[test]
    // Start every worker before joining any.
    #[allow(clippy::needless_collect)]
    fn ids_never_share_a_timestamp_and_counter_across_threads() -> anyhow::Result<()> {
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
        assert!(
            ids.iter()
                .all(|id| id.get_variant() == uuid::Variant::RFC4122)
        );
        // Unique without relying on the random tails.
        assert_eq!(
            ids.iter().map(ms_and_counter).collect::<HashSet<_>>().len(),
            ids.len()
        );
        Ok(())
    }

    #[test]
    fn ids_are_monotonic_on_one_thread() {
        let ids: Vec<Uuid> = (0..10_000).map(|_| now_v7()).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn layout_round_trips_the_counter_and_tail() {
        let id = build(1_700_000_000_123, COUNTER_MASK, 0xDEAD_BEEF);
        assert_eq!(ms_and_counter(&id), (1_700_000_000_123, COUNTER_MASK));
        assert_eq!(id.as_u128() as u32, 0xDEAD_BEEF);
        assert_eq!(id.get_version_num(), 7);
    }

    #[test]
    fn ids_are_monotonic_across_clock_rollback_and_counter_wrap() {
        let sequence = Mutex::new(Sequence::new(COUNTER_MASK - (BLOCK - 1)));
        let mut block = Block::new();
        let ids: Vec<Uuid> = std::iter::once(1_000)
            .chain(std::iter::repeat_n(999, BLOCK as usize))
            .map(|ms| block.take(ms, &sequence))
            .collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(
            ids.first().map(ms_and_counter),
            Some((1_000, COUNTER_MASK - (BLOCK - 1)))
        );
        assert_eq!(ids.last().map(ms_and_counter), Some((1_001, 0)));
    }

    #[test]
    fn retained_reservations_do_not_overlap_after_a_sequence_cycle() {
        let sequence = Mutex::new(Sequence::new(0));
        let mut slow = Block::new();
        slow.take(1_000, &sequence);

        // Other threads have reserved a complete counter cycle while this
        // thread retained its unused values. Skip those irrelevant blocks.
        exhaust_cycle(&sequence);
        let mut fast = Block::new();
        fast.take(2_000, &sequence);
        let fast_id = fast.take(2_000, &sequence);
        let slow_id = slow.take(2_000, &sequence);

        assert_ne!(ms_and_counter(&slow_id), ms_and_counter(&fast_id));
        assert_eq!(ms_and_counter(&fast_id), (2_000, 1));
        assert_eq!(ms_and_counter(&slow_id), (2_000, BLOCK));
    }

    #[test]
    fn retained_reservations_keep_their_timestamp_during_clock_rollback() {
        let sequence = Mutex::new(Sequence::new(0));
        let mut slow = Block::new();
        slow.take(1_000, &sequence);

        exhaust_cycle(&sequence);
        let mut fast = Block::new();
        fast.take(1_000, &sequence);
        let fast_id = fast.take(999, &sequence);
        let slow_id = slow.take(999, &sequence);

        assert_eq!(ms_and_counter(&fast_id), (1_001, 1));
        assert_eq!(ms_and_counter(&slow_id), (1_000, 1));
    }

    #[test]
    fn a_clock_step_on_one_thread_does_not_hold_back_others() {
        let sequence = Mutex::new(Sequence::new(0));
        let mut stepped = Block::new();
        stepped.take(5_000, &sequence);

        let mut other = Block::new();
        assert_eq!(
            ms_and_counter(&other.take(1_000, &sequence)),
            (1_000, BLOCK)
        );
        // The stepped thread stays monotonic on its own timestamp.
        assert_eq!(ms_and_counter(&stepped.take(1_000, &sequence)), (5_000, 1));
    }

    #[test]
    fn a_new_cycle_starts_after_every_timestamp_of_the_last() {
        let sequence = Mutex::new(Sequence::new(0));
        Block::new().take(5_000, &sequence);
        exhaust_cycle(&sequence);
        assert_eq!(
            ms_and_counter(&Block::new().take(1_000, &sequence)),
            (5_001, 0)
        );
    }

    #[test]
    #[allow(clippy::needless_collect)]
    fn concurrent_reservations_are_unique_across_counter_wrap() -> anyhow::Result<()> {
        let sequence = Mutex::new(Sequence::new(COUNTER_MASK - (BLOCK - 1)));
        let barrier = std::sync::Barrier::new(8);
        let ids = std::thread::scope(|s| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    s.spawn(|| {
                        let mut block = Block::new();
                        let first = block.take(1_000, &sequence);
                        // Keep the last pre-wrap block alive while the other
                        // workers reserve blocks in the next counter cycle.
                        barrier.wait();
                        let ids: Vec<_> = std::iter::once(first)
                            .chain((1..1_000).map(|i| {
                                block.take(if i % 2 == 0 { 999 } else { 1_001 }, &sequence)
                            }))
                            .collect();
                        assert!(ids.windows(2).all(|w| w[0] < w[1]));
                        ids
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .map_err(|_| anyhow::anyhow!("UUID worker panicked"))
                })
                .collect::<anyhow::Result<Vec<_>>>()
        })?;
        let ids: Vec<_> = ids.into_iter().flatten().collect();
        assert_eq!(
            ids.iter().map(ms_and_counter).collect::<HashSet<_>>().len(),
            ids.len()
        );
        Ok(())
    }

    #[test]
    fn reservations_never_straddle_counter_wrap() {
        for seed in [COUNTER_MASK - (BLOCK - 2), COUNTER_MASK] {
            let mut sequence = Sequence::new(seed);
            assert_eq!(sequence.reserve(1_000, 0), (1_000, 0));
            assert_eq!(sequence.reserve(1_000, 0), (1_000, BLOCK));
        }
    }

    #[test]
    #[should_panic(expected = "UUIDv7 timestamp exhausted")]
    fn timestamp_exhaustion_does_not_reuse_identifiers() {
        let mut sequence = Sequence::new(0);
        sequence.latest = (1 << 48) - 1;
        sequence.next = 1 << COUNTER_BITS;
        sequence.reserve(0, 0);
    }
}
