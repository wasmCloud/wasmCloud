//! Types used throughout the wasmcloud crate for workload management and host operations.
//!
//! This module contains two main categories of types:
//!
//! ## Public API Types (used in [`crate::host::HostApi`])
//! - Request/Response types: [`WorkloadStartRequest`], [`WorkloadStartResponse`],
//!   [`WorkloadStatusRequest`], [`WorkloadStatusResponse`],
//!   [`WorkloadStopRequest`], [`WorkloadStopResponse`]
//! - Host information: [`HostHeartbeat`]
//!
//! ## Core Workload Types (used internally)
//! - Workload definition: [`Workload`], [`WorkloadState`], [`WorkloadStatus`]
//! - Component configuration: [`Component`], [`Service`], [`LocalResources`]
//! - Volume management: [`Volume`], [`VolumeType`], [`VolumeMount`],
//!   [`EmptyDirVolume`], [`HostPathVolume`]

use bytes::Bytes;
use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use crate::host::allowed_hosts::AllowedHost;
use crate::wit::WitInterface;

/// Represents a deployable workload containing one or more WebAssembly components.
/// A workload defines the complete runtime configuration including components,
/// services, interfaces, and volumes.
#[derive(Debug, Clone, PartialEq)]
pub struct Workload {
    pub namespace: String,
    pub name: String,
    pub annotations: HashMap<String, String>,
    pub service: Option<Service>,
    pub components: Vec<Component>,
    pub host_interfaces: Vec<WitInterface>,
    pub volumes: Vec<Volume>,
}

/// The current state of a workload in its lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(i32)]
pub enum WorkloadState {
    Unspecified = 0,
    Starting = 1,
    Running = 2,
    Completed = 3,
    Stopping = 4,
    Error = 5,
    NotFound = 6,
}

/// Configuration for a long-running service component that handles requests.
/// Services can be restarted if they fail and have resource limits.
#[derive(Debug, Clone, PartialEq)]
pub struct Service {
    pub bytes: Bytes,
    pub digest: Option<String>,
    pub local_resources: LocalResources,
    pub max_restarts: u64,
}

/// A WebAssembly component that can be executed as part of a workload.
/// Components can be pooled for concurrent execution and have invocation limits.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Component {
    pub name: String,
    pub bytes: Bytes,
    pub digest: Option<String>,
    pub local_resources: LocalResources,
    pub pool_size: i32,
    pub max_invocations: i32,
}

/// Policy mode for outbound TCP and UDP socket access from a sandboxed
/// component. Tunnel rules (the rewrite escape hatch) apply to TCP only; UDP
/// follows the mode but is never granted a rule-based escape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SocketTunnelMode {
    /// Block all connects except loopback traffic to in-process wash services
    /// and (TCP only) loopback ports matching a declared tunnel rule (rewritten
    /// to the rule's host address). The secure default.
    #[default]
    Strict,
    /// Allow every connect to pass through to the OS as-is; tunnel rules are
    /// ignored. An explicit opt-out for scenarios that don't need sandboxing.
    AllowAll,
    /// Block every connect except in-process service-to-service loopback
    /// traffic — not even tunnel rules escape.
    DenyAll,
}

/// Outbound socket policy: a `mode` (applied to TCP and UDP) plus TCP tunnel
/// `rules`. A rule rewrites traffic the component sends to
/// `127.0.0.1:sandbox_port` to instead dial `host_addr` on the real network.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SocketTunnelPolicy {
    pub mode: SocketTunnelMode,
    /// Map from sandbox-side loopback port → real host `SocketAddr` to dial.
    /// Only consulted when `mode == Strict`.
    pub rules: HashMap<u16, SocketAddr>,
}

impl SocketTunnelPolicy {
    /// Shared permission-gate core for outbound traffic. The single source of
    /// truth for "may this connect proceed at all"; the actual routing
    /// (loopback vs. tunnel vs. straight-to-OS) lives in [`crate::sockets`] and
    /// must stay consistent with the verdict here.
    ///
    /// - `policy` is the workload's declared policy, or `None` for the strict
    ///   default (block-by-default, no rules).
    /// - `has_loopback_listener` is whether an in-process wash peer is bound on
    ///   `addr` in the loopback registry (service-to-service traffic). Callers
    ///   compute it lazily because it requires locking the registry.
    /// - `honor_rules` controls whether a declared tunnel rule on the port grants
    ///   passage. TCP sets it (a rule rewrites the destination to a real host);
    ///   UDP does not (no rewrite exists), so a UDP rule never grants escape.
    fn allows_connect(
        policy: Option<&Self>,
        addr: &SocketAddr,
        has_loopback_listener: bool,
        honor_rules: bool,
    ) -> bool {
        let mode = policy.map(|p| p.mode).unwrap_or_default();
        match mode {
            // Opt-out: every connect passes straight through to the OS.
            SocketTunnelMode::AllowAll => true,
            // Strict and DenyAll both confine traffic to loopback. In Strict, a
            // declared tunnel rule additionally lets a port through (TCP only);
            // DenyAll blocks even those. Both always allow in-process
            // service-to-service traffic.
            SocketTunnelMode::Strict | SocketTunnelMode::DenyAll => {
                if !addr.ip().is_loopback() {
                    return false;
                }
                let has_rule = honor_rules
                    && mode == SocketTunnelMode::Strict
                    && policy.is_some_and(|p| p.rules.contains_key(&addr.port()));
                has_rule || has_loopback_listener
            }
        }
    }

    /// Permission-gate decision for an outbound **TCP** connect to `addr`.
    /// Honors tunnel rules: a Strict-mode rule rewrites the destination to the
    /// rule's host address and dials it on the real network.
    pub fn allows_tcp_connect(
        policy: Option<&Self>,
        addr: &SocketAddr,
        has_loopback_listener: bool,
    ) -> bool {
        Self::allows_connect(policy, addr, has_loopback_listener, true)
    }

    /// Permission-gate decision for outbound **UDP** — a connect or a single
    /// outgoing datagram to `addr`.
    ///
    /// Tunnel rules are NOT honored for UDP: there is no UDP destination
    /// rewrite, so a rule cannot grant a real-network escape. UDP is therefore
    /// confined to in-process service-to-service loopback traffic under Strict
    /// and DenyAll, and is unrestricted only under AllowAll.
    pub fn allows_udp_connect(
        policy: Option<&Self>,
        addr: &SocketAddr,
        has_loopback_listener: bool,
    ) -> bool {
        Self::allows_connect(policy, addr, has_loopback_listener, false)
    }
}

#[cfg(test)]
mod socket_tunnel_tests {
    use super::*;

    fn policy(mode: SocketTunnelMode, rule_ports: &[u16]) -> SocketTunnelPolicy {
        SocketTunnelPolicy {
            mode,
            rules: rule_ports
                .iter()
                .map(|&p| (p, SocketAddr::from(([127, 0, 0, 1], p))))
                .collect(),
        }
    }

    fn loopback(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    fn external(port: u16) -> SocketAddr {
        SocketAddr::from(([93, 184, 216, 34], port))
    }

    #[test]
    fn none_policy_defaults_to_strict() {
        // No policy == strict default: loopback service traffic allowed,
        // everything else blocked.
        assert!(SocketTunnelPolicy::allows_tcp_connect(
            None,
            &loopback(8080),
            true
        ));
        assert!(!SocketTunnelPolicy::allows_tcp_connect(
            None,
            &loopback(8080),
            false
        ));
        assert!(!SocketTunnelPolicy::allows_tcp_connect(
            None,
            &external(443),
            false
        ));
    }

    #[test]
    fn allow_all_permits_everything() {
        let p = policy(SocketTunnelMode::AllowAll, &[]);
        assert!(SocketTunnelPolicy::allows_tcp_connect(
            Some(&p),
            &external(443),
            false
        ));
        assert!(SocketTunnelPolicy::allows_tcp_connect(
            Some(&p),
            &loopback(8080),
            false
        ));
    }

    #[test]
    fn strict_allows_loopback_listener_and_rules_only() {
        let p = policy(SocketTunnelMode::Strict, &[3306]);
        // Declared rule on a loopback port: allowed even without a listener.
        assert!(SocketTunnelPolicy::allows_tcp_connect(
            Some(&p),
            &loopback(3306),
            false
        ));
        // In-process service listener: allowed.
        assert!(SocketTunnelPolicy::allows_tcp_connect(
            Some(&p),
            &loopback(9000),
            true
        ));
        // Loopback port with neither a rule nor a listener: blocked.
        assert!(!SocketTunnelPolicy::allows_tcp_connect(
            Some(&p),
            &loopback(9000),
            false
        ));
        // A rule never lifts the loopback restriction for an external address.
        assert!(!SocketTunnelPolicy::allows_tcp_connect(
            Some(&p),
            &external(3306),
            false
        ));
    }

    #[test]
    fn deny_all_blocks_rules_but_keeps_service_to_service() {
        // Regression guard for the gate fix: DenyAll must still permit
        // in-process service-to-service loopback traffic (matching its docs),
        // while blocking tunnel rules and all non-loopback traffic.
        let p = policy(SocketTunnelMode::DenyAll, &[3306]);
        // Service-to-service loopback listener: allowed.
        assert!(SocketTunnelPolicy::allows_tcp_connect(
            Some(&p),
            &loopback(9000),
            true
        ));
        // Declared rule is ignored under DenyAll.
        assert!(!SocketTunnelPolicy::allows_tcp_connect(
            Some(&p),
            &loopback(3306),
            false
        ));
        // External traffic blocked.
        assert!(!SocketTunnelPolicy::allows_tcp_connect(
            Some(&p),
            &external(443),
            true
        ));
    }

    #[test]
    fn udp_ignores_rules_and_confines_to_loopback_service_traffic() {
        // UDP has no tunnel rewrite, so rules never grant escape. AllowAll is
        // open; Strict and DenyAll allow only in-process service-to-service
        // loopback (and behave identically for UDP).
        let strict = policy(SocketTunnelMode::Strict, &[3306]);
        let allow = policy(SocketTunnelMode::AllowAll, &[]);
        let deny = policy(SocketTunnelMode::DenyAll, &[3306]);

        // AllowAll: open.
        assert!(SocketTunnelPolicy::allows_udp_connect(
            Some(&allow),
            &external(53),
            false
        ));

        // Strict: a rule does NOT let UDP escape (unlike TCP).
        assert!(!SocketTunnelPolicy::allows_udp_connect(
            Some(&strict),
            &loopback(3306),
            false
        ));
        // Strict: in-process loopback peer allowed; non-loopback always blocked.
        assert!(SocketTunnelPolicy::allows_udp_connect(
            Some(&strict),
            &loopback(9000),
            true
        ));
        assert!(!SocketTunnelPolicy::allows_udp_connect(
            Some(&strict),
            &external(53),
            false
        ));

        // DenyAll: only service-to-service, rule ignored.
        assert!(SocketTunnelPolicy::allows_udp_connect(
            Some(&deny),
            &loopback(9000),
            true
        ));
        assert!(!SocketTunnelPolicy::allows_udp_connect(
            Some(&deny),
            &loopback(3306),
            false
        ));

        // None == strict default.
        assert!(!SocketTunnelPolicy::allows_udp_connect(
            None,
            &loopback(53),
            false
        ));
        assert!(SocketTunnelPolicy::allows_udp_connect(
            None,
            &loopback(53),
            true
        ));
    }
}

/// Resource limits and configuration for a component or service.
/// Defines memory, CPU limits, configuration values, and volume mounts.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalResources {
    pub memory_limit_mb: i32,
    pub cpu_limit: i32,
    /// Opaque key-value configuration shared between operator + runtime + plugins.
    /// Allows passing arbitrary configuration values to influence implementation behavior for all component interfaces.
    /// Example: tracing=disable
    pub config: HashMap<String, String>,
    /// `wasi:cli/env` variables, copied to `WasiCtxBuilder` at component
    /// instantiation.
    pub environment: HashMap<String, String>,
    pub volume_mounts: Vec<VolumeMount>,
    /// Parsed outbound allowlist.
    /// **Empty = deny all outgoing requests**. See
    /// [`crate::host::http::check_allowed_hosts`]). To opt into
    /// unrestricted egress, pass an explicit `[AllowedHost::Any]`. The
    /// wash config layer substitutes `[Any]` when `allowedHosts` is
    /// omitted from YAML, so workloads coming through `wash dev` never
    /// land here with an unintentionally-empty list. Strings from the
    /// wire (proto / wash YAML) are parsed at conversion time, so the
    /// request hot path matches against the typed enum directly.
    pub allowed_hosts: Arc<[AllowedHost]>,
    /// Explicit policy for outbound TCP/UDP from this workload. `None` is
    /// treated as `Some(SocketTunnelPolicy::default())` (strict + no rules) so
    /// callers that don't care can omit the field entirely.
    pub socket_tunnels: Option<SocketTunnelPolicy>,
}

impl Default for LocalResources {
    fn default() -> Self {
        Self {
            memory_limit_mb: -1,
            cpu_limit: -1,
            config: HashMap::new(),
            environment: HashMap::new(),
            volume_mounts: Vec::new(),
            allowed_hosts: Default::default(),
            socket_tunnels: None,
        }
    }
}

/// A named volume that can be mounted into components.
#[derive(Debug, Clone, PartialEq)]
pub struct Volume {
    pub name: String,
    pub volume_type: VolumeType,
}

/// The type of volume - either host path or empty directory.
#[derive(Debug, Clone, PartialEq)]
pub enum VolumeType {
    HostPath(HostPathVolume),
    EmptyDir(EmptyDirVolume),
}

/// Describes how a volume should be mounted into a component.
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeMount {
    pub name: String,
    pub mount_path: String,
    pub read_only: bool,
}

/// An ephemeral empty directory volume that exists for the lifetime of the workload.
#[derive(Debug, Clone, PartialEq)]
pub struct EmptyDirVolume {}

/// A volume that mounts a directory from the host filesystem.
#[derive(Debug, Clone, PartialEq)]
pub struct HostPathVolume {
    pub local_path: String,
}

/// Information about the host's current state and capabilities.
/// Returned by [`crate::host::HostApi::heartbeat`].
#[derive(Debug, Clone, PartialEq)]
pub struct HostHeartbeat {
    pub id: String,
    pub hostname: String,
    pub http_port: u16,
    pub friendly_name: String,
    pub version: String,
    pub labels: HashMap<String, String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub os_arch: String,
    pub os_name: String,
    pub os_kernel: String,
    /// System CPU usage in percent (0.0 - 100.0)
    pub system_cpu_usage: f32,
    /// System total memory in bytes
    pub system_memory_total: u64,
    /// System free memory in bytes
    pub system_memory_free: u64,
    pub component_count: u64,
    pub workload_count: u64,
    pub imports: Vec<WitInterface>,
    pub exports: Vec<WitInterface>,
    /// Environment the host advertises itself as running in. For
    /// Kubernetes host pods this is the pod's namespace; for
    /// out-of-cluster hosts it is whatever the operator passed via
    /// `wash host --environment`. Empty when no environment was
    /// configured.
    pub environment: String,
}

/// Status information about a workload including its ID, state, and any messages.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadStatus {
    pub workload_id: String,
    pub workload_state: WorkloadState,
    pub message: String,
}

/// Request to start a new workload on the host.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadStartRequest {
    pub workload_id: String,
    pub workload: Workload,
}

/// Response after attempting to start a workload.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadStartResponse {
    pub workload_status: WorkloadStatus,
}

/// Request to get the status of a specific workload.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadStatusRequest {
    pub workload_id: String,
}

/// Response containing the status of a requested workload.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadStatusResponse {
    pub workload_status: WorkloadStatus,
}

/// Request to stop a running workload.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadStopRequest {
    pub workload_id: String,
}

/// Response after attempting to stop a workload.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadStopResponse {
    pub workload_status: WorkloadStatus,
}
