//! How many concurrent connections one guest may hold.
//!
//! A [`GuestConnectionQuota`] is a per-guest allowance, split by surface so an
//! operator can size each independently:
//!
//! | Surface            | Counts                                            |
//! | ------------------ | ------------------------------------------------- |
//! | `outbound_http`    | pooled `wasi:http` + gRPC, idle ones included       |
//! | `outbound_sockets` | raw `wasi:sockets` connections the guest opens      |
//! | `inbound_sockets`  | published-port splices arriving at the guest        |
//!
//! Every surface rolls up into an optional host-wide ceiling, so one guest
//! cannot exhaust the machine's file descriptors and a crowd of guests cannot
//! either.
//!
//! # Why the surfaces are separate rather than one number
//!
//! They behave differently in ways a single counter cannot express:
//!
//! - **`outbound_http` may wait.** The pooled client owns its connections and can
//!   abandon an attempt, so it races a permit against an idle pooled
//!   connection freeing and times out if neither arrives. A permit is held for
//!   a *connection's* life, including while it sits idle in the keep-alive
//!   pool — so reuse costs nothing, and the number is really "how large may
//!   this guest's pool grow".
//! - **`outbound_sockets` must never wait.** A guest holds sockets across yield points,
//!   so blocking connect N+1 on a slot that only the guest's own progress can
//!   free is a self-deadlock. [`GuestConnectionQuota::try_acquire_outbound_socket`]
//!   refuses immediately instead.
//! - **`inbound_sockets` must be its own counter.** A guest whose inbound splices drew
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

/// What the host-wide ceiling falls back to when the real descriptor limit
/// cannot be read. Half of the 1024 soft limit common on Linux.
const ASSUMED_MAX_CONNECTIONS: usize = 512;

/// Share of the process's file-descriptor budget guest connections may hold.
///
/// The rest is for everything a connection is not: listening sockets, OCI
/// pulls, the control-plane connection, open files, and the descriptors a
/// connection needs *around* it while being established.
const FD_BUDGET_NUMERATOR: usize = 1;
const FD_BUDGET_DENOMINATOR: usize = 2;

/// Bounds on the derived ceiling. The floor keeps a host with a tiny limit
/// usable; the cap stops a host with an enormous one from setting a number so
/// large it stops being a bound at all.
const MIN_DERIVED_MAX_CONNECTIONS: usize = 64;
const MAX_DERIVED_MAX_CONNECTIONS: usize = 32_768;

/// Host-wide ceiling on live connections when the operator names none.
///
/// Derived from `RLIMIT_NOFILE` rather than assumed, because the number that
/// matters is the process's actual descriptor budget: a container started with
/// 1024 and one started with 1M want very different ceilings, and a fixed
/// default is wrong for both. Every guest's surfaces draw on this, so it — not
/// the per-guest ceilings, which are each larger — is what stops a crowd of
/// workloads exhausting the host's descriptors.
///
/// This is a *bound*, not a reservation: nothing is preallocated, so a generous
/// limit costs nothing until connections are actually opened.
pub fn default_max_connections() -> usize {
    let Some(soft) = descriptor_soft_limit() else {
        return ASSUMED_MAX_CONNECTIONS;
    };
    soft.saturating_mul(FD_BUDGET_NUMERATOR)
        .saturating_div(FD_BUDGET_DENOMINATOR)
        .clamp(MIN_DERIVED_MAX_CONNECTIONS, MAX_DERIVED_MAX_CONNECTIONS)
}

/// The process's soft `RLIMIT_NOFILE`, or `None` if it cannot be read or is
/// unlimited — in which case there is no budget to take a share of.
#[cfg(unix)]
fn descriptor_soft_limit() -> Option<usize> {
    let limits = rustix::process::getrlimit(rustix::process::Resource::Nofile);
    usize::try_from(limits.current?).ok()
}

/// Windows has no `RLIMIT_NOFILE` to derive from: sockets are handles bounded
/// by memory and the per-process handle limit rather than by a descriptor
/// budget, so there is no share to take and the ceiling falls back to
/// [`ASSUMED_MAX_CONNECTIONS`].
#[cfg(not(unix))]
fn descriptor_soft_limit() -> Option<usize> {
    None
}

/// Per-guest ceilings, one per surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaLimits {
    /// Outbound pooled `wasi:http` and gRPC connections, counting idle
    /// keep-alive ones.
    pub outbound_http: usize,
    /// Outbound raw `wasi:sockets` connections the guest holds open.
    pub outbound_sockets: usize,
    /// Inbound published-port splices arriving at this guest.
    pub inbound_sockets: usize,
}

impl Default for QuotaLimits {
    fn default() -> Self {
        Self {
            outbound_http: 128,
            outbound_sockets: 256,
            inbound_sockets: 256,
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
    outbound_http: Arc<Semaphore>,
    outbound_sockets: Arc<Semaphore>,
    inbound_sockets: Arc<Semaphore>,
    /// Host-wide ceiling every surface rolls up into, when one is configured.
    global: Option<Arc<Semaphore>>,
    stats: Arc<QuotaStats>,
}

#[derive(Debug, Default)]
struct QuotaStats {
    outbound_http_granted: AtomicU64,
    outbound_http_refused: AtomicU64,
    outbound_sockets_granted: AtomicU64,
    outbound_sockets_refused: AtomicU64,
    inbound_sockets_granted: AtomicU64,
    inbound_sockets_refused: AtomicU64,
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
            outbound_http: Arc::new(Semaphore::new(limits.outbound_http.max(1))),
            outbound_sockets: Arc::new(Semaphore::new(limits.outbound_sockets.max(1))),
            inbound_sockets: Arc::new(Semaphore::new(limits.inbound_sockets.max(1))),
            global,
            stats: Arc::default(),
        }
    }

    /// Take a slot for a raw socket connection, or `None` if the guest is at
    /// its ceiling.
    ///
    /// Never waits — see the module docs.
    pub fn try_acquire_outbound_socket(&self) -> Option<ConnectionSlot> {
        self.try_acquire(
            &self.outbound_sockets,
            &self.stats.outbound_sockets_granted,
            &self.stats.outbound_sockets_refused,
        )
    }

    /// Take a slot for an outbound HTTP call the host serves without opening a
    /// connection — a same-host locally routed request.
    ///
    /// Never waits, for the same reason the socket surfaces do not: the only
    /// thing that frees a slot here is another of this guest's calls finishing,
    /// and a guest holds a call across yield points, so waiting would let it
    /// deadlock against itself. A pooled network request *does* wait, because
    /// there an idle connection returning to the pool can satisfy it.
    pub fn try_acquire_outbound_http(&self) -> Option<ConnectionSlot> {
        self.try_acquire(
            &self.outbound_http,
            &self.stats.outbound_http_granted,
            &self.stats.outbound_http_refused,
        )
    }

    /// Take a slot for a published-port splice.
    pub fn try_acquire_inbound_socket(&self) -> Option<ConnectionSlot> {
        self.try_acquire(
            &self.inbound_sockets,
            &self.stats.inbound_sockets_granted,
            &self.stats.inbound_sockets_refused,
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
    pub fn outbound_http_permits(&self) -> Arc<Semaphore> {
        Arc::clone(&self.outbound_http)
    }

    /// The host-wide ceiling, for the same reason.
    pub fn global_permits(&self) -> Option<Arc<Semaphore>> {
        self.global.clone()
    }

    pub fn outbound_sockets_available(&self) -> usize {
        self.outbound_sockets.available_permits()
    }

    pub fn inbound_sockets_available(&self) -> usize {
        self.inbound_sockets.available_permits()
    }

    pub fn outbound_http_available(&self) -> usize {
        self.outbound_http.available_permits()
    }

    /// Grants and refusals per surface, for reporting.
    pub fn counts(&self) -> QuotaCounts {
        QuotaCounts {
            outbound_http_granted: self.stats.outbound_http_granted.load(Ordering::Relaxed),
            outbound_http_refused: self.stats.outbound_http_refused.load(Ordering::Relaxed),
            outbound_sockets_granted: self.stats.outbound_sockets_granted.load(Ordering::Relaxed),
            outbound_sockets_refused: self.stats.outbound_sockets_refused.load(Ordering::Relaxed),
            inbound_sockets_granted: self.stats.inbound_sockets_granted.load(Ordering::Relaxed),
            inbound_sockets_refused: self.stats.inbound_sockets_refused.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot of one quota's activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaCounts {
    pub outbound_http_granted: u64,
    pub outbound_http_refused: u64,
    pub outbound_sockets_granted: u64,
    pub outbound_sockets_refused: u64,
    pub inbound_sockets_granted: u64,
    pub inbound_sockets_refused: u64,
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
mod outbound_http_tests {
    use super::*;

    /// Local dispatch takes its slot here, so the surface has to bound and then
    /// recover: a guest at its ceiling is refused, and a finished call gives the
    /// slot back rather than leaking it.
    #[test]
    fn outbound_http_slots_bound_and_recover() {
        let quota = GuestConnectionQuota::new(
            QuotaLimits {
                outbound_http: 2,
                ..Default::default()
            },
            None,
        );

        let a = quota.try_acquire_outbound_http().expect("first slot");
        let b = quota.try_acquire_outbound_http().expect("second slot");
        assert!(
            quota.try_acquire_outbound_http().is_none(),
            "a guest at its ceiling must be refused, not queued"
        );

        drop(a);
        let c = quota
            .try_acquire_outbound_http()
            .expect("slot freed on drop");
        drop((b, c));

        let counts = quota.counts();
        assert_eq!(counts.outbound_http_granted, 3);
        assert_eq!(counts.outbound_http_refused, 1);
    }

    /// The host-wide ceiling binds too, so one busy guest cannot exhaust the
    /// host through the local path any more than through the pooled one.
    #[test]
    fn outbound_http_slots_respect_the_host_wide_ceiling() {
        let global = Arc::new(Semaphore::new(1));
        let limits = QuotaLimits {
            outbound_http: 8,
            ..Default::default()
        };
        let one = GuestConnectionQuota::new(limits, Some(Arc::clone(&global)));
        let two = GuestConnectionQuota::new(limits, Some(global));

        let held = one.try_acquire_outbound_http().expect("host slot");
        assert!(
            two.try_acquire_outbound_http().is_none(),
            "the host-wide ceiling is spent, even though this guest is idle"
        );
        drop(held);
        assert!(two.try_acquire_outbound_http().is_some());
    }
}

#[cfg(test)]
mod tests {
    /// The ceiling tracks the descriptor budget the process actually has: a
    /// container started with 1024 and one started with a million want very
    /// different numbers, and the point of deriving it is that neither has to
    /// say so.
    #[test]
    fn the_default_ceiling_follows_the_descriptor_limit() {
        let derived = super::default_max_connections();
        assert!(
            derived >= super::MIN_DERIVED_MAX_CONNECTIONS,
            "a host with a small limit must still be usable"
        );
        assert!(
            derived <= super::MAX_DERIVED_MAX_CONNECTIONS,
            "a number this large would stop being a bound"
        );

        // Whatever this machine's limit is, the ceiling leaves at least as many
        // descriptors for everything a connection is not.
        if let Some(soft) = super::descriptor_soft_limit()
            && soft >= super::MIN_DERIVED_MAX_CONNECTIONS * 2
            && soft / 2 <= super::MAX_DERIVED_MAX_CONNECTIONS
        {
            assert_eq!(derived, soft / 2);
            assert!(
                derived <= soft - derived,
                "half the budget is left for listeners, pulls, and open files"
            );
        }
    }
    use super::*;

    fn quota(outbound_sockets: usize, inbound_sockets: usize) -> GuestConnectionQuota {
        GuestConnectionQuota::new(
            QuotaLimits {
                outbound_http: 4,
                outbound_sockets,
                inbound_sockets,
            },
            None,
        )
    }

    #[test]
    fn a_quota_hands_out_its_ceiling_and_then_refuses() {
        let quota = quota(2, 1);
        let a = quota.try_acquire_outbound_socket().expect("first slot");
        let b = quota.try_acquire_outbound_socket().expect("second slot");
        assert!(
            quota.try_acquire_outbound_socket().is_none(),
            "third should be refused"
        );

        drop(a);
        assert!(
            quota.try_acquire_outbound_socket().is_some(),
            "a freed slot is reusable"
        );
        drop(b);

        let counts = quota.counts();
        assert_eq!(counts.outbound_sockets_granted, 3);
        assert_eq!(counts.outbound_sockets_refused, 1);
    }

    /// The surfaces must not share: a guest that filled one still has to be
    /// able to use the others.
    #[test]
    fn the_surfaces_do_not_starve_each_other() {
        let quota = quota(1, 1);
        let _inbound = quota.try_acquire_inbound_socket().expect("inbound slot");
        assert!(quota.try_acquire_inbound_socket().is_none());
        assert!(
            quota.try_acquire_outbound_socket().is_some(),
            "sockets must survive inbound exhaustion"
        );
        // And HTTP is untouched by either.
        assert_eq!(quota.outbound_http_available(), 4);
    }

    #[test]
    fn the_host_wide_ceiling_bounds_every_guest_together() {
        let registry = QuotaRegistry::new(QuotaLimits::default(), Some(1));
        let one = registry.for_guest("a");
        let two = registry.for_guest("b");

        let _held = one
            .try_acquire_outbound_socket()
            .expect("first guest takes the slot");
        assert!(
            two.try_acquire_outbound_socket().is_none(),
            "the second guest is bounded by the host-wide ceiling"
        );
    }

    /// A guest at its own ceiling must not consume host-wide capacity on the
    /// way to being refused, or one greedy guest starves everyone else.
    #[test]
    fn a_guest_at_its_ceiling_does_not_touch_the_host_wide_one() {
        let registry = QuotaRegistry::new(
            QuotaLimits {
                outbound_http: 1,
                outbound_sockets: 1,
                inbound_sockets: 1,
            },
            Some(4),
        );
        let quota = registry.for_guest("a");
        let _held = quota.try_acquire_outbound_socket().expect("its one slot");
        assert!(quota.try_acquire_outbound_socket().is_none());
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
                outbound_http: 4,
                outbound_sockets: 1,
                inbound_sockets: 1,
            },
            None,
        );
        let _a = registry
            .for_guest("a")
            .try_acquire_outbound_socket()
            .expect("guest a's slot");
        assert!(
            registry
                .for_guest("b")
                .try_acquire_outbound_socket()
                .is_some(),
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
                outbound_http: 4,
                outbound_sockets: 1,
                inbound_sockets: 1,
            },
            None,
        );
        let _held = registry
            .for_guest("a")
            .try_acquire_outbound_socket()
            .expect("first slot");
        assert!(
            registry
                .for_guest("a")
                .try_acquire_outbound_socket()
                .is_none(),
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
