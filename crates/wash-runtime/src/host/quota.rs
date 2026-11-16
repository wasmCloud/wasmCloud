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

use core::hash::BuildHasher;
use core::time::Duration;
use std::collections::HashMap;
use std::hash::RandomState;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::sockets::DenyReason;

/// Default for [`QuotaRegistry::with_http_wait`]. Kept well below the
/// request-timeout defaults (600s) so saturation surfaces as a prompt,
/// classifiable connect timeout instead of a long hang.
pub(crate) const DEFAULT_HTTP_WAIT: Duration = Duration::from_secs(5);

/// What the host-wide ceiling falls back to on a platform with no descriptor
/// limit to read — Windows. Half of the 1024 soft limit common on Linux.
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

/// Share of the descriptor budget the host's own HTTP ingress may hold in
/// accepted connections.
///
/// Its own share rather than the guest one above, because the two fail
/// differently: a guest refused a connection gets an error it can report, while
/// ingress out of descriptors cannot accept at all — and an accept loop with
/// nothing to accept is what makes a live host look dead to a TCP probe.
const INGRESS_BUDGET_NUMERATOR: usize = 1;
const INGRESS_BUDGET_DENOMINATOR: usize = 4;

/// Floor under the ingress share, above the one guests get. A share of a small
/// desktop limit lands near 64, and a server that can hold 64 connections is
/// not usefully a server — a `wash dev` or a benchmark would sit at the ceiling.
const MIN_INGRESS_CONNECTIONS: usize = 256;

/// Ceiling on the raise [`raise_descriptor_limit`] performs.
///
/// Linux's own default `fs.nr_open`, and the point past which more descriptors
/// buy nothing. An unlimited soft limit is worse than a large one: it leaves no
/// number for the ceilings below to take a share of.
#[cfg(any(unix, test))]
const MAX_RAISED_DESCRIPTORS: u64 = 1_048_576;

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
    descriptor_share(FD_BUDGET_NUMERATOR, FD_BUDGET_DENOMINATOR)
}

/// Host-wide ceiling on the TCP connections the HTTP ingress holds, when the
/// operator names none.
///
/// Counted where they are accepted, before TLS and before a protocol is known,
/// so this bounds sockets rather than requests: an idle keep-alive connection
/// holds a slot, and an HTTP/2 connection holds one however many streams it
/// carries. They are not a guest's to account for — which workload a connection
/// belongs to is unknown until its first request names one — so they draw on
/// their own share rather than on [`default_max_connections`].
pub fn default_max_http_ingress_connections() -> usize {
    descriptor_share(INGRESS_BUDGET_NUMERATOR, INGRESS_BUDGET_DENOMINATOR)
        .max(MIN_INGRESS_CONNECTIONS)
}

/// `numerator/denominator` of the process's descriptor budget, bounded.
fn descriptor_share(numerator: usize, denominator: usize) -> usize {
    share_of(descriptor_soft_limit(), numerator, denominator)
}

/// The share arithmetic on its own, so the ceilings can be checked against a
/// limit this process does not have to be running under.
fn share_of(soft: Option<usize>, numerator: usize, denominator: usize) -> usize {
    let Some(soft) = soft else {
        return ASSUMED_MAX_CONNECTIONS;
    };
    soft.saturating_mul(numerator)
        .saturating_div(denominator)
        .clamp(MIN_DERIVED_MAX_CONNECTIONS, MAX_DERIVED_MAX_CONNECTIONS)
}

/// A soft `RLIMIT_NOFILE` as a budget, given what `getrlimit` reported.
///
/// `None` there means unlimited, which reads as [`MAX_RAISED_DESCRIPTORS`]
/// rather than as no answer: no bound at all is a larger budget than any finite
/// one, and calling it unknown would hand out the smallest share of all.
#[cfg(any(unix, test))]
fn soft_limit_as_budget(reported: Option<u64>) -> Option<usize> {
    usize::try_from(reported.unwrap_or(MAX_RAISED_DESCRIPTORS)).ok()
}

/// The process's soft `RLIMIT_NOFILE` as a budget.
#[cfg(unix)]
fn descriptor_soft_limit() -> Option<usize> {
    soft_limit_as_budget(rustix::process::getrlimit(rustix::process::Resource::Nofile).current)
}

/// Windows has no `RLIMIT_NOFILE` to derive from: sockets are handles bounded
/// by memory and the per-process handle limit rather than by a descriptor
/// budget, so there is no share to take and the ceiling falls back to
/// [`ASSUMED_MAX_CONNECTIONS`].
#[cfg(not(unix))]
fn descriptor_soft_limit() -> Option<usize> {
    None
}

/// The soft descriptor limits worth asking for, largest first, for a process
/// currently at `soft` with a ceiling of `hard`. Empty when there is nothing
/// worth asking for.
///
/// A hard limit of "unlimited" (`None`) is not a number any kernel will take —
/// Linux refuses anything over `fs.nr_open`, macOS over `kern.maxfilesperproc`
/// — and neither reports what it *would* take. So this is a sequence to try
/// rather than one number, halving until the ask is one the process already
/// has.
#[cfg(any(unix, test))]
fn raise_plan(soft: Option<u64>, hard: Option<u64>) -> Vec<u64> {
    let have = soft.unwrap_or(u64::MAX);
    let mut target = hard
        .unwrap_or(MAX_RAISED_DESCRIPTORS)
        .min(MAX_RAISED_DESCRIPTORS);
    let mut plan = Vec::new();
    while target > have {
        plan.push(target);
        target /= 2;
    }
    plan
}

/// Raise the process's soft descriptor limit as far as the kernel allows, and
/// report the limit in force afterwards.
///
/// **An embedder's to call, never the runtime's.** The soft limit is
/// process-global: a library that moved it would be reaching outside the host
/// it was handed, and an embedder with its own opinion could not opt out. What
/// lives here is the policy — how high is worth asking for, and what to do when
/// the kernel says no ([`raise_plan`]); a `wash host`, a `wash dev` or a
/// third-party embedder decides whether to apply it.
///
/// Apply it before deriving any ceiling above, since each is a share of
/// whatever this leaves in place.
#[cfg(unix)]
pub fn raise_descriptor_limit() -> Option<usize> {
    let limits = rustix::process::getrlimit(rustix::process::Resource::Nofile);
    for target in raise_plan(limits.current, limits.maximum) {
        let ask = rustix::process::Rlimit {
            current: Some(target),
            maximum: limits.maximum,
        };
        if rustix::process::setrlimit(rustix::process::Resource::Nofile, ask).is_ok() {
            return descriptor_soft_limit();
        }
    }
    // Not fatal: the host runs on whatever it was given, and every derived
    // ceiling is sized from that same number.
    tracing::debug!(soft = ?limits.current, "left the descriptor limit where it was");
    descriptor_soft_limit()
}

#[cfg(not(unix))]
pub fn raise_descriptor_limit() -> Option<usize> {
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
pub struct GuestConnectionQuota(Arc<QuotaState>);

#[derive(Debug)]
struct QuotaState {
    // Held inline rather than behind `Arc`s of their own: a slot keeps the
    // whole state alive anyway, so minting a quota costs one allocation. That
    // matters because a guest holding nothing between invocations gets a
    // fresh quota per invocation.
    outbound_http: Semaphore,
    outbound_sockets: Semaphore,
    inbound_sockets: Semaphore,
    /// Host-wide ceiling every surface rolls up into, when one is configured.
    global: Option<Arc<Semaphore>>,
    stats: QuotaStats,
    _registration: Option<QuotaRegistration>,
}

#[derive(Debug, Clone, Copy)]
enum Surface {
    OutboundHttp,
    OutboundSockets,
    InboundSockets,
}

impl QuotaState {
    fn semaphore(&self, surface: Surface) -> &Semaphore {
        match surface {
            Surface::OutboundHttp => &self.outbound_http,
            Surface::OutboundSockets => &self.outbound_sockets,
            Surface::InboundSockets => &self.inbound_sockets,
        }
    }

    /// `(granted, refused)` for `surface`.
    fn counters(&self, surface: Surface) -> (&AtomicU64, &AtomicU64) {
        let stats = &self.stats;
        match surface {
            Surface::OutboundHttp => (&stats.outbound_http_granted, &stats.outbound_http_refused),
            Surface::OutboundSockets => (
                &stats.outbound_sockets_granted,
                &stats.outbound_sockets_refused,
            ),
            Surface::InboundSockets => (
                &stats.inbound_sockets_granted,
                &stats.inbound_sockets_refused,
            ),
        }
    }
}

/// How many lock shards a [`QuotaRegistry`] spreads its guests over, so that
/// lookups and teardowns for different guests rarely contend.
const REGISTRY_SHARDS: usize = 16;

/// A shard whose capacity has grown past this is shrunk once it falls to a
/// quarter full, so a burst of short-lived guests does not pin its peak size.
const SHARD_SHRINK_FLOOR: usize = 64;

type Shard = Mutex<HashMap<Arc<str>, Weak<QuotaState>>>;

#[derive(Debug, Default)]
struct GuestQuotas {
    hasher: RandomState,
    shards: [Shard; REGISTRY_SHARDS],
}

impl GuestQuotas {
    #[expect(
        clippy::indexing_slicing,
        reason = "the index is reduced modulo the array length"
    )]
    fn shard(&self, guest_id: &str) -> MutexGuard<'_, HashMap<Arc<str>, Weak<QuotaState>>> {
        // The remainder is below `REGISTRY_SHARDS`, so the cast cannot truncate.
        let index = (self.hasher.hash_one(guest_id) % REGISTRY_SHARDS as u64) as usize;
        self.shards[index]
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.lock().unwrap_or_else(PoisonError::into_inner).len())
            .sum()
    }
}

#[derive(Debug)]
struct QuotaRegistration {
    registry: Weak<GuestQuotas>,
    guest_id: Arc<str>,
}

impl Drop for QuotaRegistration {
    fn drop(&mut self) {
        let Some(registry) = self.registry.upgrade() else {
            return;
        };
        let mut guests = registry.shard(&self.guest_id);
        // A racing lookup can already have replaced this generation.
        if guests
            .get(&self.guest_id)
            .is_some_and(|quota| quota.strong_count() == 0)
        {
            guests.remove(&self.guest_id);
            if guests.capacity() > SHARD_SHRINK_FLOOR && guests.len() * 4 < guests.capacity() {
                let target = guests.len() * 2;
                guests.shrink_to(target);
            }
        }
    }
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
/// too; both are released together. It also keeps the guest's quota alive, so
/// a connection outliving its client or its workload stays charged to it.
#[derive(Debug)]
pub struct ConnectionSlot {
    quota: GuestConnectionQuota,
    surface: Surface,
    _global: Option<OwnedSemaphorePermit>,
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        // The per-guest permit was forgotten when the slot was granted.
        self.quota.0.semaphore(self.surface).add_permits(1);
    }
}

impl GuestConnectionQuota {
    pub fn new(limits: QuotaLimits, global: Option<Arc<Semaphore>>) -> Self {
        Self::with_registration(limits, global, None)
    }

    fn with_registration(
        limits: QuotaLimits,
        global: Option<Arc<Semaphore>>,
        registration: Option<QuotaRegistration>,
    ) -> Self {
        Self(Arc::new(QuotaState {
            outbound_http: Semaphore::new(limits.outbound_http.max(1)),
            outbound_sockets: Semaphore::new(limits.outbound_sockets.max(1)),
            inbound_sockets: Semaphore::new(limits.inbound_sockets.max(1)),
            global,
            stats: QuotaStats::default(),
            _registration: registration,
        }))
    }

    /// Take a slot for a raw socket connection, or `None` if the guest is at
    /// its ceiling.
    ///
    /// Never waits — see the module docs.
    pub fn try_acquire_outbound_socket(&self) -> Option<ConnectionSlot> {
        self.try_acquire(Surface::OutboundSockets)
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
        self.try_acquire(Surface::OutboundHttp)
    }

    /// Take a slot for a published-port splice.
    pub fn try_acquire_inbound_socket(&self) -> Option<ConnectionSlot> {
        self.try_acquire(Surface::InboundSockets)
    }

    fn try_acquire(&self, surface: Surface) -> Option<ConnectionSlot> {
        let state = &*self.0;
        let (granted, refused) = state.counters(surface);
        // The guest's own surface first, so a guest at its ceiling does not
        // take host-wide capacity it will immediately hand back.
        let Ok(guest) = state.semaphore(surface).try_acquire() else {
            refused.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        let global = match &state.global {
            Some(global) => match Arc::clone(global).try_acquire_owned() {
                Ok(permit) => Some(permit),
                Err(_) => {
                    refused.fetch_add(1, Ordering::Relaxed);
                    return None;
                }
            },
            None => None,
        };
        guest.forget();
        granted.fetch_add(1, Ordering::Relaxed);
        Some(self.slot(surface, global))
    }

    /// Wait up to `wait` for a connection slot for the pooled HTTP client, or
    /// `None` if the guest's allowance or the host-wide ceiling stayed full.
    ///
    /// Counted as a grant or a refusal like the non-waiting surfaces. The slot
    /// keeps the workload's quota registered until the connection closes,
    /// including when the client is replaced or the workload stops.
    pub async fn acquire_outbound_http(&self, wait: Duration) -> Option<ConnectionSlot> {
        let state = &*self.0;
        let (granted, refused) = state.counters(Surface::OutboundHttp);
        // Acquire order (guest, then host-wide) is fixed everywhere, and
        // waiters hold no resource another waiter needs, so waiting on both
        // cannot deadlock. Acquiring only fails on a closed semaphore, and
        // these are private and never closed.
        let acquire = async {
            let guest = state.outbound_http.acquire().await.ok()?;
            let global = match &state.global {
                Some(global) => Some(Arc::clone(global).acquire_owned().await.ok()?),
                None => None,
            };
            guest.forget();
            Some(global)
        };
        match tokio::time::timeout(wait, acquire).await {
            Ok(Some(global)) => {
                granted.fetch_add(1, Ordering::Relaxed);
                Some(self.slot(Surface::OutboundHttp, global))
            }
            Ok(None) | Err(_) => {
                refused.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    fn slot(&self, surface: Surface, global: Option<OwnedSemaphorePermit>) -> ConnectionSlot {
        ConnectionSlot {
            quota: self.clone(),
            surface,
            _global: global,
        }
    }

    /// The host-wide ceiling, when one is configured. Exposed for diagnostics
    /// such as the pooled client's exhaustion warning; slots are taken through
    /// the `acquire` methods, never from this directly.
    pub fn global_permits(&self) -> Option<Arc<Semaphore>> {
        self.0.global.clone()
    }

    pub fn outbound_sockets_available(&self) -> usize {
        self.0.outbound_sockets.available_permits()
    }

    pub fn inbound_sockets_available(&self) -> usize {
        self.0.inbound_sockets.available_permits()
    }

    pub fn outbound_http_available(&self) -> usize {
        self.0.outbound_http.available_permits()
    }

    /// Grants and refusals per surface, for reporting.
    ///
    /// The counts cover this quota's current lifetime. Once a guest holds no
    /// slot and every handle to its quota is dropped, the registry forgets it,
    /// and the next [`QuotaRegistry::for_guest`] starts again from zero. A
    /// reporter that wants a guest's full history should hold a handle for as
    /// long as it samples.
    pub fn counts(&self) -> QuotaCounts {
        let load = |surface| {
            let (granted, refused) = self.0.counters(surface);
            (
                granted.load(Ordering::Relaxed),
                refused.load(Ordering::Relaxed),
            )
        };
        let (outbound_http_granted, outbound_http_refused) = load(Surface::OutboundHttp);
        let (outbound_sockets_granted, outbound_sockets_refused) = load(Surface::OutboundSockets);
        let (inbound_sockets_granted, inbound_sockets_refused) = load(Surface::InboundSockets);
        QuotaCounts {
            outbound_http_granted,
            outbound_http_refused,
            outbound_sockets_granted,
            outbound_sockets_refused,
            inbound_sockets_granted,
            inbound_sockets_refused,
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
    quotas: Arc<GuestQuotas>,
}

/// The registry a policy built without one draws on.
///
/// One per process, not one per policy: a registry of its own would give each
/// policy a private "host-wide" ceiling, which is the opposite of a ceiling —
/// two of them would let a host hold the limit twice over. Built on first use,
/// so a host that raises its descriptor limit during startup is measured after
/// the raise rather than before it.
pub fn default_registry() -> Arc<QuotaRegistry> {
    static DEFAULT: std::sync::LazyLock<Arc<QuotaRegistry>> = std::sync::LazyLock::new(|| {
        QuotaRegistry::new(QuotaLimits::default(), Some(default_max_connections()))
    });
    Arc::clone(&DEFAULT)
}

impl QuotaRegistry {
    /// A registry handing out `limits` to each guest, with an optional
    /// host-wide ceiling every guest rolls up into.
    pub fn new(limits: QuotaLimits, host_wide: Option<usize>) -> Arc<Self> {
        Arc::new(Self {
            limits,
            global: host_wide.map(|total| Arc::new(Semaphore::new(total.max(1)))),
            http_wait: DEFAULT_HTTP_WAIT,
            quotas: Arc::default(),
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
        let mut guests = self.quotas.shard(guest_id);
        if let Some(quota) = guests.get(guest_id).and_then(Weak::upgrade) {
            return GuestConnectionQuota(quota);
        }
        let guest_id: Arc<str> = Arc::from(guest_id);
        let quota = GuestConnectionQuota::with_registration(
            self.limits,
            self.global.clone(),
            Some(QuotaRegistration {
                registry: Arc::downgrade(&self.quotas),
                guest_id: Arc::clone(&guest_id),
            }),
        );
        guests.insert(guest_id, Arc::downgrade(&quota.0));
        quota
    }

    pub fn limits(&self) -> QuotaLimits {
        self.limits
    }
}

/// Counters for what the socket policy and the `hostPath` volume gate decided.
///
/// `would_deny` is the count-mode signal: while the host runs in count mode it
/// records every refusal enforcement *would* have made, so an operator can see
/// the blast radius. If these are non-zero, enforcing breaks someone.
#[derive(Debug, Default)]
pub struct PolicyMeters {
    denied: [AtomicU64; DENY_REASONS],
    would_deny: [AtomicU64; DENY_REASONS],
    host_path_denied: AtomicU64,
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

    /// Record a refused `hostPath` volume: one outside the host's allowed host
    /// paths, or one exposing a reserved path, a kernel filesystem, or the
    /// `emptyDir` scratch root.
    pub fn record_host_path_denied(&self) {
        self.host_path_denied.fetch_add(1, Ordering::Relaxed);
    }

    /// `hostPath` volumes the host refused.
    pub fn host_path_denied(&self) -> u64 {
        self.host_path_denied.load(Ordering::Relaxed)
    }

    /// Every reason with a non-zero count, for a status line or a metric sweep.
    pub fn nonzero(&self) -> Vec<(DenyReason, u64, u64)> {
        ALL_REASONS
            .into_iter()
            .map(|r| (r, self.denied(r), self.would_deny(r)))
            .filter(|(_, d, w)| *d > 0 || *w > 0)
            .collect()
    }

    /// These counters, published as metrics on the process-wide meter: the
    /// would-deny figures are what an operator watches before switching the
    /// socket policy to enforce. With no OTel exporter configured the global
    /// meter is a no-op and nothing is ever read.
    pub fn into_metered(self) -> Arc<Self> {
        let meters = Arc::new(self);
        meters.register_metrics(&opentelemetry::global::meter("wash-runtime"));
        meters
    }

    fn register_metrics(self: &Arc<Self>, meter: &opentelemetry::metrics::Meter) {
        // Weak, so the meter provider does not keep a dropped host's counters
        // alive; a callback that cannot upgrade reports nothing.
        let observe = |name: &'static str, doc: &'static str, read: fn(&Self) -> u64| {
            let meters = Arc::downgrade(self);
            let _ = meter
                .u64_observable_counter(name)
                .with_description(doc)
                .with_unit("{volume}")
                .with_callback(move |observer| {
                    if let Some(meters) = meters.upgrade() {
                        observer.observe(read(&meters), &[]);
                    }
                })
                .build();
        };
        observe(
            "host_path.denied",
            "hostPath volumes refused: outside the allowlist, or exposing a reserved path, a \
             kernel filesystem, or the emptyDir scratch root",
            Self::host_path_denied,
        );
        let by_reason =
            |name: &'static str, doc: &'static str, read: fn(&Self, DenyReason) -> u64| {
                let meters = Arc::downgrade(self);
                let _ = meter
                    .u64_observable_counter(name)
                    .with_description(doc)
                    .with_unit("{connection}")
                    .with_callback(move |observer| {
                        if let Some(meters) = meters.upgrade() {
                            for reason in ALL_REASONS {
                                observer.observe(
                                    read(&meters, reason),
                                    &[opentelemetry::KeyValue::new("reason", reason.as_str())],
                                );
                            }
                        }
                    })
                    .build();
            };
        by_reason(
            "socket_policy.denied",
            "Socket operations the socket policy refused, by reason",
            Self::denied,
        );
        by_reason(
            "socket_policy.would_deny",
            "Socket operations counted in count mode that enforcement would refuse, by reason",
            Self::would_deny,
        );
    }
}

const ALL_REASONS: [DenyReason; DENY_REASONS] = [
    DenyReason::NotPermitted,
    DenyReason::BindNotPermitted,
    DenyReason::HostLoopbackNotPermitted,
    DenyReason::HostOwnedPort,
    DenyReason::BlockedRange,
    DenyReason::NoCapacity,
];

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod policy_meter_tests {
    use super::*;

    /// The would-deny counters an operator is told to watch reach an exporter,
    /// through a real SDK pipeline with a reader of its own.
    #[test]
    fn policy_counters_are_published_as_metrics() {
        use opentelemetry::metrics::MeterProvider as _;
        use opentelemetry_sdk::metrics::{
            InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
            data::{AggregatedMetrics, MetricData},
        };

        let meters = Arc::new(PolicyMeters::default());
        meters.record_host_path_denied();
        meters.record_would_deny(DenyReason::BlockedRange);

        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        meters.register_metrics(&provider.meter("test"));
        provider.force_flush().expect("flush");

        let mut seen = std::collections::BTreeMap::new();
        for resource in exporter.get_finished_metrics().expect("finished metrics") {
            for scope in resource.scope_metrics() {
                for metric in scope.metrics() {
                    let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
                        continue;
                    };
                    for point in sum.data_points() {
                        let reason = point
                            .attributes()
                            .find(|kv| kv.key.as_str() == "reason")
                            .map(|kv| kv.value.to_string());
                        seen.insert((metric.name().to_string(), reason), point.value());
                    }
                }
            }
        }
        assert_eq!(seen[&("host_path.denied".to_string(), None)], 1);
        assert_eq!(
            seen[&(
                "socket_policy.would_deny".to_string(),
                Some("blocked_range".to_string())
            )],
            1
        );
        assert_eq!(
            seen[&(
                "socket_policy.denied".to_string(),
                Some("not_permitted".to_string())
            )],
            0
        );
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
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
#[cfg_attr(coverage_nightly, coverage(off))]
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

    /// Guests and ingress take their shares of one budget, and both fit inside
    /// it with room left for the listeners, pulls and open files that are
    /// neither. Checked against limits this process need not be running under,
    /// because the interesting ones are not the ones a test machine has.
    #[test]
    fn ingress_and_guests_fit_inside_one_descriptor_budget() {
        for soft in [4096usize, 65_536, 1_048_576] {
            let guests = super::share_of(
                Some(soft),
                super::FD_BUDGET_NUMERATOR,
                super::FD_BUDGET_DENOMINATOR,
            );
            let ingress = super::share_of(
                Some(soft),
                super::INGRESS_BUDGET_NUMERATOR,
                super::INGRESS_BUDGET_DENOMINATOR,
            )
            .max(super::MIN_INGRESS_CONNECTIONS);
            assert!(
                guests + ingress < soft,
                "{guests} guest and {ingress} ingress connections must fit in {soft}"
            );
            assert!(ingress <= guests, "ingress takes the smaller share");
        }
    }

    /// Under a limit too small to divide, the floors win and together promise
    /// more than the budget holds. That is the deliberate trade — a host this
    /// constrained is unusable at a proportional ceiling — and it is recorded
    /// here so it stays a choice rather than becoming a surprise.
    #[test]
    fn a_tiny_descriptor_limit_gets_the_floors_not_a_share() {
        let soft = 128;
        assert_eq!(
            super::share_of(
                Some(soft),
                super::FD_BUDGET_NUMERATOR,
                super::FD_BUDGET_DENOMINATOR
            ),
            super::MIN_DERIVED_MAX_CONNECTIONS
        );
        assert!(super::MIN_DERIVED_MAX_CONNECTIONS + super::MIN_INGRESS_CONNECTIONS > soft);
    }

    /// What the raise asks for, without asking the kernel for anything.
    ///
    /// The step-down exists because an unlimited hard limit is not a number any
    /// kernel takes, and none reports what it would take — so a single ask that
    /// is refused must not be the end of it, or the raise silently does nothing
    /// on exactly the hosts it was written for.
    #[test]
    fn the_raise_asks_lower_until_it_reaches_what_it_already_has() {
        // Nothing to ask for: already at the ceiling, or above what is useful.
        assert!(super::raise_plan(Some(1_048_576), Some(1_048_576)).is_empty());
        assert!(super::raise_plan(None, None).is_empty());

        // The ordinary container: soft 1024, hard far above it.
        let plan = super::raise_plan(Some(1024), Some(1_048_576));
        assert_eq!(plan.first(), Some(&1_048_576), "ask for the most first");
        assert!(
            plan.last().is_some_and(|last| *last > 1024),
            "never ask for less than the process already has"
        );
        assert!(
            plan.windows(2).all(|w| w[0] > w[1]),
            "each ask is smaller than the last"
        );

        // An unlimited hard limit is capped, not taken literally.
        assert_eq!(
            super::raise_plan(Some(256), None).first(),
            Some(&super::MAX_RAISED_DESCRIPTORS)
        );
    }

    /// An unlimited `RLIMIT_NOFILE` is the largest budget there is, so it must
    /// not read as the smallest. Reported as no answer it would hand out the
    /// fallback share — a *sixty-fourth* of what the same host gets when its
    /// limit is a large number rather than "no limit".
    #[test]
    fn an_unlimited_descriptor_limit_is_a_generous_budget() {
        let unlimited = super::share_of(super::soft_limit_as_budget(None), 1, 2);
        assert!(
            unlimited > super::ASSUMED_MAX_CONNECTIONS,
            "an unlimited limit gave {unlimited}, no better than not reading it at all"
        );
        assert_eq!(
            unlimited,
            super::share_of(super::soft_limit_as_budget(Some(1_048_576)), 1, 2),
            "unlimited and the largest finite limit are the same budget"
        );
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
    fn guest_quota_is_removed_when_its_last_slot_closes() {
        let registry = QuotaRegistry::new(QuotaLimits::default(), None);
        let slot = registry
            .for_guest("guest")
            .try_acquire_outbound_socket()
            .expect("connection slot");
        assert_eq!(registry.quotas.len(), 1);
        drop(slot);
        assert!(registry.quotas.len() == 0);
        assert_eq!(
            registry
                .for_guest("guest")
                .counts()
                .outbound_sockets_granted,
            0
        );
    }

    #[tokio::test]
    async fn http_connection_slot_keeps_the_same_guest_quota_registered() {
        let registry = QuotaRegistry::new(
            QuotaLimits {
                outbound_http: 1,
                ..QuotaLimits::default()
            },
            None,
        );
        let slot = registry
            .for_guest("guest")
            .acquire_outbound_http(Duration::from_secs(1))
            .await
            .expect("connection slot");
        assert!(
            registry
                .for_guest("guest")
                .try_acquire_outbound_http()
                .is_none()
        );
        drop(slot);
        assert!(registry.quotas.len() == 0);
        assert!(
            registry
                .for_guest("guest")
                .try_acquire_outbound_http()
                .is_some()
        );
    }

    #[tokio::test]
    async fn pooled_http_acquire_counts_grants_and_timeouts() {
        let quota = GuestConnectionQuota::new(
            QuotaLimits {
                outbound_http: 1,
                ..QuotaLimits::default()
            },
            None,
        );
        let held = quota
            .acquire_outbound_http(Duration::from_millis(10))
            .await
            .expect("first slot");
        assert!(
            quota
                .acquire_outbound_http(Duration::from_millis(10))
                .await
                .is_none(),
            "a full allowance times out"
        );
        let counts = quota.counts();
        assert_eq!(counts.outbound_http_granted, 1);
        assert_eq!(counts.outbound_http_refused, 1);
        drop(held);
        assert_eq!(quota.outbound_http_available(), 1);
    }

    #[tokio::test]
    async fn pooled_http_wait_returns_the_guest_permit_when_host_wide_times_out() {
        let registry = QuotaRegistry::new(QuotaLimits::default(), Some(1));
        let other = registry.for_guest("other");
        let _host = other.try_acquire_outbound_socket().expect("host slot");
        let quota = registry.for_guest("guest");
        let before = quota.outbound_http_available();
        assert!(
            quota
                .acquire_outbound_http(Duration::from_millis(10))
                .await
                .is_none()
        );
        assert_eq!(quota.outbound_http_available(), before);
    }

    #[test]
    fn registry_shards_shrink_after_a_burst_of_guests() {
        let registry = QuotaRegistry::new(QuotaLimits::default(), None);
        let slots: Vec<_> = (0..4096)
            .map(|i| {
                registry
                    .for_guest(&format!("guest-{i}"))
                    .try_acquire_outbound_socket()
                    .expect("slot")
            })
            .collect();
        assert_eq!(registry.quotas.len(), 4096);
        drop(slots);
        assert_eq!(registry.quotas.len(), 0);
        let capacity: usize = registry
            .quotas
            .shards
            .iter()
            .map(|shard| shard.lock().unwrap().capacity())
            .sum();
        assert!(
            capacity <= REGISTRY_SHARDS * SHARD_SHRINK_FLOOR * 2,
            "shards kept {capacity} slots after emptying"
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
