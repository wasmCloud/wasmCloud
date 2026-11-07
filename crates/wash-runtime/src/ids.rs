//! Identifiers minted on hot paths.
//!
//! Every UUIDv7 here takes its counter from one process-wide sequence:
//!
//! - Each thread reserves [`BLOCK`] values at a time with one atomic add,
//!   and draws the random tails for the whole block with one OS read.
//! - The 42-bit counter field makes IDs unique within the process by
//!   construction: two IDs share a counter value only 2^42 IDs apart.
//! - Timestamps are read when an ID is handed out, so a pooled ID is never
//!   stale; IDs are monotonic per thread.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
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

/// A UUIDv7 unique within this process, without a process-wide lock.
pub(crate) fn now_v7() -> Uuid {
    RESERVED.with_borrow_mut(|block| {
        if block.left == 0 {
            block.refill(sequence().fetch_add(BLOCK as u64, Ordering::Relaxed));
        }
        block.take(unix_millis())
    })
}

/// The process-wide sequence, starting at a random value so that separate
/// processes don't walk the same counters.
fn sequence() -> &'static AtomicU64 {
    static SEQUENCE: OnceLock<AtomicU64> = OnceLock::new();
    SEQUENCE.get_or_init(|| {
        let mut seed = [0; 8];
        fill_random(&mut seed);
        AtomicU64::new(u64::from_ne_bytes(seed))
    })
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
        .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
}

/// One thread's reserved counter values and their random tails.
struct Block {
    next: u64,
    left: usize,
    tails: [u8; BLOCK * TAIL_BYTES],
    /// The (millisecond, counter) of the last ID handed out.
    last: Option<(u64, u64)>,
}

impl Block {
    const fn new() -> Self {
        Self {
            next: 0,
            left: 0,
            tails: [0; BLOCK * TAIL_BYTES],
            last: None,
        }
    }

    fn refill(&mut self, start: u64) {
        self.next = start;
        self.left = BLOCK;
        fill_random(&mut self.tails);
    }

    fn take(&mut self, now_ms: u64) -> Uuid {
        let counter = self.next & COUNTER_MASK;
        let offset = BLOCK.saturating_sub(self.left).saturating_mul(TAIL_BYTES);
        let tail = self
            .tails
            .get(offset..offset.saturating_add(TAIL_BYTES))
            .and_then(|bytes| <[u8; TAIL_BYTES]>::try_from(bytes).ok())
            .map_or(0, u32::from_be_bytes);
        self.next = self.next.wrapping_add(1);
        self.left = self.left.saturating_sub(1);

        // Admission: each ID must sort after this thread's last one. A clock
        // step back keeps the last millisecond; the counter wrapping within a
        // millisecond moves to the next one, as uuid's own context does.
        let ms = match self.last {
            Some((last_ms, last_counter)) if now_ms <= last_ms => {
                if counter > last_counter {
                    last_ms
                } else {
                    last_ms.saturating_add(1)
                }
            }
            _ => now_ms,
        };
        self.last = Some((ms, counter));
        build(ms, counter, tail)
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
    fn ids_never_share_a_counter_across_threads() -> anyhow::Result<()> {
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
        assert!(ids.iter().all(|id| id.get_variant() == uuid::Variant::RFC4122));
        // Unique without counting on the random tails.
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
    fn admission_keeps_order_across_a_clock_step_back_and_a_counter_wrap() {
        let mut block = Block::new();
        block.refill(COUNTER_MASK - 1);
        let ids: Vec<Uuid> = [1_000, 999, 999]
            .into_iter()
            .map(|ms| block.take(ms))
            .collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(
            ids.iter().map(ms_and_counter).collect::<Vec<_>>(),
            [(1_000, COUNTER_MASK - 1), (1_000, COUNTER_MASK), (1_001, 0)]
        );
    }
}
