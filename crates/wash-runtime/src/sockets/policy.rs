//! Which addresses a guest may bind and connect, and on which plane.
//!
//! One [`SocketPolicy`] per guest, installed as the `socket_addr_check` closure
//! on its [`WasiSocketsCtx`](super::WasiSocketsCtx). Every bind, connect, and
//! outgoing datagram goes through [`SocketPolicy::decide`], so the answer to
//! "what can this reach" is readable — and meterable — in one place.
//!
//! The order of evaluation for an outbound address, and why:
//!
//! 1. **The host sentinel**, first, because it is the one address whose meaning
//!    is not its value. It resolves to real loopback on [`Plane::Host`], gated
//!    by the guest's own `allowedHostLoopbackPorts` and the host's flag.
//! 2. **Virtual loopback**, which reaches only the guest's own in-process
//!    network and so needs no egress policy at all.
//! 3. **Everything else** is real egress: layer 1 the declared `allowedHosts`,
//!    layer 2 the address-range policy. Layer 1 is what closes the hole — a
//!    range policy alone still permits dialing the Kubernetes API on an
//!    ordinary routable address.
//!
//! Binds never reach the egress layers: a bind is not a destination.

use core::net::SocketAddr;
use std::sync::Arc;

use super::{AddrDecision, Allowed, DenyReason, Plane, SocketAddrUse, internal_names};
use crate::host::allowed_hosts::{AllowedHost, check_allowed_addr};
use crate::host::allowed_loopback::{AllowedLoopbackPort, check_allowed_loopback};
use crate::host::declared_port::Protocol;
use crate::host::egress_policy::EgressAddressPolicy;
use crate::host::ports::PortTable;

/// A real address a host component plugin is permitted to bind directly,
/// declared by the operator that installed the plugin.
///
/// The address is always concrete: an unspecified address is rejected when the
/// declaration is parsed, so this never grants "every interface". Built from a
/// plugin's declared `ports` entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectBind {
    pub addr: SocketAddr,
    pub udp: bool,
}

impl DirectBind {
    pub fn permits(&self, reason: SocketAddrUse, addr: SocketAddr) -> bool {
        let matches_protocol = match reason {
            SocketAddrUse::TcpBind => !self.udp,
            SocketAddrUse::UdpBind => self.udp,
            _ => return false,
        };
        matches_protocol && self.addr == addr
    }
}

/// What kind of guest a policy belongs to, which is what decides its bind rules.
#[derive(Debug, Clone, Default)]
pub enum GuestKind {
    /// A workload component. Never listens.
    #[default]
    Component,
    /// A workload's long-lived service. Listens, inside the workload's virtual
    /// network only.
    Service,
    /// A host component plugin. Listens inside its own private virtual network,
    /// plus any concrete address the operator declared for it.
    Plugin { direct_binds: Arc<[DirectBind]> },
}

/// How strictly the egress gate is applied.
///
/// Turning the gate on is a breaking change for any guest doing socket egress
/// without a declared `allowedHosts` — which, since the socket path was never
/// gated, is all of them. [`EgressMode::Count`] exists so an operator can see
/// what enforcement *would* break before it breaks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EgressMode {
    /// Evaluate the policy, log and count what it would refuse, allow it
    /// anyway. The default, so upgrading a host does not sever live traffic.
    #[default]
    Count,
    /// Refuse what the policy refuses.
    Enforce,
}

/// The complete socket policy for one guest.
///
/// Built by the host and reinstalled on every incarnation; there is no
/// guest-facing setter, so a guest cannot widen its own reach.
#[derive(Clone, Debug)]
pub struct SocketPolicy {
    pub kind: GuestKind,
    /// Declared egress allowlist, shared with `wasi:http`. Empty denies every
    /// connect once [`EgressMode::Enforce`] is on.
    pub allowed_hosts: Arc<[AllowedHost]>,
    /// Ports on the machine's own loopback this guest may reach through
    /// `host.wasmcloud.internal`. Empty denies every one.
    pub host_loopback: Arc<[AllowedLoopbackPort]>,
    /// Whether the host permits host-loopback access at all. A workload-level
    /// grant is inert without it.
    pub host_loopback_enabled: bool,
    /// Range policy applied to every address layer 1 permitted.
    pub egress_addrs: EgressAddressPolicy,
    /// Ports this host owns. A guest reaching one through the sentinel would be
    /// reaching the host's own ingress or control plane, or another tenant's
    /// published service.
    pub host_owned_ports: Option<Arc<PortTable>>,
    pub egress_mode: EgressMode,
    /// This guest's connection allowance. Bounds its raw sockets here; the
    /// same quota bounds its pooled HTTP and its inbound published ports.
    ///
    /// Minted per guest from the host's registry — see
    /// [`SocketPolicy::for_guest`]. A policy built without one, as the
    /// host-level template is, has no quota until a guest claims it.
    pub quota: Option<crate::host::quota::GuestConnectionQuota>,
    /// Where [`SocketPolicy::for_guest`] mints a quota from.
    pub quotas: Option<Arc<crate::host::quota::QuotaRegistry>>,
    /// Counters for what the policy decided.
    pub meters: Option<Arc<crate::host::quota::PolicyMeters>>,
}

impl Default for SocketPolicy {
    /// The same policy the `wash` CLI builds when an operator passes no socket
    /// flags: range filtering on, egress gate counting rather than enforcing,
    /// host loopback closed. An embedder that installs no policy of its own
    /// gets what an operator running the host would get, rather than a
    /// permissive one nobody chose.
    fn default() -> Self {
        Self {
            kind: GuestKind::Component,
            allowed_hosts: Arc::from([]),
            host_loopback: Arc::from([]),
            host_loopback_enabled: false,
            egress_addrs: EgressAddressPolicy::default(),
            host_owned_ports: None,
            egress_mode: EgressMode::Count,
            quota: None,
            quotas: None,
            meters: None,
        }
    }
}

impl SocketPolicy {
    pub fn for_kind(kind: GuestKind) -> Self {
        Self {
            kind,
            ..Default::default()
        }
    }

    /// This host-level policy, specialised for one guest.
    ///
    /// Claims the guest's own allowance from the registry, so a per-guest
    /// ceiling really is per guest. Without this every guest on the host would
    /// share whatever single quota the template happened to carry, making the
    /// number a host-wide cap wearing a per-guest name.
    #[must_use]
    pub fn for_guest(&self, kind: GuestKind, guest_id: &str) -> Self {
        Self {
            kind,
            quota: self.quota_for(guest_id),
            ..self.clone()
        }
    }

    /// One guest's allowance, without building a whole policy around it.
    ///
    /// The registry hands back the *same* allowance for the same id, so a port
    /// published on a guest's behalf draws on the ceiling that guest's own
    /// sockets and HTTP draw on. `None` when the host configured no registry.
    #[must_use]
    pub fn quota_for(&self, guest_id: &str) -> Option<crate::host::quota::GuestConnectionQuota> {
        self.quotas.as_ref().map(|r| r.for_guest(guest_id))
    }

    /// The single decision point. See the module docs for the evaluation order.
    pub fn decide(&self, reason: SocketAddrUse, addr: SocketAddr) -> AddrDecision {
        let decision = match reason {
            SocketAddrUse::TcpBind | SocketAddrUse::UdpBind => self.decide_bind(reason, addr),
            // Giving an outgoing datagram a local endpoint is egress, not
            // listening, and its destination is checked separately as
            // `UdpOutgoingDatagram`. Denying it would deny UDP egress outright
            // — but it does open a real socket, so it spends a quota slot.
            SocketAddrUse::UdpImplicitBind => self.allow_with_slot(addr, Plane::Host),
            SocketAddrUse::TcpConnect => self.decide_connect(addr, Protocol::Tcp),
            SocketAddrUse::UdpConnect => self.decide_connect(addr, Protocol::Udp),
            // A datagram is not a connection: the socket sending it is one
            // descriptor however many peers it addresses, and that socket
            // already took a slot when it bound or connected. Charging per
            // datagram would count traffic while bounding nothing, so this runs
            // the same policy but spends nothing.
            SocketAddrUse::UdpOutgoingDatagram => match self.resolve_connect(addr, Protocol::Udp) {
                Ok((addr, plane)) => AddrDecision::allow_on(addr, plane),
                Err(reason) => AddrDecision::Deny(reason),
            },
        };
        if let (Some(meters), AddrDecision::Deny(why)) = (&self.meters, &decision) {
            meters.record_deny(*why);
        }
        decision
    }

    fn decide_bind(&self, reason: SocketAddrUse, addr: SocketAddr) -> AddrDecision {
        let ip = addr.ip();
        let permitted = match &self.kind {
            // A component never *listens* — but a UDP bind is not listening.
            // Binding a local endpoint is the required first step of UDP
            // egress: p2 refuses `stream()` on an unbound socket, and Rust's
            // std does an explicit bind on p3 too, so denying this would deny
            // outbound datagrams rather than deny a listener.
            GuestKind::Component => match reason {
                SocketAddrUse::UdpBind => ip.is_loopback() || ip.is_unspecified(),
                _ => false,
            },
            // A service listens inside the workload's virtual network. An
            // unspecified UDP bind is permitted because `UdpSocket::bind`
            // rewrites it to loopback before it reaches the OS, exactly as the
            // TCP path does.
            GuestKind::Service => match reason {
                SocketAddrUse::UdpBind => ip.is_loopback() || ip.is_unspecified(),
                _ => ip.is_loopback(),
            },
            // A plugin's virtual network is private, so a listener in it
            // reaches nothing until the host publishes a real port that splices
            // into it. Beyond that it may bind only an address the operator
            // declared, matched exactly.
            GuestKind::Plugin { direct_binds } => {
                ip.is_loopback() || direct_binds.iter().any(|b| b.permits(reason, addr))
            }
        };
        if permitted {
            AddrDecision::allow_by_address(addr)
        } else {
            AddrDecision::Deny(DenyReason::BindNotPermitted)
        }
    }

    /// Resolve an outbound address to the address and plane to actually use,
    /// or the reason it is refused.
    ///
    /// **The only place connect policy lives.** The metered path
    /// ([`Self::decide_connect`]) and the unmetered datagram path both resolve
    /// through here, so every rule applies to both and neither can drift.
    fn resolve_connect(
        &self,
        addr: SocketAddr,
        protocol: Protocol,
    ) -> Result<(SocketAddr, Plane), DenyReason> {
        // 1. The sentinel: the machine's own loopback, by name.
        if internal_names::is_host_sentinel(addr.ip()) {
            return self.resolve_host_loopback(addr, protocol);
        }

        // 2. The guest's own virtual network. Reaches nothing outside this
        //    process, so no egress policy applies.
        if addr.ip().to_canonical().is_loopback() {
            return Ok((addr, Plane::Virtual));
        }

        // 3. Real egress: the declared allowlist, then the address ranges.
        if !check_allowed_addr(&self.allowed_hosts, addr) {
            return self.gate(DenyReason::NotPermitted, addr).map(|p| (addr, p));
        }
        if !self.egress_addrs.permits(addr.ip()) {
            return self.gate(DenyReason::BlockedRange, addr).map(|p| (addr, p));
        }
        Ok((addr, Plane::Host))
    }

    /// [`Self::resolve_connect`], spending a quota slot on what it permits.
    fn decide_connect(&self, addr: SocketAddr, protocol: Protocol) -> AddrDecision {
        match self.resolve_connect(addr, protocol) {
            Ok((addr, plane)) => self.allow_with_slot(addr, plane),
            Err(reason) => AddrDecision::Deny(reason),
        }
    }

    fn resolve_host_loopback(
        &self,
        addr: SocketAddr,
        protocol: Protocol,
    ) -> Result<(SocketAddr, Plane), DenyReason> {
        if !self.host_loopback_enabled {
            return Err(DenyReason::HostLoopbackNotPermitted);
        }
        if !check_allowed_loopback(&self.host_loopback, addr, protocol) {
            return Err(DenyReason::HostLoopbackNotPermitted);
        }
        let target = internal_names::rewrite_sentinel(addr);
        // A guest reaching a port the host owns would be reaching the host's own
        // ingress or control plane, or another tenant's published service. This
        // is checked after the allowlist so an operator cannot grant it by
        // listing the port.
        if let Some(table) = &self.host_owned_ports
            && table.is_published(protocol, target)
        {
            return Err(DenyReason::HostOwnedPort);
        }
        // Already past an explicit, per-port grant — a stronger check than the
        // range policy, which would deny loopback categorically.
        Ok((target, Plane::Host))
    }

    /// Apply a refusal under the current [`EgressMode`]: deny it, or count it
    /// and let it through so an operator can see the blast radius first.
    fn gate(&self, reason: DenyReason, addr: SocketAddr) -> Result<Plane, DenyReason> {
        match self.egress_mode {
            EgressMode::Enforce => Err(reason),
            EgressMode::Count => {
                if let Some(meters) = &self.meters {
                    meters.record_would_deny(reason);
                }
                tracing::debug!(
                    %addr,
                    reason = reason.as_str(),
                    "socket egress policy would refuse this connection; allowing it because the \
                     host is in count mode"
                );
                Ok(Plane::Host)
            }
        }
    }

    fn allow_with_slot(&self, addr: SocketAddr, plane: Plane) -> AddrDecision {
        // Only real connections spend the quota: a virtual one costs no file
        // descriptor and no remote resource.
        if plane == Plane::Virtual {
            return AddrDecision::allow_on(addr, plane);
        }
        let Some(quota) = &self.quota else {
            return AddrDecision::allow_on(addr, plane);
        };
        // `try_acquire`, never `.await`: a guest holds sockets across yield
        // points, so waiting for a slot it must make progress to free would
        // deadlock it against itself.
        match quota.try_acquire_outbound_socket() {
            Some(permit) => AddrDecision::Allow(Allowed {
                addr,
                plane,
                permit: Some(permit),
            }),
            None => AddrDecision::Deny(DenyReason::NoCapacity),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().expect("test address should parse")
    }

    fn plane_of(decision: &AddrDecision) -> Option<Plane> {
        match decision {
            AddrDecision::Allow(a) => Some(a.plane),
            AddrDecision::Deny(_) => None,
        }
    }

    fn denied(decision: &AddrDecision) -> Option<DenyReason> {
        match decision {
            AddrDecision::Deny(r) => Some(*r),
            AddrDecision::Allow(_) => None,
        }
    }

    fn allowed_addr(decision: &AddrDecision) -> Option<SocketAddr> {
        match decision {
            AddrDecision::Allow(a) => Some(a.addr),
            AddrDecision::Deny(_) => None,
        }
    }

    fn enforcing(kind: GuestKind) -> SocketPolicy {
        SocketPolicy {
            egress_mode: EgressMode::Enforce,
            egress_addrs: EgressAddressPolicy::default(),
            ..SocketPolicy::for_kind(kind)
        }
    }

    #[test]
    fn a_component_cannot_listen_at_all() {
        let policy = SocketPolicy::for_kind(GuestKind::Component);
        for a in ["127.0.0.1:8080", "0.0.0.0:8080", "10.0.0.5:8080"] {
            assert_eq!(
                denied(&policy.decide(SocketAddrUse::TcpBind, addr(a))),
                Some(DenyReason::BindNotPermitted),
                "component should not listen on {a}"
            );
        }
    }

    /// A UDP bind is the required first step of *egress*, not a listener: p2
    /// refuses `stream()` on an unbound socket. Denying it would take away
    /// outbound datagrams, which components have always had.
    #[test]
    fn a_component_binds_udp_for_egress_but_not_a_real_interface() {
        let policy = SocketPolicy::for_kind(GuestKind::Component);
        for a in ["0.0.0.0:0", "127.0.0.1:0", "[::]:0"] {
            assert!(
                plane_of(&policy.decide(SocketAddrUse::UdpBind, addr(a))).is_some(),
                "component should bind {a} to send datagrams"
            );
        }
        // A concrete external address is still a listener on a real interface.
        assert_eq!(
            denied(&policy.decide(SocketAddrUse::UdpBind, addr("10.0.0.5:8080"))),
            Some(DenyReason::BindNotPermitted)
        );
    }

    #[test]
    fn a_service_listens_only_on_its_virtual_loopback() {
        let policy = SocketPolicy::for_kind(GuestKind::Service);
        assert!(plane_of(&policy.decide(SocketAddrUse::TcpBind, addr("127.0.0.1:8080"))).is_some());
        assert!(denied(&policy.decide(SocketAddrUse::TcpBind, addr("10.0.0.5:8080"))).is_some());
        // Unspecified UDP is permitted only because the bind path rewrites it
        // to loopback before the OS sees it.
        assert!(plane_of(&policy.decide(SocketAddrUse::UdpBind, addr("0.0.0.0:8080"))).is_some());
        assert!(denied(&policy.decide(SocketAddrUse::UdpBind, addr("10.0.0.5:8080"))).is_some());
    }

    #[test]
    fn an_implicit_bind_is_egress_not_listening() {
        let policy = SocketPolicy::for_kind(GuestKind::Component);
        assert!(
            plane_of(&policy.decide(SocketAddrUse::UdpImplicitBind, addr("0.0.0.0:0"))).is_some()
        );
    }

    #[test]
    fn a_plugin_binds_its_own_loopback_and_its_declared_address() {
        let declared: Arc<[DirectBind]> = Arc::from([DirectBind {
            addr: addr("10.0.0.5:9000"),
            udp: false,
        }]);
        let policy = SocketPolicy::for_kind(GuestKind::Plugin {
            direct_binds: declared,
        });

        for ok in ["127.0.0.1:50051", "10.0.0.5:9000"] {
            assert!(
                plane_of(&policy.decide(SocketAddrUse::TcpBind, addr(ok))).is_some(),
                "{ok} should bind"
            );
        }
        for denied_addr in ["10.0.0.6:9000", "10.0.0.5:9001", "0.0.0.0:9000"] {
            assert!(
                denied(&policy.decide(SocketAddrUse::TcpBind, addr(denied_addr))).is_some(),
                "{denied_addr} should not bind"
            );
        }
        // Same tuple, wrong protocol.
        assert!(denied(&policy.decide(SocketAddrUse::UdpBind, addr("10.0.0.5:9000"))).is_some());
    }

    #[test]
    fn the_virtual_loopback_needs_no_egress_policy() {
        let policy = enforcing(GuestKind::Component);
        // `allowed_hosts` is empty, yet the guest's own network is reachable.
        let decision = policy.decide(SocketAddrUse::TcpConnect, addr("127.0.0.1:6432"));
        assert_eq!(plane_of(&decision), Some(Plane::Virtual));
    }

    #[test]
    fn real_egress_needs_a_declared_allowlist_when_enforcing() {
        let policy = enforcing(GuestKind::Component);
        assert_eq!(
            denied(&policy.decide(SocketAddrUse::TcpConnect, addr("93.184.216.34:443"))),
            Some(DenyReason::NotPermitted)
        );

        let permitted = SocketPolicy {
            allowed_hosts: Arc::from(["93.184.216.34".parse::<AllowedHost>().unwrap()]),
            ..enforcing(GuestKind::Component)
        };
        assert_eq!(
            plane_of(&permitted.decide(SocketAddrUse::TcpConnect, addr("93.184.216.34:443"))),
            Some(Plane::Host)
        );
    }

    /// `*` grants the internet, not the machine the host runs on: the range
    /// policy still refuses what a name might have resolved into.
    #[test]
    fn any_does_not_grant_the_blocked_ranges() {
        let policy = SocketPolicy {
            allowed_hosts: Arc::from([AllowedHost::Any]),
            ..enforcing(GuestKind::Component)
        };
        assert_eq!(
            denied(&policy.decide(SocketAddrUse::TcpConnect, addr("169.254.169.254:80"))),
            Some(DenyReason::BlockedRange)
        );
        assert_eq!(
            plane_of(&policy.decide(SocketAddrUse::TcpConnect, addr("10.0.0.5:5432"))),
            Some(Plane::Host)
        );
    }

    #[test]
    fn the_sentinel_needs_both_the_host_flag_and_the_workload_grant() {
        let sentinel = addr("127.255.255.254:5432");

        // Neither.
        let none = enforcing(GuestKind::Component);
        assert_eq!(
            denied(&none.decide(SocketAddrUse::TcpConnect, sentinel)),
            Some(DenyReason::HostLoopbackNotPermitted)
        );

        // Host flag only.
        let host_only = SocketPolicy {
            host_loopback_enabled: true,
            ..enforcing(GuestKind::Component)
        };
        assert_eq!(
            denied(&host_only.decide(SocketAddrUse::TcpConnect, sentinel)),
            Some(DenyReason::HostLoopbackNotPermitted)
        );

        // Workload grant only.
        let workload_only = SocketPolicy {
            host_loopback: Arc::from([AllowedLoopbackPort::tcp(5432)]),
            ..enforcing(GuestKind::Component)
        };
        assert_eq!(
            denied(&workload_only.decide(SocketAddrUse::TcpConnect, sentinel)),
            Some(DenyReason::HostLoopbackNotPermitted)
        );

        // Both: allowed, rewritten to real loopback, forced onto the host plane.
        let both = SocketPolicy {
            host_loopback_enabled: true,
            host_loopback: Arc::from([AllowedLoopbackPort::tcp(5432)]),
            ..enforcing(GuestKind::Component)
        };
        let decision = both.decide(SocketAddrUse::TcpConnect, sentinel);
        assert_eq!(plane_of(&decision), Some(Plane::Host));
        assert_eq!(allowed_addr(&decision), Some(addr("127.0.0.1:5432")));
    }

    /// `allowedHosts: ["*"]` must not reach the host's loopback: that needs its
    /// own grant, which is the whole point of the sentinel.
    #[test]
    fn any_does_not_grant_the_sentinel() {
        let policy = SocketPolicy {
            allowed_hosts: Arc::from([AllowedHost::Any]),
            host_loopback_enabled: true,
            ..enforcing(GuestKind::Component)
        };
        assert_eq!(
            denied(&policy.decide(SocketAddrUse::TcpConnect, addr("127.255.255.254:5432"))),
            Some(DenyReason::HostLoopbackNotPermitted)
        );
    }

    #[test]
    fn the_sentinel_grant_is_per_protocol() {
        let policy = SocketPolicy {
            host_loopback_enabled: true,
            host_loopback: Arc::from([AllowedLoopbackPort::tcp(53)]),
            ..enforcing(GuestKind::Component)
        };
        let sentinel = addr("127.255.255.254:53");
        assert!(plane_of(&policy.decide(SocketAddrUse::TcpConnect, sentinel)).is_some());
        assert_eq!(
            denied(&policy.decide(SocketAddrUse::UdpConnect, sentinel)),
            Some(DenyReason::HostLoopbackNotPermitted)
        );
    }

    #[test]
    fn a_host_owned_port_is_refused_even_when_granted() {
        let table = PortTable::new();
        let _reservation = table
            .reserve(
                Protocol::Tcp,
                addr("127.0.0.1:8080"),
                crate::host::ports::PortOwner::Workload("other".into()),
            )
            .unwrap();

        let policy = SocketPolicy {
            host_loopback_enabled: true,
            host_loopback: Arc::from([AllowedLoopbackPort::tcp(8080)]),
            host_owned_ports: Some(table),
            ..enforcing(GuestKind::Component)
        };
        assert_eq!(
            denied(&policy.decide(SocketAddrUse::TcpConnect, addr("127.255.255.254:8080"))),
            Some(DenyReason::HostOwnedPort)
        );
    }

    /// Count mode is the upgrade path: it must decide exactly as enforce would,
    /// record it, and then let the traffic through.
    #[test]
    fn count_mode_allows_what_enforce_would_refuse_and_counts_it() {
        let meters = Arc::new(crate::host::quota::PolicyMeters::default());
        let policy = SocketPolicy {
            egress_mode: EgressMode::Count,
            egress_addrs: EgressAddressPolicy::default(),
            meters: Some(Arc::clone(&meters)),
            ..SocketPolicy::for_kind(GuestKind::Component)
        };

        assert_eq!(
            plane_of(&policy.decide(SocketAddrUse::TcpConnect, addr("93.184.216.34:443"))),
            Some(Plane::Host),
            "count mode must not sever live traffic"
        );
        assert_eq!(meters.would_deny(DenyReason::NotPermitted), 1);
        assert_eq!(meters.denied(DenyReason::NotPermitted), 0);
    }

    /// A bind refusal is not part of the egress rollout: it was always denied,
    /// so count mode must not weaken it.
    #[test]
    fn count_mode_does_not_soften_bind_refusals() {
        let policy = SocketPolicy {
            egress_mode: EgressMode::Count,
            ..SocketPolicy::for_kind(GuestKind::Component)
        };
        assert_eq!(
            denied(&policy.decide(SocketAddrUse::TcpBind, addr("127.0.0.1:8080"))),
            Some(DenyReason::BindNotPermitted)
        );
    }

    /// The quota bounds concurrent connections, so a decision must *hold* its
    /// permit — a policy that granted and immediately released one would count
    /// attempts and bound nothing.
    #[test]
    fn an_exhausted_quota_denies_with_its_own_reason() {
        let policy = SocketPolicy {
            quota: Some(crate::host::quota::GuestConnectionQuota::new(
                crate::host::quota::QuotaLimits {
                    outbound_http: 1,
                    outbound_sockets: 1,
                    inbound_sockets: 1,
                },
                None,
            )),
            allowed_hosts: Arc::from([AllowedHost::Any]),
            ..enforcing(GuestKind::Component)
        };
        let target = addr("10.0.0.5:5432");

        let first = policy.decide(SocketAddrUse::TcpConnect, target);
        assert_eq!(plane_of(&first), Some(Plane::Host));

        // The first decision is still holding its slot.
        assert_eq!(
            denied(&policy.decide(SocketAddrUse::TcpConnect, target)),
            Some(DenyReason::NoCapacity)
        );

        drop(first);
        assert_eq!(
            plane_of(&policy.decide(SocketAddrUse::TcpConnect, target)),
            Some(Plane::Host),
            "releasing a connection must return its slot"
        );
    }

    /// A datagram must spend nothing, *including* to the host sentinel: the
    /// sentinel resolves through the same host-loopback path a connect takes,
    /// and taking a slot there would grant one this caller never releases.
    #[test]
    fn a_datagram_never_spends_a_slot() {
        let quota = crate::host::quota::GuestConnectionQuota::new(
            crate::host::quota::QuotaLimits {
                outbound_http: 1,
                outbound_sockets: 1,
                inbound_sockets: 1,
            },
            None,
        );
        let policy = SocketPolicy {
            quota: Some(quota.clone()),
            allowed_hosts: Arc::from([AllowedHost::Any]),
            host_loopback_enabled: true,
            host_loopback: Arc::from([AllowedLoopbackPort::udp(53)]),
            ..enforcing(GuestKind::Component)
        };

        // Ordinary destination, then the sentinel: neither may consume the
        // guest's single socket slot.
        for target in ["10.0.0.5:53", "127.255.255.254:53"] {
            let decision = policy.decide(SocketAddrUse::UdpOutgoingDatagram, addr(target));
            assert!(
                plane_of(&decision).is_some(),
                "{target} should be permitted"
            );
            assert_eq!(
                quota.outbound_sockets_available(),
                1,
                "sending a datagram to {target} must not spend a slot"
            );
        }

        // The sentinel destination is still rewritten to real loopback.
        let decision = policy.decide(
            SocketAddrUse::UdpOutgoingDatagram,
            addr("127.255.255.254:53"),
        );
        assert_eq!(allowed_addr(&decision), Some(addr("127.0.0.1:53")));
    }

    /// The datagram path runs the *same* policy as connect, so a refusal there
    /// is a refusal here. This is what would break if the two ever split again.
    #[test]
    fn a_datagram_is_refused_wherever_a_connect_would_be() {
        let policy = SocketPolicy {
            allowed_hosts: Arc::from([AllowedHost::Any]),
            ..enforcing(GuestKind::Component)
        };
        for (reason, target) in [
            (DenyReason::BlockedRange, "169.254.169.254:53"),
            (DenyReason::HostLoopbackNotPermitted, "127.255.255.254:53"),
        ] {
            assert_eq!(
                denied(&policy.decide(SocketAddrUse::UdpOutgoingDatagram, addr(target))),
                Some(reason),
                "{target}"
            );
            assert_eq!(
                denied(&policy.decide(SocketAddrUse::UdpConnect, addr(target))),
                Some(reason),
                "{target} must be refused identically on connect"
            );
        }
    }

    /// Reaching the guest's own virtual network costs no file descriptor and no
    /// remote resource, so it must not draw on an allowance meant for real egress.
    #[test]
    fn the_virtual_plane_does_not_spend_the_quota() {
        let policy = SocketPolicy {
            quota: Some(crate::host::quota::GuestConnectionQuota::new(
                crate::host::quota::QuotaLimits {
                    outbound_http: 1,
                    outbound_sockets: 1,
                    inbound_sockets: 1,
                },
                None,
            )),
            ..enforcing(GuestKind::Component)
        };
        let virtual_target = addr("127.0.0.1:6432");
        let held: Vec<_> = (0..8)
            .map(|_| policy.decide(SocketAddrUse::TcpConnect, virtual_target))
            .collect();
        assert!(
            held.iter().all(|d| plane_of(d) == Some(Plane::Virtual)),
            "virtual connections must not be capped by the egress quota"
        );
    }

    /// Nor is the sentinel: it is new capability, so there is nothing to
    /// grandfather and count mode must keep it shut.
    #[test]
    fn count_mode_does_not_open_the_host_loopback_door() {
        let policy = SocketPolicy {
            egress_mode: EgressMode::Count,
            ..SocketPolicy::for_kind(GuestKind::Component)
        };
        assert_eq!(
            denied(&policy.decide(SocketAddrUse::TcpConnect, addr("127.255.255.254:5432"))),
            Some(DenyReason::HostLoopbackNotPermitted)
        );
    }

    /// An embedder that installs no policy of its own takes this default, so
    /// it has to be the policy `wash host start` builds from its own flag
    /// defaults. Otherwise the library is the weaker of the two and nobody
    /// chose that.
    #[test]
    fn the_default_matches_the_cli_flag_defaults() {
        let policy = SocketPolicy::default();
        // `--deny-special-ranges` defaults on, `--deny-private-ranges` off.
        assert_eq!(policy.egress_addrs, EgressAddressPolicy::default());
        assert!(policy.egress_addrs.deny_special);
        assert!(policy.egress_addrs.allow_private);
        // `--socket-egress count`, `--allow-host-loopback` off.
        assert_eq!(policy.egress_mode, EgressMode::Count);
        assert!(!policy.host_loopback_enabled);
    }
}
