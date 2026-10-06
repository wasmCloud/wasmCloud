//! What one store has resolved, so an `allowedHosts` entry naming a host can
//! permit a raw socket to the address that name returned.
//!
//! A socket connects to an address and carries no name, so a name entry has
//! nothing to match on its own. This table is the link: each address the guest
//! was handed by `wasi:sockets/ip-name-lookup` is recorded against the name it
//! asked for, and [`ResolvedNames::permits`] asks whether a destination is one
//! of those addresses under a name the allowlist grants.
//!
//! # What a grant is
//!
//! An **observed address and a permitted port**, not a server identity. Once a
//! guest has resolved a granted name it may connect to the returned address
//! directly, and two names sharing an address cannot be told apart. The range
//! policy still applies afterwards, but private ranges are permitted by
//! default, so a granted name that resolves to another private service grants
//! that service.
//!
//! The name is the one the guest asked for. Reverse DNS, search-domain
//! expansions and CNAME targets are never consulted.
//!
//! # Grants expire
//!
//! Each observation permits its addresses for [`ResolvedNameLimits::lifetime`]
//! from the moment resolution completed, on a monotonic clock. That is a
//! runtime authorization lifetime, not a DNS TTL: the resolver APIs return no
//! TTL to honor. Resolving again refreshes only the addresses returned again,
//! so an address a Service no longer answers with expires on its own schedule.
//! A guest that caches an address longer than the lifetime has to resolve
//! again before a new connect; a connection already open is unaffected.
//!
//! A connected UDP socket is not a connection in that sense: each datagram is
//! new egress. It carries its grant's deadline ([`super::Allowed::valid_until`])
//! and asks the policy again only once that has passed, so a lapsed grant
//! stops it without every send taking this table's lock. Eviction is a memory
//! bound rather than a revocation, and such a socket keeps the deadline it
//! connected under.
//!
//! # One table per store
//!
//! Allocated with each store's [`super::WasiSocketsCtx`] and dropped with it.
//! Never placed on the reusable [`super::policy::SocketPolicy`]: two stores of
//! one workload must not share a history.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::host::allowed_hosts::AllowedHost;

/// Bounds on one store's [`ResolvedNames`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedNameLimits {
    /// How long an observation permits its addresses. Zero records nothing, so
    /// name entries permit no socket.
    pub lifetime: Duration,
    /// Distinct names remembered at once.
    pub max_names: usize,
    /// Name/address associations remembered at once, across every name.
    pub max_associations: usize,
}

impl Default for ResolvedNameLimits {
    fn default() -> Self {
        Self {
            lifetime: Duration::from_secs(60),
            max_names: 256,
            max_associations: 4096,
        }
    }
}

#[derive(Debug)]
struct NameEntry {
    addrs: BTreeSet<IpAddr>,
    /// The latest observation ever recorded under this name.
    newest: Instant,
}

/// Every association is indexed three ways, so the connect path, expiry and
/// both evictions each find what they want without walking the table.
#[derive(Debug, Default)]
struct Inner {
    /// Address to the names that returned it, with when. The connect path's
    /// question is "which names returned this address".
    by_addr: BTreeMap<IpAddr, BTreeMap<Arc<str>, Instant>>,
    names: BTreeMap<Arc<str>, NameEntry>,
    /// Associations oldest first. The lifetime is one value per table, so
    /// this is also the order they expire in.
    by_observed: BTreeSet<(Instant, IpAddr, Arc<str>)>,
    /// Names by their latest observation, stalest first.
    by_newest: BTreeSet<(Instant, Arc<str>)>,
}

/// The addresses one store's lookups returned, by the name that was asked for.
#[derive(Debug, Default)]
pub struct ResolvedNames {
    limits: ResolvedNameLimits,
    inner: Mutex<Inner>,
}

impl ResolvedNames {
    pub fn new(limits: ResolvedNameLimits) -> Self {
        Self {
            limits,
            inner: Mutex::default(),
        }
    }

    /// Record that a lookup of `name`, completed at `observed`, returned
    /// `addrs` to the guest.
    ///
    /// `name` is the normalized name the guest asked for — see
    /// [`normalize_name`]. An observation already past its lifetime records
    /// nothing, so reading an old answer late does not restart its clock.
    pub fn record(&self, name: &str, addrs: impl IntoIterator<Item = IpAddr>, observed: Instant) {
        self.record_at(name, addrs, observed, Instant::now());
    }

    /// When the grant for `addr` lapses: it is an address this store was
    /// handed for a name that `policy` grants on `addr`'s port. `None` if
    /// there is no current grant, else the latest deadline among the names
    /// that grant it.
    #[must_use]
    pub fn permitted_until(&self, policy: &[AllowedHost], addr: SocketAddr) -> Option<Instant> {
        self.permitted_until_at(policy, addr, Instant::now())
    }

    #[cfg(test)]
    pub(crate) fn permits(&self, policy: &[AllowedHost], addr: SocketAddr) -> bool {
        self.permitted_until(policy, addr).is_some()
    }

    fn record_at(
        &self,
        name: &str,
        addrs: impl IntoIterator<Item = IpAddr>,
        observed: Instant,
        now: Instant,
    ) {
        let lapsed = observed
            .checked_add(self.limits.lifetime)
            .is_none_or(|deadline| deadline <= now);
        if lapsed || self.limits.max_names == 0 || self.limits.max_associations == 0 {
            return;
        }
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        for addr in addrs {
            inner.insert(name, addr.to_canonical(), observed, now, self.limits);
        }
    }

    #[cfg(test)]
    fn permits_at(&self, policy: &[AllowedHost], addr: SocketAddr, now: Instant) -> bool {
        self.permitted_until_at(policy, addr, now).is_some()
    }

    fn permitted_until_at(
        &self,
        policy: &[AllowedHost],
        addr: SocketAddr,
        now: Instant,
    ) -> Option<Instant> {
        let lifetime = self.limits.lifetime;
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner
            .by_addr
            .get(&addr.ip().to_canonical())?
            .iter()
            .filter_map(|(name, observed)| {
                let deadline = observed.checked_add(lifetime)?;
                (deadline > now
                    && policy
                        .iter()
                        .any(|entry| entry.permits_name(name, addr.port())))
                .then_some(deadline)
            })
            .max()
    }

    #[cfg(test)]
    fn len(&self) -> (usize, usize) {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(inner.by_newest.len(), inner.names.len());
        assert_eq!(
            inner.by_addr.values().map(BTreeMap::len).sum::<usize>(),
            inner.by_observed.len()
        );
        (inner.names.len(), inner.by_observed.len())
    }
}

impl Inner {
    fn insert(
        &mut self,
        name: &str,
        addr: IpAddr,
        observed: Instant,
        now: Instant,
        limits: ResolvedNameLimits,
    ) {
        // An address returned again is refreshed in place. Only ever forward,
        // so two overlapping resolutions grant the later deadline whichever of
        // them is read first.
        if let Some((key, _)) = self.names.get_key_value(name)
            && let Some(recorded) = self.by_addr.get_mut(&addr).and_then(|g| g.get_mut(name))
        {
            let key = Arc::clone(key);
            if observed > *recorded {
                self.by_observed
                    .remove(&(*recorded, addr, Arc::clone(&key)));
                *recorded = observed;
                self.by_observed.insert((observed, addr, Arc::clone(&key)));
            }
            self.touch(&key, observed);
            return;
        }

        let is_new_name = !self.names.contains_key(name);
        if self.by_observed.len() >= limits.max_associations
            || (is_new_name && self.names.len() >= limits.max_names)
        {
            self.purge_expired(now, limits.lifetime);
        }
        // Eviction only ever removes a grant; a later connect it would have
        // permitted is refused.
        while self.by_observed.len() >= limits.max_associations {
            let Some((_, addr, name)) = self.by_observed.first().cloned() else {
                return;
            };
            self.remove(addr, &name);
        }
        while !self.names.contains_key(name) && self.names.len() >= limits.max_names {
            if !self.evict_stalest_name() {
                return;
            }
        }

        let key: Arc<str> = match self.names.get_key_value(name) {
            Some((key, _)) => Arc::clone(key),
            None => {
                let key: Arc<str> = Arc::from(name);
                self.names.insert(
                    Arc::clone(&key),
                    NameEntry {
                        addrs: BTreeSet::new(),
                        newest: observed,
                    },
                );
                self.by_newest.insert((observed, Arc::clone(&key)));
                key
            }
        };
        if let Some(entry) = self.names.get_mut(&key) {
            entry.addrs.insert(addr);
        }
        self.touch(&key, observed);
        self.by_addr
            .entry(addr)
            .or_default()
            .insert(Arc::clone(&key), observed);
        self.by_observed.insert((observed, addr, key));
    }

    /// Move `name` forward in the staleness order if `observed` is its latest.
    fn touch(&mut self, name: &Arc<str>, observed: Instant) {
        let Some(entry) = self.names.get_mut(name) else {
            return;
        };
        if observed > entry.newest {
            self.by_newest.remove(&(entry.newest, Arc::clone(name)));
            entry.newest = observed;
            self.by_newest.insert((observed, Arc::clone(name)));
        }
    }

    fn remove(&mut self, addr: IpAddr, name: &Arc<str>) {
        let Some(grants) = self.by_addr.get_mut(&addr) else {
            return;
        };
        let Some(observed) = grants.remove(name) else {
            return;
        };
        if grants.is_empty() {
            self.by_addr.remove(&addr);
        }
        self.by_observed.remove(&(observed, addr, Arc::clone(name)));
        if let Some(entry) = self.names.get_mut(name) {
            entry.addrs.remove(&addr);
            if entry.addrs.is_empty() {
                self.by_newest.remove(&(entry.newest, Arc::clone(name)));
                self.names.remove(name);
            }
        }
    }

    fn purge_expired(&mut self, now: Instant, lifetime: Duration) {
        while let Some((observed, addr, name)) = self.by_observed.first().cloned() {
            if observed
                .checked_add(lifetime)
                .is_some_and(|deadline| deadline > now)
            {
                break;
            }
            self.remove(addr, &name);
        }
    }

    /// Drop the name whose most recent observation is the oldest, with every
    /// address recorded under it.
    fn evict_stalest_name(&mut self) -> bool {
        let Some((_, name)) = self.by_newest.first().cloned() else {
            return false;
        };
        let addrs = self
            .names
            .get(&name)
            .map(|entry| entry.addrs.clone())
            .unwrap_or_default();
        for addr in addrs {
            self.remove(addr, &name);
        }
        true
    }
}

/// The key a looked-up domain is recorded under: lowercase, IDNA-encoded by
/// [`url::Host::parse`], without the terminal root dot. `None` for anything
/// that is not a domain — a literal address grants nothing by name.
#[must_use]
pub fn normalize_name(host: &url::Host) -> Option<String> {
    match host {
        url::Host::Domain(domain) => {
            let domain = domain.strip_suffix('.').unwrap_or(domain);
            (!domain.is_empty()).then(|| domain.to_ascii_lowercase())
        }
        url::Host::Ipv4(_) | url::Host::Ipv6(_) => None,
    }
}

/// The name a lookup asked a resolver for, and when its answer arrived.
#[derive(Debug)]
pub(crate) struct Observation {
    pub(crate) name: String,
    pub(crate) observed: Instant,
}

/// What `wasi:sockets/ip-name-lookup` answers for a host, on p2 and p3 alike.
#[derive(Debug)]
pub(crate) struct Lookup {
    pub(crate) addresses: Vec<IpAddr>,
    /// `None` for an answer no resolver produced — a literal, `*.localhost` —
    /// which grants nothing by name.
    pub(crate) observation: Option<Observation>,
}

impl Lookup {
    pub(crate) fn fixed(addresses: Vec<IpAddr>) -> Self {
        Self {
            addresses,
            observation: None,
        }
    }
}

/// Resolve `host`. Blocks on the system resolver, so call it off the executor.
///
/// Only names are resolved, not ports. The observation is dated here, when
/// resolution completes, so an answer the guest reads late is no younger for
/// it.
pub(crate) fn lookup_blocking(host: &url::Host) -> std::io::Result<Lookup> {
    use std::net::{Ipv4Addr, Ipv6Addr, ToSocketAddrs};

    match host {
        url::Host::Ipv4(addr) => Ok(Lookup::fixed(vec![(*addr).into()])),
        url::Host::Ipv6(addr) => Ok(Lookup::fixed(vec![(*addr).into()])),
        url::Host::Domain(domain) => {
            if domain.ends_with(".localhost") && domain != "localhost" {
                return Ok(Lookup::fixed(vec![
                    Ipv4Addr::LOCALHOST.into(),
                    Ipv6Addr::LOCALHOST.into(),
                ]));
            }
            let addresses = (domain.as_str(), 0)
                .to_socket_addrs()?
                .map(|addr| addr.ip().to_canonical())
                .collect();
            Ok(Lookup {
                addresses,
                observation: normalize_name(host).map(|name| Observation {
                    name,
                    observed: Instant::now(),
                }),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(entries: &[&str]) -> Vec<AllowedHost> {
        entries.iter().map(|e| e.parse().unwrap()).collect()
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn sock(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    fn names(lifetime_secs: u64) -> ResolvedNames {
        ResolvedNames::new(ResolvedNameLimits {
            lifetime: Duration::from_secs(lifetime_secs),
            ..Default::default()
        })
    }

    #[test]
    fn a_recorded_address_is_permitted_under_its_name_and_port() {
        let t0 = Instant::now();
        let table = names(60);
        table.record_at("db.internal", [ip("10.0.0.5")], t0, t0);

        let allow = policy(&["db.internal:5432"]);
        assert!(table.permits_at(&allow, sock("10.0.0.5:5432"), t0));
        assert!(!table.permits_at(&allow, sock("10.0.0.5:6379"), t0));
        assert!(!table.permits_at(&allow, sock("10.0.0.6:5432"), t0));
        assert!(
            !table.permits_at(&policy(&["other.internal"]), sock("10.0.0.5:5432"), t0),
            "an address is granted only under a name the policy names"
        );
    }

    #[test]
    fn a_wildcard_entry_permits_a_resolved_subdomain_only() {
        let t0 = Instant::now();
        let table = names(60);
        table.record_at("db.svc.local", [ip("10.0.0.5")], t0, t0);
        table.record_at("svc.local", [ip("10.0.0.6")], t0, t0);
        table.record_at("evilsvc.local", [ip("10.0.0.7")], t0, t0);

        let allow = policy(&["*.svc.local"]);
        assert!(table.permits_at(&allow, sock("10.0.0.5:80"), t0));
        assert!(!table.permits_at(&allow, sock("10.0.0.6:80"), t0));
        assert!(!table.permits_at(&allow, sock("10.0.0.7:80"), t0));
    }

    #[test]
    fn a_grant_expires_and_resolving_again_refreshes_only_what_returned() {
        let t0 = Instant::now();
        let table = names(60);
        let allow = policy(&["headless.svc"]);

        // A headless Service answering with two pods, then with one of them
        // replaced.
        table.record_at("headless.svc", [ip("10.0.0.1"), ip("10.0.0.2")], t0, t0);
        let t30 = t0 + Duration::from_secs(30);
        table.record_at("headless.svc", [ip("10.0.0.2"), ip("10.0.0.3")], t30, t30);

        let t61 = t0 + Duration::from_secs(61);
        assert!(
            !table.permits_at(&allow, sock("10.0.0.1:80"), t61),
            "an address not returned again expires on its own schedule"
        );
        assert!(table.permits_at(&allow, sock("10.0.0.2:80"), t61));
        assert!(table.permits_at(&allow, sock("10.0.0.3:80"), t61));

        let t91 = t0 + Duration::from_secs(91);
        assert!(!table.permits_at(&allow, sock("10.0.0.2:80"), t91));
        assert!(!table.permits_at(&allow, sock("10.0.0.3:80"), t91));
    }

    #[test]
    fn an_observation_read_late_does_not_restart_its_lifetime() {
        let t0 = Instant::now();
        let table = names(60);
        let allow = policy(&["db.internal"]);

        // Resolved at t0, consumed at t50: still dated t0.
        let t50 = t0 + Duration::from_secs(50);
        table.record_at("db.internal", [ip("10.0.0.5")], t0, t50);
        assert!(table.permits_at(&allow, sock("10.0.0.5:1"), t50));
        assert!(!table.permits_at(&allow, sock("10.0.0.5:1"), t0 + Duration::from_secs(60)));

        // Consumed after it expired: nothing is granted at all.
        let t70 = t0 + Duration::from_secs(70);
        table.record_at("late.internal", [ip("10.0.0.9")], t0, t70);
        assert!(!table.permits_at(&policy(&["late.internal"]), sock("10.0.0.9:1"), t70));
        assert_eq!(table.len().1, 1);
    }

    #[test]
    fn overlapping_resolutions_keep_the_later_deadline_in_either_order() {
        let t0 = Instant::now();
        let t10 = t0 + Duration::from_secs(10);
        let allow = policy(&["db.internal"]);
        let t65 = t0 + Duration::from_secs(65);

        for order in [[t0, t10], [t10, t0]] {
            let table = names(60);
            for observed in order {
                table.record_at("db.internal", [ip("10.0.0.5")], observed, t10);
            }
            assert!(table.permits_at(&allow, sock("10.0.0.5:1"), t65));
        }
    }

    #[test]
    fn a_mapped_address_is_the_address_it_maps() {
        let t0 = Instant::now();
        let table = names(60);
        table.record_at("db.internal", [ip("::ffff:10.0.0.5")], t0, t0);
        let allow = policy(&["db.internal"]);
        assert!(table.permits_at(&allow, sock("10.0.0.5:5432"), t0));
        assert!(table.permits_at(&allow, sock("[::ffff:10.0.0.5]:5432"), t0));
        assert_eq!(table.len(), (1, 1));
    }

    #[test]
    fn the_association_bound_evicts_the_oldest_and_never_permits_it_again() {
        let t0 = Instant::now();
        let table = ResolvedNames::new(ResolvedNameLimits {
            lifetime: Duration::from_secs(60),
            max_names: 8,
            max_associations: 2,
        });
        let allow = policy(&["db.internal"]);
        for (i, addr) in ["10.0.0.1", "10.0.0.2", "10.0.0.3"].into_iter().enumerate() {
            let at = t0 + Duration::from_secs(i as u64);
            table.record_at("db.internal", [ip(addr)], at, at);
        }
        let now = t0 + Duration::from_secs(3);
        assert_eq!(table.len(), (1, 2));
        assert!(!table.permits_at(&allow, sock("10.0.0.1:1"), now));
        assert!(table.permits_at(&allow, sock("10.0.0.2:1"), now));
        assert!(table.permits_at(&allow, sock("10.0.0.3:1"), now));
    }

    #[test]
    fn the_name_bound_evicts_the_stalest_name_whole() {
        let t0 = Instant::now();
        let table = ResolvedNames::new(ResolvedNameLimits {
            lifetime: Duration::from_secs(60),
            max_names: 2,
            max_associations: 64,
        });
        let t1 = t0 + Duration::from_secs(1);
        let t2 = t0 + Duration::from_secs(2);
        table.record_at("a.internal", [ip("10.0.0.1"), ip("10.0.0.2")], t0, t0);
        table.record_at("b.internal", [ip("10.0.0.3")], t1, t1);
        table.record_at("c.internal", [ip("10.0.0.4")], t2, t2);

        assert_eq!(table.len(), (2, 2));
        let allow = policy(&["*.internal"]);
        assert!(!table.permits_at(&allow, sock("10.0.0.1:1"), t2));
        assert!(!table.permits_at(&allow, sock("10.0.0.2:1"), t2));
        assert!(table.permits_at(&allow, sock("10.0.0.3:1"), t2));
        assert!(table.permits_at(&allow, sock("10.0.0.4:1"), t2));
    }

    #[test]
    fn expired_entries_are_removed_before_anything_live_is_evicted() {
        let t0 = Instant::now();
        let table = ResolvedNames::new(ResolvedNameLimits {
            lifetime: Duration::from_secs(60),
            max_names: 8,
            max_associations: 2,
        });
        let t50 = t0 + Duration::from_secs(50);
        let t70 = t0 + Duration::from_secs(70);
        // The expired entry is the *newer-keyed* one's elder, but a live entry
        // observed later must survive it.
        table.record_at("old.internal", [ip("10.0.0.1")], t0, t0);
        table.record_at("live.internal", [ip("10.0.0.2")], t50, t50);
        table.record_at("new.internal", [ip("10.0.0.3")], t70, t70);

        let allow = policy(&["*.internal"]);
        assert_eq!(table.len(), (2, 2));
        assert!(table.permits_at(&allow, sock("10.0.0.2:1"), t70));
        assert!(table.permits_at(&allow, sock("10.0.0.3:1"), t70));
    }

    #[test]
    fn a_zero_lifetime_records_nothing() {
        let t0 = Instant::now();
        let table = names(0);
        table.record_at("db.internal", [ip("10.0.0.5")], t0, t0);
        assert_eq!(table.len(), (0, 0));
    }

    #[test]
    fn concurrent_recording_stays_within_bounds() {
        let table = Arc::new(ResolvedNames::new(ResolvedNameLimits {
            lifetime: Duration::from_secs(60),
            max_names: 4,
            max_associations: 16,
        }));
        let t0 = Instant::now();
        std::thread::scope(|scope| {
            for thread in 0..8u8 {
                let table = Arc::clone(&table);
                scope.spawn(move || {
                    for i in 0..64u8 {
                        let name = format!("n{}.internal", (thread + i) % 9);
                        let addr = IpAddr::from([10, thread, 0, i]);
                        table.record_at(&name, [addr], t0, t0);
                    }
                });
            }
        });
        let (names, associations) = table.len();
        assert!(
            names <= 4 && associations <= 16,
            "{names} names, {associations} associations"
        );
    }

    #[test]
    fn names_normalize_case_and_the_root_dot() {
        let parse = |s: &str| normalize_name(&url::Host::parse(s).unwrap());
        assert_eq!(parse("DB.Internal.").as_deref(), Some("db.internal"));
        assert_eq!(
            parse("bücher.example").as_deref(),
            Some("xn--bcher-kva.example")
        );
        assert_eq!(parse("10.0.0.5"), None);
        assert_eq!(parse("[::1]"), None);
    }

    /// An answer no resolver produced carries no name to grant by.
    #[test]
    fn a_lookup_answered_without_a_resolver_observes_nothing() {
        for (host, expected) in [("10.0.0.5", 1), ("[fd00::1]", 1), ("app.localhost", 2)] {
            let lookup = lookup_blocking(&url::Host::parse(host).unwrap()).unwrap();
            assert_eq!(lookup.addresses.len(), expected, "{host}");
            assert!(lookup.observation.is_none(), "{host}");
        }
    }

    /// Eviction and expiry work from ordered indexes; churn well past both
    /// bounds has to leave them agreeing with the table (`len` checks) and
    /// keep exactly the newest associations.
    #[test]
    fn churn_past_both_bounds_keeps_the_newest_and_the_indexes_consistent() {
        let t0 = Instant::now();
        let table = ResolvedNames::new(ResolvedNameLimits {
            lifetime: Duration::from_secs(60),
            max_names: 8,
            max_associations: 32,
        });
        for i in 0..2000u32 {
            let at = t0 + Duration::from_millis(u64::from(i));
            let name = format!("n{}.internal", i % 50);
            let addr = IpAddr::from(std::net::Ipv4Addr::from(0x0a00_0000 + (i % 300)));
            table.record_at(&name, [addr], at, at);
            let (names, associations) = table.len();
            assert!(names <= 8 && associations <= 32);
        }
        let now = t0 + Duration::from_millis(2000);
        let allow = policy(&["*.internal"]);
        // The last record is `n49` at 10.0.0.199; the first is long evicted.
        assert!(table.permits_at(&allow, SocketAddr::new(ip("10.0.0.199"), 1), now));
        assert!(!table.permits_at(&allow, SocketAddr::new(ip("10.0.0.0"), 1), now));
        assert_eq!(table.len(), (8, 8));
    }
}
