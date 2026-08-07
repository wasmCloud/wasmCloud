//! How many concurrent connections one guest may hold.
//!
//! A [`GuestConnectionQuota`] is a per-guest allowance, split by surface so an
//! operator can size each independently:
//!
//! | Surface    | Counts                                                    |
//! | ---------- | --------------------------------------------------------- |
//! | `http`     | pooled `wasi:http` + gRPC connections, idle ones included  |
//! | `sockets`  | raw `wasi:sockets` connections the guest holds             |
//! | `inbound`  | published-port splices arriving at the guest               |
//!
//! Every surface rolls up into an optional host-wide ceiling, so one guest
//! cannot exhaust the machine's file descriptors and a crowd of guests cannot
//! either.
//!
//! # Why the surfaces are separate rather than one number
//!
//! They behave differently in ways a single counter cannot express:
//!
//! - **`http` may wait.** The pooled client owns its connections and can
//!   abandon an attempt, so it races a permit against an idle pooled
//!   connection freeing and times out if neither arrives. A permit is held for
//!   a *connection's* life, including while it sits idle in the keep-alive
//!   pool — so reuse costs nothing, and the number is really "how large may
//!   this guest's pool grow".
//! - **`sockets` must never wait.** A guest holds sockets across yield points,
//!   so blocking connect N+1 on a slot that only the guest's own progress can
//!   free is a self-deadlock. [`GuestConnectionQuota::try_acquire_socket`]
//!   refuses immediately instead.
//! - **`inbound` must be its own counter.** A guest whose inbound splices drew
//!   from the same allowance as its outbound calls would deadlock against
//!   itself: serving a request needs an outbound call, the call needs a slot,
//!   and the slot is held by the request that is waiting.
//!
//! # One quota per guest, not one per host
//!
//! Quotas are minted by a [`QuotaRegistry`] keyed on guest id. This is what
//! makes a per-guest limit actually per-guest: a single quota shared by every
//! guest on the host would make the number a host-wide cap wearing a
//! per-guest name.

use core::time::Duration;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::sockets::DenyReason;

/// How long a per-guest quota is kept after its last use.
///
/// Outliving the guest matters: connections opened by a replaced client keep
/// their slots until they close, and those slots have to keep counting against
/// the guest that opened them.
const QUOTA_IDLE: Duration = Duration::from_secs(300);

/// Default for [`QuotaRegistry::with_http_wait`]. Kept well below the
/// request-timeout defaults (600s) so saturation surfaces as a prompt,
/// classifiable connect timeout instead of a long hang.
const DEFAULT_HTTP_WAIT: Duration = Duration::from_secs(5);

/// Host-wide ceiling on live connections when the operator names none.
///
/// Kept inside common default file-descriptor soft limits (1024 on many Linux
/// distributions) with room left for ingress connections, OCI pulls, and the
/// host's own control-plane traffic. Every guest's three surfaces draw on this,
/// so it — not the per-guest ceilings, which are each larger — is what stops a
/// crowd of workloads exhausting the host's descriptors.
pub const DEFAULT_MAX_CONNECTIONS: usize = 512;

/// Per-guest ceilings, one per surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaLimits {
    /// Pooled `wasi:http` and gRPC connections, counting idle keep-alive ones.
    pub http: usize,
    /// Raw `wasi:sockets` connections the guest holds open.
    pub sockets: usize,
    /// Published-port splices arriving at this guest.
    pub inbound: usize,
}

impl Default for QuotaLimits {
    fn default() -> Self {
        Self {
            http: 128,
            sockets: 256,
            inbound: 256,
        }
    }
}

/// One guest's share of the host's connection capacity.
///
/// Cloning shares the same allowance; dropping every clone returns it, which is
/// how tearing a guest down reclaims capacity immediately rather than waiting
/// out an idle timeout.
#[derive(Debug, Clone)]
pub struct GuestConnectionQuota {
    http: Arc<Semaphore>,
    sockets: Arc<Semaphore>,
    inbound: Arc<Semaphore>,
    /// Host-wide ceiling every surface rolls up into, when one is configured.
    global: Option<Arc<Semaphore>>,
    stats: Arc<QuotaStats>,
}

#[derive(Debug, Default)]
struct QuotaStats {
    sockets_granted: AtomicU64,
    sockets_refused: AtomicU64,
    inbound_granted: AtomicU64,
    inbound_refused: AtomicU64,
}

/// A held connection slot. Returns its capacity on drop.
///
/// Carries the per-guest permit and, when the host is capped, the host-wide one
/// too; both are released together.
#[derive(Debug)]
pub struct ConnectionSlot {
    _guest: OwnedSemaphorePermit,
    _global: Option<OwnedSemaphorePermit>,
}

impl GuestConnectionQuota {
    pub fn new(limits: QuotaLimits, global: Option<Arc<Semaphore>>) -> Self {
        Self {
            http: Arc::new(Semaphore::new(limits.http.max(1))),
            sockets: Arc::new(Semaphore::new(limits.sockets.max(1))),
            inbound: Arc::new(Semaphore::new(limits.inbound.max(1))),
            global,
            stats: Arc::default(),
        }
    }

    /// Take a slot for a raw socket connection, or `None` if the guest is at
    /// its ceiling.
    ///
    /// Never waits — see the module docs.
    pub fn try_acquire_socket(&self) -> Option<ConnectionSlot> {
        self.try_acquire(
            &self.sockets,
            &self.stats.sockets_granted,
            &self.stats.sockets_refused,
        )
    }

    /// Take a slot for a published-port splice.
    pub fn try_acquire_inbound(&self) -> Option<ConnectionSlot> {
        self.try_acquire(
            &self.inbound,
            &self.stats.inbound_granted,
            &self.stats.inbound_refused,
        )
    }

    fn try_acquire(
        &self,
        surface: &Arc<Semaphore>,
        granted: &AtomicU64,
        refused: &AtomicU64,
    ) -> Option<ConnectionSlot> {
        // The guest's own surface first, so a guest at its ceiling does not
        // take host-wide capacity it will immediately hand back.
        let Ok(guest) = Arc::clone(surface).try_acquire_owned() else {
            refused.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        let global = match &self.global {
            Some(global) => match Arc::clone(global).try_acquire_owned() {
                Ok(permit) => Some(permit),
                Err(_) => {
                    refused.fetch_add(1, Ordering::Relaxed);
                    return None;
                }
            },
            None => None,
        };
        granted.fetch_add(1, Ordering::Relaxed);
        Some(ConnectionSlot {
            _guest: guest,
            _global: global,
        })
    }

    /// The HTTP surface's permits, for the pooled client to draw on directly.
    ///
    /// Handed out as a raw semaphore rather than through `try_acquire` because
    /// the pool needs to *wait* on it, racing the wait against an idle
    /// connection freeing — behavior `try_acquire` deliberately does not have.
    pub fn http_permits(&self) -> Arc<Semaphore> {
        Arc::clone(&self.http)
    }

    /// The host-wide ceiling, for the same reason.
    pub fn global_permits(&self) -> Option<Arc<Semaphore>> {
        self.global.clone()
    }

    pub fn sockets_available(&self) -> usize {
        self.sockets.available_permits()
    }

    pub fn inbound_available(&self) -> usize {
        self.inbound.available_permits()
    }

    pub fn http_available(&self) -> usize {
        self.http.available_permits()
    }

    /// Grants and refusals per surface, for reporting.
    pub fn counts(&self) -> QuotaCounts {
        QuotaCounts {
            sockets_granted: self.stats.sockets_granted.load(Ordering::Relaxed),
            sockets_refused: self.stats.sockets_refused.load(Ordering::Relaxed),
            inbound_granted: self.stats.inbound_granted.load(Ordering::Relaxed),
            inbound_refused: self.stats.inbound_refused.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot of one quota's activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaCounts {
    pub sockets_granted: u64,
    pub sockets_refused: u64,
    pub inbound_granted: u64,
    pub inbound_refused: u64,
}

/// Mints and remembers one [`GuestConnectionQuota`] per guest.
///
/// Every surface — the socket policy, the pooled HTTP client, the published-port
/// publisher — looks a guest up here, so all of them draw on the same
/// allowance and an operator configures one set of numbers.
#[derive(Debug, Clone)]
pub struct QuotaRegistry {
    limits: QuotaLimits,
    global: Option<Arc<Semaphore>>,
    http_wait: Duration,
    quotas: moka::sync::Cache<String, GuestConnectionQuota>,
}

impl QuotaRegistry {
    /// A registry handing out `limits` to each guest, with an optional
    /// host-wide ceiling every guest rolls up into.
    pub fn new(limits: QuotaLimits, host_wide: Option<usize>) -> Arc<Self> {
        Arc::new(Self {
            limits,
            global: host_wide.map(|total| Arc::new(Semaphore::new(total.max(1)))),
            http_wait: DEFAULT_HTTP_WAIT,
            quotas: moka::sync::Cache::builder()
                .time_to_idle(QUOTA_IDLE)
                .build(),
        })
    }

    /// How long a pooled HTTP connect attempt waits for a slot before failing
    /// with a connect timeout.
    ///
    /// Only the HTTP surface has this: it is the one that waits. The deadline
    /// has to exist because hyper's pool spawns an already-started connect to
    /// completion in the background when idle-connection reuse wins the
    /// checkout race, and such an abandoned attempt parked on the semaphore
    /// would otherwise camp there indefinitely — holding the pool alive (which
    /// pins the very idle-connection slots it is waiting for) and grabbing
    /// freed slots to open connections nobody asked for.
    ///
    /// A guest's own `connect-timeout` bounds its request independently, so
    /// this caps only how long an abandoned attempt may camp.
    #[must_use]
    pub fn with_http_wait(mut self, wait: Duration) -> Self {
        self.http_wait = wait;
        self
    }

    pub fn http_wait(&self) -> Duration {
        self.http_wait
    }

    /// This guest's quota, created on first use.
    ///
    /// `guest_id` must be the host-assigned identifier — a workload id or a
    /// plugin id — never anything a guest can choose, or two guests could
    /// collapse into one allowance.
    pub fn for_guest(&self, guest_id: &str) -> GuestConnectionQuota {
        self.quotas.get_with_by_ref(guest_id, || {
            GuestConnectionQuota::new(self.limits, self.global.clone())
        })
    }

    pub fn limits(&self) -> QuotaLimits {
        self.limits
    }
}

/// Counters for what the socket policy decided.
///
/// `would_deny` is the migration signal: while the host runs in count mode it
/// records every refusal enforcement *would* have made, so an operator can see
/// the blast radius before turning it on. If these are non-zero, enforcing
/// breaks someone.
#[derive(Debug, Default)]
pub struct PolicyMeters {
    denied: [AtomicU64; DENY_REASONS],
    would_deny: [AtomicU64; DENY_REASONS],
}

const DENY_REASONS: usize = 6;

fn reason_index(reason: DenyReason) -> usize {
    match reason {
        DenyReason::NotPermitted => 0,
        DenyReason::BindNotPermitted => 1,
        DenyReason::HostLoopbackNotPermitted => 2,
        DenyReason::HostOwnedPort => 3,
        DenyReason::BlockedRange => 4,
        DenyReason::NoCapacity => 5,
    }
}

impl PolicyMeters {
    pub fn record_deny(&self, reason: DenyReason) {
        if let Some(counter) = self.denied.get(reason_index(reason)) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_would_deny(&self, reason: DenyReason) {
        if let Some(counter) = self.would_deny.get(reason_index(reason)) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn denied(&self, reason: DenyReason) -> u64 {
        self.denied
            .get(reason_index(reason))
            .map_or(0, |c| c.load(Ordering::Relaxed))
    }

    pub fn would_deny(&self, reason: DenyReason) -> u64 {
        self.would_deny
            .get(reason_index(reason))
            .map_or(0, |c| c.load(Ordering::Relaxed))
    }

    /// Every reason with a non-zero count, for a status line or a metric sweep.
    pub fn nonzero(&self) -> Vec<(DenyReason, u64, u64)> {
        [
            DenyReason::NotPermitted,
            DenyReason::BindNotPermitted,
            DenyReason::HostLoopbackNotPermitted,
            DenyReason::HostOwnedPort,
            DenyReason::BlockedRange,
            DenyReason::NoCapacity,
        ]
        .into_iter()
        .map(|r| (r, self.denied(r), self.would_deny(r)))
        .filter(|(_, d, w)| *d > 0 || *w > 0)
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quota(sockets: usize, inbound: usize) -> GuestConnectionQuota {
        GuestConnectionQuota::new(
            QuotaLimits {
                http: 4,
                sockets,
                inbound,
            },
            None,
        )
    }

    #[test]
    fn a_quota_hands_out_its_ceiling_and_then_refuses() {
        let quota = quota(2, 1);
        let a = quota.try_acquire_socket().expect("first slot");
        let b = quota.try_acquire_socket().expect("second slot");
        assert!(
            quota.try_acquire_socket().is_none(),
            "third should be refused"
        );

        drop(a);
        assert!(
            quota.try_acquire_socket().is_some(),
            "a freed slot is reusable"
        );
        drop(b);

        let counts = quota.counts();
        assert_eq!(counts.sockets_granted, 3);
        assert_eq!(counts.sockets_refused, 1);
    }

    /// The surfaces must not share: a guest that filled one still has to be
    /// able to use the others.
    #[test]
    fn the_surfaces_do_not_starve_each_other() {
        let quota = quota(1, 1);
        let _inbound = quota.try_acquire_inbound().expect("inbound slot");
        assert!(quota.try_acquire_inbound().is_none());
        assert!(
            quota.try_acquire_socket().is_some(),
            "sockets must survive inbound exhaustion"
        );
        // And HTTP is untouched by either.
        assert_eq!(quota.http_available(), 4);
    }

    #[test]
    fn the_host_wide_ceiling_bounds_every_guest_together() {
        let registry = QuotaRegistry::new(QuotaLimits::default(), Some(1));
        let one = registry.for_guest("a");
        let two = registry.for_guest("b");

        let _held = one
            .try_acquire_socket()
            .expect("first guest takes the slot");
        assert!(
            two.try_acquire_socket().is_none(),
            "the second guest is bounded by the host-wide ceiling"
        );
    }

    /// A guest at its own ceiling must not consume host-wide capacity on the
    /// way to being refused, or one greedy guest starves everyone else.
    #[test]
    fn a_guest_at_its_ceiling_does_not_touch_the_host_wide_one() {
        let registry = QuotaRegistry::new(
            QuotaLimits {
                http: 1,
                sockets: 1,
                inbound: 1,
            },
            Some(4),
        );
        let quota = registry.for_guest("a");
        let _held = quota.try_acquire_socket().expect("its one slot");
        assert!(quota.try_acquire_socket().is_none());
        assert_eq!(
            quota.global_permits().unwrap().available_permits(),
            3,
            "a refused attempt must not hold host-wide capacity"
        );
    }

    /// The whole point of the registry: a per-guest limit that is actually per
    /// guest, rather than one allowance wearing a per-guest name.
    #[test]
    fn each_guest_gets_its_own_allowance() {
        let registry = QuotaRegistry::new(
            QuotaLimits {
                http: 4,
                sockets: 1,
                inbound: 1,
            },
            None,
        );
        let _a = registry
            .for_guest("a")
            .try_acquire_socket()
            .expect("guest a's slot");
        assert!(
            registry.for_guest("b").try_acquire_socket().is_some(),
            "one guest at its ceiling must not exhaust another's"
        );
    }

    /// Looking a guest up twice must return the same allowance, or a guest
    /// whose client is rebuilt would get a second full ceiling while its old
    /// connections drain.
    #[test]
    fn a_guest_lookup_is_stable() {
        let registry = QuotaRegistry::new(
            QuotaLimits {
                http: 4,
                sockets: 1,
                inbound: 1,
            },
            None,
        );
        let _held = registry
            .for_guest("a")
            .try_acquire_socket()
            .expect("first slot");
        assert!(
            registry.for_guest("a").try_acquire_socket().is_none(),
            "the same guest must see the same allowance"
        );
    }

    #[test]
    fn policy_meters_separate_enforced_from_would_be_denials() {
        let meters = PolicyMeters::default();
        meters.record_deny(DenyReason::NotPermitted);
        meters.record_deny(DenyReason::NotPermitted);
        meters.record_would_deny(DenyReason::BlockedRange);

        assert_eq!(meters.denied(DenyReason::NotPermitted), 2);
        assert_eq!(meters.would_deny(DenyReason::NotPermitted), 0);
        assert_eq!(meters.would_deny(DenyReason::BlockedRange), 1);
        assert_eq!(meters.nonzero().len(), 2);
    }
}
