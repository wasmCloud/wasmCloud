//! Identifiers minted on hot paths.
//!
//! Every UUIDv7 here takes its counter from one process-wide sequence:
//!
//! - Each thread reserves [`BLOCK`] values at a time under one shared lock,
//!   and draws the random tails for the whole block with one OS read.
//! - Reservations bind counters to a millisecond. The shared sequence advances
//!   that millisecond when the 42-bit counter wraps, so even retained blocks
//!   cannot reuse a (millisecond, counter) pair.
//! - A thread renews its reservation when the clock advances. IDs are never
//!   stale and are monotonic per thread, including through clock rollback.

use std::cell::RefCell;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::{Builder, Uuid};

/// Counter values a thread reserves from [`sequence`] at a time.
const BLOCK: usize = 64;
/// Width of the v7 counter field; the 32 bits below it are random.
const COUNTER_BITS: u32 = 42;
const COUNTER_MASK: u64 = (1 << COUNTER_BITS) - 1;
const TAIL_BYTES: usize = 4;

thread_local! {
    static RESERVED: RefCell<Block> = const { RefCell::new(Block::new()) };
}

/// A UUIDv7 unique within this process, locking only to renew a reservation.
pub(crate) fn now_v7() -> Uuid {
    RESERVED.with_borrow_mut(|block| block.take(unix_millis, sequence()))
}

/// The process-wide sequence, starting at a random value so that separate
/// processes don't walk the same counters.
fn sequence() -> &'static Mutex<Sequence> {
    static SEQUENCE: OnceLock<Mutex<Sequence>> = OnceLock::new();
    SEQUENCE.get_or_init(|| {
        let mut seed = [0; 8];
        fill_random(&mut seed);
        Mutex::new(Sequence::new(u64::from_ne_bytes(seed)))
    })
}

/// Allocates disjoint counter ranges at nondecreasing timestamps. A reserved
/// range must keep its timestamp, even if another thread completes a cycle.
struct Sequence {
    next: u64,
    millis: u64,
}

impl Sequence {
    const fn new(seed: u64) -> Self {
        Self {
            next: seed & COUNTER_MASK,
            millis: 0,
        }
    }

    fn reserve(&mut self, now_ms: u64) -> (u64, u64) {
        // Keep each block entirely within one counter cycle. Any remaining
        // values are discarded when fewer than BLOCK remain before wrap.
        if self.next > COUNTER_MASK - (BLOCK as u64 - 1) {
            self.next = 0;
            self.millis = self.millis.saturating_add(1);
        }
        self.millis = self.millis.max(now_ms);
        // The UUID layout stores only 48 timestamp bits. Fail on exhaustion
        // rather than silently truncate the timestamp and reuse identifiers.
        assert!(self.millis < (1 << 48), "UUIDv7 timestamp exhausted");
        let counter = self.next;
        self.next += BLOCK as u64;
        (self.millis, counter)
    }
}

/// Uniqueness within the process comes from the sequence alone; a failed OS
/// read only costs the random bits their unpredictability.
fn fill_random(bytes: &mut [u8]) {
    if getrandom::fill(bytes).is_err() {
        bytes.fill_with(|| fastrand::u8(..));
    }
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// One thread's reserved counter values and their random tails.
struct Block {
    next: u64,
    left: usize,
    tails: [u8; BLOCK * TAIL_BYTES],
    /// The timestamp assigned to every counter in this reservation.
    millis: u64,
}

impl Block {
    const fn new() -> Self {
        Self {
            next: 0,
            left: 0,
            tails: [0; BLOCK * TAIL_BYTES],
            millis: 0,
        }
    }

    fn take(&mut self, now: impl Fn() -> u64, sequence: &Mutex<Sequence>) -> Uuid {
        if self.left == 0 || now() > self.millis {
            // Draw entropy before taking the shared lock. Read the clock
            // again under the lock so the reservation includes any time
            // spent waiting for entropy or another reserving thread.
            fill_random(&mut self.tails);
            let mut sequence = sequence.lock().unwrap_or_else(PoisonError::into_inner);
            (self.millis, self.next) = sequence.reserve(now());
            self.left = BLOCK;
        }
        let counter = self.next;
        let offset = BLOCK.saturating_sub(self.left).saturating_mul(TAIL_BYTES);
        let tail = self
            .tails
            .get(offset..offset.saturating_add(TAIL_BYTES))
            .and_then(|bytes| <[u8; TAIL_BYTES]>::try_from(bytes).ok())
            .map_or(0, u32::from_be_bytes);
        self.next += 1;
        self.left = self.left.saturating_sub(1);
        build(self.millis, counter, tail)
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
        // Unique without relying on the random tails, even across a wrap.
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
        let sequence = Mutex::new(Sequence::new(COUNTER_MASK - (BLOCK as u64 - 1)));
        let mut block = Block::new();
        let ids: Vec<Uuid> = std::iter::once(1_000)
            .chain(std::iter::repeat_n(999, BLOCK))
            .map(|ms| block.take(|| ms, &sequence))
            .collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(
            ids.first().map(ms_and_counter),
            Some((1_000, COUNTER_MASK - (BLOCK as u64 - 1)))
        );
        assert_eq!(ids.last().map(ms_and_counter), Some((1_001, 0)));
    }

    #[test]
    fn retained_reservations_do_not_overlap_after_a_sequence_cycle() {
        let sequence = Mutex::new(Sequence::new(0));
        let mut slow = Block::new();
        slow.take(|| 1_000, &sequence);

        // Other threads have reserved a complete counter cycle while this
        // thread retained its unused values. Skip those irrelevant blocks.
        sequence.lock().unwrap_or_else(PoisonError::into_inner).next = 1 << COUNTER_BITS;
        let mut fast = Block::new();
        fast.take(|| 2_000, &sequence);
        let fast_id = fast.take(|| 2_000, &sequence);
        let slow_id = slow.take(|| 2_000, &sequence);

        assert_ne!(ms_and_counter(&slow_id), ms_and_counter(&fast_id));
        assert_eq!(ms_and_counter(&fast_id), (2_000, 1));
        assert_eq!(ms_and_counter(&slow_id), (2_000, BLOCK as u64));
    }

    #[test]
    fn retained_reservations_keep_their_timestamp_during_clock_rollback() {
        let sequence = Mutex::new(Sequence::new(0));
        let mut slow = Block::new();
        slow.take(|| 1_000, &sequence);

        sequence.lock().unwrap_or_else(PoisonError::into_inner).next = 1 << COUNTER_BITS;
        let mut fast = Block::new();
        fast.take(|| 1_000, &sequence);
        let fast_id = fast.take(|| 999, &sequence);
        let slow_id = slow.take(|| 999, &sequence);

        assert_eq!(ms_and_counter(&fast_id), (1_001, 1));
        assert_eq!(ms_and_counter(&slow_id), (1_000, 1));
    }

    #[test]
    #[allow(clippy::needless_collect)]
    fn concurrent_reservations_are_unique_across_counter_wrap() -> anyhow::Result<()> {
        let sequence = Mutex::new(Sequence::new(COUNTER_MASK - (BLOCK as u64 - 1)));
        let barrier = std::sync::Barrier::new(8);
        let ids = std::thread::scope(|s| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    s.spawn(|| {
                        let mut block = Block::new();
                        let first = block.take(|| 1_000, &sequence);
                        // Keep the last pre-wrap block alive while the other
                        // workers reserve blocks in the next counter cycle.
                        barrier.wait();
                        let ids: Vec<_> = std::iter::once(first)
                            .chain((1..1_000).map(|i| {
                                block.take(|| if i % 2 == 0 { 999 } else { 1_001 }, &sequence)
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
    fn refill_observes_clock_advances_while_reserving() {
        let sequence = Mutex::new(Sequence::new(0));
        let mut block = Block::new();
        block.take(|| 1_000, &sequence);

        let clock = std::cell::Cell::new(1_001);
        let id = block.take(
            || {
                let now = clock.get();
                clock.set(now + 1);
                now
            },
            &sequence,
        );
        assert_eq!(ms_and_counter(&id), (1_002, BLOCK as u64));
    }

    #[test]
    fn reservations_never_straddle_counter_wrap() {
        for seed in [COUNTER_MASK - (BLOCK as u64 - 2), COUNTER_MASK] {
            let mut sequence = Sequence::new(seed);
            assert_eq!(sequence.reserve(1_000), (1_000, 0));
            assert_eq!(sequence.reserve(999), (1_000, BLOCK as u64));
        }
    }

    #[test]
    #[should_panic(expected = "UUIDv7 timestamp exhausted")]
    fn timestamp_exhaustion_does_not_reuse_identifiers() {
        let mut sequence = Sequence::new(0);
        sequence.millis = (1 << 48) - 1;
        sequence.next = 1 << COUNTER_BITS;
        sequence.reserve(0);
    }
}
