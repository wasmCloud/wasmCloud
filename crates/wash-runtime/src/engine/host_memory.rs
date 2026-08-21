//! The three numbers that decide how much memory this host may hand to guests.
//!
//! All three already exist inside wasmtime; none of them was reachable from the
//! outside. This module gives each one a name, a derivation, and a startup line,
//! and checks the three against each other before the engine is built.
//!
//! | Knob | What it bounds | Unset |
//! | --- | --- | --- |
//! | [`HostMemory::max_memory`] | Total guest memory this host may use | Derived from the cgroup limit that would OOM-kill this process |
//! | [`HostMemory::default_heap_memory`] | How large any single linear memory may grow | wasmtime's own default (4 GiB) |
//! | [`HostMemory::core_instances`] | Instance slots the pooling allocator keeps | wasmtime's own default (1000) |
//!
//! **Nothing here changes behaviour when the flags are unset.** Both pool knobs
//! fall through to exactly the values wasmtime has always used, and the budget
//! is used to *check* the other two rather than to gate anything. That is
//! deliberate: this is the vocabulary a memory-aware host needs, landed on its
//! own so the enforcement built on top of it can be reviewed separately.
//!
//! # Why the budget is worth having before anything enforces it
//!
//! `default_heap_memory × core_instances` is what the pooling allocator
//! reserves — every slot is sized for the largest memory it might hold, whether
//! or not anything grows into it. Today a host claims 4 GiB × 1000 = 3.9 TiB of
//! address space and nobody is told, which is close enough to the 4 TiB the
//! allocator probes for that nudging either knob crosses it. Naming the budget
//! lets the host say, at startup, whether the pool it is about to build is one
//! the machine could actually back.

use std::fmt;

/// One mebibyte.
pub const MIB: u64 = 1024 * 1024;

/// wasmtime's default for `PoolingAllocationConfig::max_memory_size`.
///
/// Restated here so the startup log can say what the host resolved even when
/// the operator set nothing — "unset" is not an answer to "how large may a
/// linear memory get".
pub const WASMTIME_DEFAULT_HEAP_MEMORY: u64 = 4 * 1024 * MIB;

/// wasmtime's default for the pooling allocator's instance counts, and what
/// [`crate::engine::EngineBuilder`] has always passed when `max_instances` is
/// unset.
pub const WASMTIME_DEFAULT_CORE_INSTANCES: u32 = 1000;

/// Share of the detected memory limit that guest work may use. The rest is for
/// everything a guest is not: wasmtime itself, NATS, OCI pulls, host buffers.
const GUEST_MEMORY_NUMERATOR: u64 = 3;
const GUEST_MEMORY_DENOMINATOR: u64 = 4;

/// Bounds on the derived budget. The floor keeps a tiny container usable; the
/// cap stops an enormous one producing a number so large it stops being a bound.
const MIN_DERIVED_MAX_MEMORY: u64 = 256 * MIB;
const MAX_DERIVED_MAX_MEMORY: u64 = 1024 * 1024 * MIB;

/// Parse a byte size, following Kubernetes' quantity suffixes exactly.
///
/// | Suffix | Meaning |
/// | --- | --- |
/// | none, `B` | bytes |
/// | `Ki`, `KiB`, `Mi`, `MiB`, `Gi`, `GiB`, `Ti`, `TiB` | binary (1024ⁿ) |
/// | `K`, `KB`, `M`, `MB`, `G`, `GB`, `T`, `TB` | decimal (1000ⁿ) |
///
/// Kubernetes semantics rather than "binary everywhere", because the Helm chart
/// feeds this the host group's own `resources.limits.memory` verbatim — a
/// Kubernetes quantity, where `2Gi` and `2G` are genuinely different numbers.
/// Reading `2G` as 2 GiB would have the host believe it has 7% more than the
/// kernel will actually give it, and over-reading the budget is the dangerous
/// direction: the correction arrives as an OOMKill.
///
/// The `i` forms are what Kubernetes manifests overwhelmingly use, and what a
/// wasm page (exactly 64 KiB) is denominated in.
pub fn parse_bytes(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("expected a byte size, e.g. 128MiB".to_string());
    }
    let digits_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if digits_end == 0 {
        return Err(format!("{s:?} does not start with a number"));
    }
    let (number, suffix) = s.split_at(digits_end);
    let value: u64 = number
        .parse()
        .map_err(|_| format!("{number:?} is not a valid number"))?;
    let multiplier = match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        // Binary — the `i` forms, as Kubernetes spells them.
        "ki" | "kib" => 1024,
        "mi" | "mib" => MIB,
        "gi" | "gib" => 1024 * MIB,
        "ti" | "tib" => 1024 * 1024 * MIB,
        // Decimal — SI, and what Kubernetes means by a bare `K`/`M`/`G`/`T`.
        "k" | "kb" => 1_000,
        "m" | "mb" => 1_000_000,
        "g" | "gb" => 1_000_000_000,
        "t" | "tb" => 1_000_000_000_000,
        other => {
            return Err(format!(
                "unknown size suffix {other:?}; expected a Kubernetes-style quantity \
                 (Ki/Mi/Gi/Ti for binary, K/M/G/T for decimal, or plain bytes)"
            ));
        }
    };
    value
        .checked_mul(multiplier)
        .ok_or_else(|| format!("{s:?} overflows a byte count"))
}

/// Render a byte count in binary units, for logs and errors.
///
/// Exact multiples print as whole units; anything else falls back to one
/// decimal in the largest unit that fits. The reservation these knobs multiply
/// out to is rarely a round number — 4 GiB across 1000 slots is 3.9 TiB, not
/// 4 — and rounding that to a whole unit in an error message would misstate
/// the very number the operator is being asked to act on.
pub fn render_bytes(bytes: u64) -> String {
    const GIB: u64 = 1024 * MIB;
    const TIB: u64 = 1024 * GIB;
    for (unit, suffix) in [(TIB, "TiB"), (GIB, "GiB"), (MIB, "MiB"), (1024, "KiB")] {
        if bytes >= unit {
            return if bytes % unit == 0 {
                format!("{}{suffix}", bytes / unit)
            } else {
                format!("{:.1}{suffix}", bytes as f64 / unit as f64)
            };
        }
    }
    format!("{bytes}B")
}

/// The host's resolved memory shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostMemory {
    /// Total guest memory this host may use, in bytes.
    pub max_memory: u64,
    /// Ceiling on any single linear memory — `max_memory_size` on the pooling
    /// allocator.
    pub default_heap_memory: u64,
    /// Instance slots the pooling allocator keeps.
    pub core_instances: u32,
    /// Whether `max_memory` came from a flag or was derived, so the startup log
    /// can say which without the caller re-deriving it.
    pub max_memory_from_flag: bool,
}

impl Default for HostMemory {
    fn default() -> Self {
        Self {
            max_memory: derive_max_memory(),
            default_heap_memory: WASMTIME_DEFAULT_HEAP_MEMORY,
            core_instances: WASMTIME_DEFAULT_CORE_INSTANCES,
            max_memory_from_flag: false,
        }
    }
}

impl HostMemory {
    /// Resolve the three knobs, falling back to the derived budget and to
    /// wasmtime's own defaults.
    ///
    /// # Errors
    ///
    /// Rejects a zero for any of the three: each would mean "this host runs
    /// nothing", which is never what an operator meant, and two of them would
    /// panic or misbehave inside wasmtime rather than saying so.
    pub fn resolve(
        max_memory: Option<u64>,
        default_heap_memory: Option<u64>,
        core_instances: Option<u32>,
    ) -> Result<Self, String> {
        if max_memory == Some(0) {
            return Err("--max-memory must be greater than zero".to_string());
        }
        if default_heap_memory == Some(0) {
            return Err("--default-heap-memory must be greater than zero".to_string());
        }
        if core_instances == Some(0) {
            return Err("--core-instances must be at least 1".to_string());
        }
        Ok(Self {
            max_memory: max_memory.unwrap_or_else(derive_max_memory),
            default_heap_memory: default_heap_memory.unwrap_or(WASMTIME_DEFAULT_HEAP_MEMORY),
            core_instances: core_instances.unwrap_or(WASMTIME_DEFAULT_CORE_INSTANCES),
            max_memory_from_flag: max_memory.is_some(),
        })
    }

    /// Address space the pooling allocator will reserve: every slot is sized
    /// for the largest memory it might hold.
    ///
    /// Saturating rather than checked because the answer to an overflow here is
    /// the same as the answer to a merely enormous number — refuse it — and a
    /// saturated `u64` is still comfortably past any real limit.
    pub fn pool_reservation(&self) -> u64 {
        self.default_heap_memory
            .saturating_mul(u64::from(self.core_instances))
    }

    /// What the host should say about this combination at startup, if anything.
    ///
    /// This is the whole reason the budget is worth naming before anything
    /// enforces it: the reservation is a product of two knobs an operator sets
    /// independently, and the failure it causes — instantiation refusing, much
    /// later, with neither number in the message — is unpleasant to diagnose.
    pub fn advisory(&self) -> Option<String> {
        // Reserving address space is not consuming memory: on 64-bit the kernel
        // backs it only when written to. So a reservation past the budget is a
        // warning about over-commitment, not an error — but past what the
        // allocator can actually map, it will fail at the first instantiation.
        if self.pool_reservation() > MAX_POOL_RESERVATION {
            return Some(format!(
                "--default-heap-memory {} across --core-instances {} would reserve {} of \
                 address space, past the {} the pooling allocator is probed for at startup. \
                 Instantiation will fail once the pool is exhausted. Lower one of the two.",
                render_bytes(self.default_heap_memory),
                self.core_instances,
                render_bytes(self.pool_reservation()),
                render_bytes(MAX_POOL_RESERVATION),
            ));
        }
        if self.default_heap_memory > self.max_memory {
            return Some(format!(
                "--default-heap-memory {} is larger than this host's whole memory budget \
                 ({}), so a single guest could exhaust the host on its own",
                render_bytes(self.default_heap_memory),
                render_bytes(self.max_memory),
            ));
        }
        None
    }
}

/// What `is_pooling_allocator_supported` probes for at startup, and therefore
/// the most address space the pool can be assumed to get.
const MAX_POOL_RESERVATION: u64 = 4 * 1024 * 1024 * MIB;

/// The memory limit this process is really subject to.
///
/// cgroup first, because in Kubernetes that is the number behind an OOMKill and
/// the machine's total is irrelevant to a container limited well below it. Both
/// cgroup revisions are read; v2 spells "no limit" as the literal string `max`.
fn detected_memory_limit() -> Option<u64> {
    fn read_cgroup(path: &str) -> Option<u64> {
        let raw = std::fs::read_to_string(path).ok()?;
        let raw = raw.trim();
        if raw == "max" {
            return None;
        }
        let value: u64 = raw.parse().ok()?;
        // cgroup v1 spells "unlimited" as a very large number rather than a
        // word, so anything past what a machine could plausibly have is treated
        // as absent rather than believed.
        (value < u64::MAX / 2).then_some(value)
    }

    read_cgroup("/sys/fs/cgroup/memory.max")
        .or_else(|| read_cgroup("/sys/fs/cgroup/memory/memory.limit_in_bytes"))
        .or_else(system_total_memory)
}

/// Total physical memory, for a host with no cgroup to read — a developer's
/// laptop, or a bare-metal deployment.
fn system_total_memory() -> Option<u64> {
    let mut monitor = crate::host::sysinfo::SystemMonitor::new();
    monitor.refresh();
    let total = monitor.memory_usage().total_memory;
    (total > 0).then_some(total)
}

/// The guest memory budget when an operator names none.
///
/// A share of the limit that will actually kill the process, clamped. Mirrors
/// [`crate::host::quota::default_max_connections`], which takes the same shape
/// of a share of `RLIMIT_NOFILE`: an unset flag means the derived ceiling, never
/// "unbounded".
pub fn derive_max_memory() -> u64 {
    let Some(limit) = detected_memory_limit() else {
        return MIN_DERIVED_MAX_MEMORY;
    };
    limit
        .saturating_mul(GUEST_MEMORY_NUMERATOR)
        .saturating_div(GUEST_MEMORY_DENOMINATOR)
        .clamp(MIN_DERIVED_MAX_MEMORY, MAX_DERIVED_MAX_MEMORY)
}

impl fmt::Display for HostMemory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "max_memory={} default_heap_memory={} core_instances={} pool_reservation={}",
            render_bytes(self.max_memory),
            render_bytes(self.default_heap_memory),
            self.core_instances,
            render_bytes(self.pool_reservation()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_parse_in_binary_units() {
        assert_eq!(parse_bytes("128MiB"), Ok(128 * MIB));
        assert_eq!(parse_bytes("4GiB"), Ok(4 * 1024 * MIB));
        assert_eq!(parse_bytes("512"), Ok(512));
        assert_eq!(parse_bytes(" 256 MiB "), Ok(256 * MIB));
        // `MB` is a spelling of `MiB`, not 10^6: a host that resolved this to
        // 512_000_000 while the cgroup counts 536_870_912 would be quietly
        // wrong about the number that actually kills it.
        // Kubernetes quantities, which the chart passes through verbatim from
        // `resources.limits.memory`. `2Gi` and `2G` are different numbers and
        // must stay different: reading `2G` as binary would have the host
        // believe it has 7% more than the kernel will give it.
        assert_eq!(parse_bytes("2Gi"), Ok(2 * 1024 * MIB));
        assert_eq!(parse_bytes("2G"), Ok(2_000_000_000));
        assert_eq!(parse_bytes("512Mi"), Ok(512 * MIB));
        assert_eq!(parse_bytes("512M"), Ok(512_000_000));
        assert!(
            parse_bytes("2G").unwrap() < parse_bytes("2Gi").unwrap(),
            "the ambiguous-looking form must resolve to the SMALLER number"
        );
        assert_eq!(render_bytes(4000 * 1024 * MIB), "3.9TiB");
        assert_eq!(render_bytes(4 * 1024 * MIB), "4GiB");
        assert!(parse_bytes("12PiB").is_err());
        assert!(parse_bytes("MiB").is_err());
        assert!(parse_bytes("").is_err());
    }

    #[test]
    fn unset_knobs_resolve_to_wasmtimes_own_defaults() {
        // The whole safety property of this change: setting nothing must leave
        // the host exactly as it was.
        let resolved = HostMemory::resolve(None, None, None).expect("defaults are valid");
        assert_eq!(resolved.default_heap_memory, WASMTIME_DEFAULT_HEAP_MEMORY);
        assert_eq!(resolved.core_instances, WASMTIME_DEFAULT_CORE_INSTANCES);
        assert!(!resolved.max_memory_from_flag);
        assert!(
            resolved.max_memory >= MIN_DERIVED_MAX_MEMORY,
            "an unset budget is derived, never zero or unbounded"
        );
    }

    #[test]
    fn a_zero_is_refused_for_each_knob() {
        // Zero means "this host runs nothing" in every case, which is never
        // what was meant — and two of the three would misbehave inside
        // wasmtime rather than saying so.
        assert!(HostMemory::resolve(Some(0), None, None).is_err());
        assert!(HostMemory::resolve(None, Some(0), None).is_err());
        assert!(HostMemory::resolve(None, None, Some(0)).is_err());
    }

    #[test]
    fn the_pool_reservation_is_the_product_of_the_two_pool_knobs() {
        let resolved =
            HostMemory::resolve(Some(8 * 1024 * MIB), Some(128 * MIB), Some(100)).unwrap();
        assert_eq!(resolved.pool_reservation(), 12800 * MIB);
        assert_eq!(resolved.advisory(), None, "a modest pool needs no advisory");
    }

    #[test]
    fn the_stock_defaults_sit_just_under_the_probe() {
        // Worth pinning, because it is the number nobody is told today: 4 GiB
        // per slot across 1000 slots is 3.9 TiB — close enough to the 4 TiB the
        // allocator is probed for that raising either knob crosses it, which is
        // exactly why both are worth naming.
        let stock = HostMemory::resolve(None, None, None).unwrap();
        assert_eq!(stock.pool_reservation(), 4000 * 1024 * MIB);
        assert!(stock.pool_reservation() < MAX_POOL_RESERVATION);
        assert_eq!(render_bytes(stock.pool_reservation()), "3.9TiB");
        assert_eq!(
            stock.advisory(),
            None,
            "the stock configuration must not warn — it is what every host runs today"
        );
    }

    #[test]
    fn an_unreservable_combination_is_reported_with_both_numbers() {
        let resolved = HostMemory::resolve(None, Some(16 * 1024 * MIB), Some(1000)).unwrap();
        let advisory = resolved.advisory().expect("15.6TiB is past the probe");
        assert!(
            advisory.contains("15.6TiB"),
            "names the reservation, to one decimal because it is not a round unit: {advisory}"
        );
        assert!(advisory.contains("1000"), "names the instance count: {advisory}");
        assert!(
            advisory.contains("--default-heap-memory") && advisory.contains("--core-instances"),
            "names both knobs so it is actionable: {advisory}"
        );
    }

    #[test]
    fn a_guest_ceiling_above_the_whole_budget_is_called_out() {
        // Not an error — reserving is not consuming — but an operator who has
        // done this has almost certainly mis-set one of the two.
        let resolved =
            HostMemory::resolve(Some(512 * MIB), Some(2 * 1024 * MIB), Some(4)).unwrap();
        let advisory = resolved.advisory().expect("a guest can exhaust the host");
        assert!(advisory.contains("2GiB") && advisory.contains("512MiB"));
    }
}
