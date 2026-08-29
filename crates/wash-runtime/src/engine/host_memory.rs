//! The three numbers that decide how much memory this host may hand to guests.
//!
//! All three already exist inside wasmtime; none of them was reachable from the
//! outside. This module gives each one a name, a derivation, and a startup line,
//! and checks the three against each other before the engine is built.
//!
//! | Knob | What it bounds | Unset |
//! | --- | --- | --- |
//! | [`HostMemoryBudgets::max_guest_memory`] | Total guest memory this host may use | Derived from the cgroup limit that would OOM-kill this process |
//! | [`HostMemoryBudgets::default_heap_memory`] | How large any single linear memory may grow | wasmtime's own default (4 GiB) |
//! | [`HostMemoryBudgets::core_instances`] | Instance slots the pooling allocator keeps | wasmtime's own default (1000) |
//!
//! **Nothing here changes behaviour when the flags are unset.** Both pool knobs
//! fall through to exactly the values wasmtime has always used, and the budget
//! only *checks* the other two unless a host opts into enforcing it. What
//! enforces it is [`crate::engine::guest_memory`], which counts rather than
//! refuses by default — for the same reason: an unset budget is derived, never
//! absent, so enforcing it out of the box would hand every host a ceiling
//! nobody chose.
//!
//! # Why the budget is worth naming apart from enforcing it
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
const MIN_DERIVED_MAX_GUEST_MEMORY: u64 = 256 * MIB;
const MAX_DERIVED_MAX_GUEST_MEMORY: u64 = 1024 * 1024 * MIB;

/// Parse a byte size, following Kubernetes' quantity grammar.
///
/// | Form | Meaning |
/// | --- | --- |
/// | none, `B` | bytes |
/// | `Ki`, `Mi`, `Gi`, `Ti`, `Pi`, `Ei` (and the `…iB` spellings) | binary (1024ⁿ) |
/// | `k`, `M`, `G`, `T`, `P`, `E` (and the `…B` spellings) | decimal (1000ⁿ) |
/// | `1.5Gi` | fractional, truncated toward zero |
/// | `1e9`, `1.5E3` | decimal exponent |
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
///
/// Fractions evaluate in integer arithmetic: `1.5Gi` is exactly 1610612736.
///
/// # Errors
///
/// An empty string, a mantissa that is not a number, an unknown suffix, a bare
/// `m` or `e`, a nonzero quantity under one byte, or a value past `u64`.
pub fn parse_bytes(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("expected a byte size, e.g. 128MiB".to_string());
    }
    let mantissa_end = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len());
    if mantissa_end == 0 {
        return Err(format!("{s:?} does not start with a number"));
    }
    let (mantissa, suffix) = s.split_at(mantissa_end);
    let suffix = suffix.trim();

    // Case folding below would merge these with `M` and `E`, which are orders
    // of magnitude larger.
    match suffix {
        // Milli, not mega.
        "m" => {
            return Err(format!(
                "{s:?} uses Kubernetes' milli suffix, a thousandth of a byte, which is not a \
                 memory size; write 'M' for megabytes or 'Mi' for mebibytes"
            ));
        }
        // Exponent marker, not exa.
        "e" | "eb" => {
            return Err(format!(
                "{s:?} has no exponent digits; write '1e9' for a decimal exponent, or 'E'/'Ei' \
                 for exabytes/exbibytes"
            ));
        }
        _ => {}
    }

    const KI: u64 = 1024;
    let factor = match suffix.to_ascii_lowercase().as_str() {
        "" | "b" => Some(1),
        // Binary: the `i` forms, as Kubernetes spells them.
        "ki" | "kib" => Some(KI),
        "mi" | "mib" => Some(MIB),
        "gi" | "gib" => Some(KI * MIB),
        "ti" | "tib" => Some(KI * KI * MIB),
        "pi" | "pib" => Some(KI * KI * KI * MIB),
        "ei" | "eib" => Some(KI * KI * KI * KI * MIB),
        // Decimal: SI, and what Kubernetes means by a bare `k`/`M`/`G`/`T`.
        // Bare `m` is refused above; this arm is `M`, `MB`, `mb`.
        "k" | "kb" => Some(1_000),
        "m" | "mb" => Some(1_000_000),
        "g" | "gb" => Some(1_000_000_000),
        "t" | "tb" => Some(1_000_000_000_000),
        "p" | "pb" => Some(1_000_000_000_000_000),
        "e" | "eb" => Some(1_000_000_000_000_000_000),
        _ => None,
    };

    // Decimal exponent. The table above claims `E` and `Ei` first.
    let (factor, exponent) = match factor {
        Some(factor) => (factor, 0i32),
        None => {
            let exponent = suffix.strip_prefix(['e', 'E']).ok_or_else(|| {
                format!(
                    "unknown size suffix {suffix:?}; expected a Kubernetes-style quantity \
                     (Ki/Mi/Gi/Ti/Pi/Ei for binary, k/M/G/T/P/E for decimal, a decimal \
                     exponent such as 1e9, or plain bytes)"
                )
            })?;
            let exponent: i32 = exponent
                .parse()
                .map_err(|_| format!("{suffix:?} is not a valid decimal exponent"))?;
            (1, exponent)
        }
    };

    // Mantissa as a whole number over a power of ten, so fractions stay exact.
    let (digits, fraction_len) = match mantissa.split_once('.') {
        Some((whole, fraction)) => (format!("{whole}{fraction}"), fraction.len() as u32),
        None => (mantissa.to_string(), 0),
    };

    let overflow = || format!("{s:?} overflows a byte count");
    let digits: u128 = digits.parse().map_err(|_| {
        // All digits but unparseable means too long for `u128`, not malformed.
        if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
            overflow()
        } else {
            format!("{mantissa:?} is not a valid number")
        }
    })?;

    let mut scaled = digits
        .checked_mul(u128::from(factor))
        .ok_or_else(overflow)?;
    if exponent > 0 {
        scaled = 10u128
            .checked_pow(exponent.unsigned_abs())
            .and_then(|p| scaled.checked_mul(p))
            .ok_or_else(overflow)?;
    }
    // Fractional digits plus any negative exponent. A divisor past `u128`
    // floors the result to zero, which the check below reports.
    let divisor_pow = fraction_len.saturating_add(exponent.min(0).unsigned_abs());
    let divisor = 10u128.checked_pow(divisor_pow).unwrap_or(u128::MAX);
    let bytes = u64::try_from(scaled / divisor).map_err(|_| overflow())?;

    // `0.5` is not zero. Returning zero would surface through `resolve` as
    // "must be greater than zero".
    if bytes == 0 && digits != 0 {
        return Err(format!("{s:?} is less than one byte"));
    }
    Ok(bytes)
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
            return if bytes.is_multiple_of(unit) {
                format!("{}{suffix}", bytes / unit)
            } else {
                format!("{:.1}{suffix}", bytes as f64 / unit as f64)
            };
        }
    }
    format!("{bytes}B")
}

/// The host's resolved memory budgets.
///
/// Every field bounds memory: `core_instances` is a count of WebAssembly
/// *core* instances (pool slots), each sized for `default_heap_memory`, not a
/// CPU-side knob. Compute is metered separately via
/// [`EngineBuilder::with_fuel_consumption`].
///
/// [`EngineBuilder::with_fuel_consumption`]: crate::engine::EngineBuilder::with_fuel_consumption
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostMemoryBudgets {
    /// Total guest memory this host may use, in bytes.
    pub max_guest_memory: u64,
    /// Ceiling on any single linear memory — `max_memory_size` on the pooling
    /// allocator.
    pub default_heap_memory: u64,
    /// Instance slots the pooling allocator keeps.
    pub core_instances: u32,
}

impl Default for HostMemoryBudgets {
    fn default() -> Self {
        Self {
            max_guest_memory: derive_max_guest_memory(),
            default_heap_memory: WASMTIME_DEFAULT_HEAP_MEMORY,
            core_instances: WASMTIME_DEFAULT_CORE_INSTANCES,
        }
    }
}

impl HostMemoryBudgets {
    /// Resolve the three knobs, falling back to the derived budget and to
    /// wasmtime's own defaults.
    ///
    /// # Errors
    ///
    /// Rejects a zero for any of the three: each would mean "this host runs
    /// nothing", which is never what an operator meant, and two of them would
    /// panic or misbehave inside wasmtime rather than saying so.
    ///
    /// Errors name knobs canonically (`max-guest-memory`), not as flags. A
    /// value may arrive from YAML or the environment, never from a flag.
    pub fn resolve(
        max_guest_memory: Option<u64>,
        default_heap_memory: Option<u64>,
        core_instances: Option<u32>,
    ) -> Result<Self, String> {
        if max_guest_memory == Some(0) {
            return Err("max-guest-memory must be greater than zero".to_string());
        }
        if default_heap_memory == Some(0) {
            return Err("default-heap-memory must be greater than zero".to_string());
        }
        if core_instances == Some(0) {
            return Err("core-instances must be at least 1".to_string());
        }
        Ok(Self {
            max_guest_memory: max_guest_memory.unwrap_or_else(derive_max_guest_memory),
            default_heap_memory: default_heap_memory.unwrap_or(WASMTIME_DEFAULT_HEAP_MEMORY),
            core_instances: core_instances.unwrap_or(WASMTIME_DEFAULT_CORE_INSTANCES),
        })
    }

    /// [`Self::resolve`] over the strings a host's configuration holds.
    ///
    /// Sizes follow [`parse_bytes`]. A blank value counts as unset: clap's
    /// `env` fallback yields `Some("")` for a variable that is set but empty.
    ///
    /// # Errors
    ///
    /// A size that does not parse, naming the knob, plus everything
    /// [`Self::resolve`] rejects.
    pub fn resolve_strs(
        max_guest_memory: Option<&str>,
        default_heap_memory: Option<&str>,
        core_instances: Option<u32>,
    ) -> Result<Self, String> {
        let parse = |value: Option<&str>, knob: &str| -> Result<Option<u64>, String> {
            value
                .map(str::trim)
                .filter(|raw| !raw.is_empty())
                .map(|raw| parse_bytes(raw).map_err(|e| format!("invalid {knob}: {e}")))
                .transpose()
        };
        Self::resolve(
            parse(max_guest_memory, "max-guest-memory")?,
            parse(default_heap_memory, "default-heap-memory")?,
            core_instances,
        )
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
                "default-heap-memory {} across core-instances {} would reserve {} of \
                 address space, past the {} the pooling allocator is probed for at startup. \
                 Instantiation will fail once the pool is exhausted. Lower one of the two.",
                render_bytes(self.default_heap_memory),
                self.core_instances,
                render_bytes(self.pool_reservation()),
                render_bytes(MAX_POOL_RESERVATION),
            ));
        }
        if self.default_heap_memory > self.max_guest_memory {
            return Some(format!(
                "default-heap-memory {} is larger than this host's whole memory budget \
                 ({}), so a single guest could exhaust the host on its own",
                render_bytes(self.default_heap_memory),
                render_bytes(self.max_guest_memory),
            ));
        }
        None
    }

    /// What to say about enforcing this budget on this machine, if anything.
    ///
    /// Separate from [`Self::advisory`] because it is only worth saying when
    /// the budget is a real ceiling: in count mode a budget larger than the
    /// machine costs nothing, because nothing is refused.
    ///
    /// The check is against the limit that would actually OOM-kill this
    /// process, not against the budget's own derivation, so it catches the
    /// misconfiguration however it arrived — a flag, an environment variable,
    /// or a Helm chart passing `resources.limits.memory` straight through.
    /// A *derived* budget is three quarters of that limit and never trips it.
    pub fn enforcement_advisory(&self) -> Option<String> {
        let limit = detected_memory_limit()?;
        if self.max_guest_memory.saturating_mul(100)
            <= limit.saturating_mul(MAX_ENFORCED_SHARE_PERCENT)
        {
            return None;
        }
        Some(format!(
            "max-guest-memory {} is {}% of the {} this process is actually limited to, and \
             guest memory is being enforced. Everything a guest is not — wasmtime itself, \
             compiled module images, NATS, OCI pulls, host buffers — comes out of the same \
             limit and is not charged to this budget, so the kernel is likely to OOM-kill \
             this host before the budget ever refuses a guest. An unset budget reserves a \
             quarter of the limit for them; set max-guest-memory below the container limit, \
             or raise the limit.",
            render_bytes(self.max_guest_memory),
            self.max_guest_memory.saturating_mul(100) / limit.max(1),
            render_bytes(limit),
        ))
    }
}

/// Share of the real memory limit an *enforced* guest budget may claim before
/// the host says the number leaves it no room to be a host.
///
/// Above the derived 75%, so a host that named no budget never trips it, and
/// below 100%, which is the value a chart passing `limits.memory` through
/// produces.
const MAX_ENFORCED_SHARE_PERCENT: u64 = 90;

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
///
/// Refreshes RAM only. [`SystemMonitor`] samples every CPU core for the host's
/// periodic reporting, which this path would pay on every engine build.
///
/// [`SystemMonitor`]: crate::host::sysinfo::SystemMonitor
fn system_total_memory() -> Option<u64> {
    use sysinfo::{MemoryRefreshKind, RefreshKind, System};

    let system = System::new_with_specifics(
        RefreshKind::new().with_memory(MemoryRefreshKind::new().with_ram()),
    );
    let total = system.total_memory();
    (total > 0).then_some(total)
}

/// The guest memory budget when an operator names none.
///
/// A share of the limit that will actually kill the process, clamped. Mirrors
/// [`crate::host::quota::default_max_connections`], which takes the same shape
/// of a share of `RLIMIT_NOFILE`: an unset flag means the derived ceiling, never
/// "unbounded".
pub fn derive_max_guest_memory() -> u64 {
    let Some(limit) = detected_memory_limit() else {
        return MIN_DERIVED_MAX_GUEST_MEMORY;
    };
    limit
        .saturating_mul(GUEST_MEMORY_NUMERATOR)
        .saturating_div(GUEST_MEMORY_DENOMINATOR)
        .clamp(MIN_DERIVED_MAX_GUEST_MEMORY, MAX_DERIVED_MAX_GUEST_MEMORY)
}

impl fmt::Display for HostMemoryBudgets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "max_guest_memory={} default_heap_memory={} core_instances={} pool_reservation={}",
            render_bytes(self.max_guest_memory),
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
        // The large binary suffixes are part of the grammar and fit a u64.
        assert_eq!(parse_bytes("12PiB"), Ok(12 * 1024 * 1024 * 1024 * MIB));
        assert!(parse_bytes("MiB").is_err());
        assert!(parse_bytes("").is_err());
    }

    #[test]
    fn the_memory_only_refresh_still_reports_total_ram() {
        // A refresh kind that stops populating `total_memory` fails silently:
        // the derivation drops every non-cgroup host to the 256MiB floor.
        let total =
            system_total_memory().expect("a machine running this test has readable total memory");
        assert!(
            total >= MIN_DERIVED_MAX_GUEST_MEMORY,
            "implausible total memory {total}, the refresh kind likely stopped populating it"
        );
    }

    #[test]
    fn milli_is_refused_rather_than_read_as_mega() {
        // `500m` is half a byte to Kubernetes. Folded onto `M` it reads as
        // 500_000_000, a billion-fold over-read.
        let err = parse_bytes("500m").expect_err("milli is not a memory size");
        assert!(
            err.contains("milli") && err.contains("Mi"),
            "the error must name the suffix and offer the one that was meant: {err}"
        );
        assert_eq!(parse_bytes("500M"), Ok(500_000_000));
    }

    #[test]
    fn a_truncated_exponent_is_refused_rather_than_read_as_exa() {
        // Folded onto `E`, a `1e9` that lost its digits reads as an exabyte.
        for input in ["1e", "1eb"] {
            let err = parse_bytes(input).expect_err("a bare exponent marker is not a size");
            assert!(
                err.contains("exponent"),
                "the error must name what is missing: {err}"
            );
        }
        assert_eq!(parse_bytes("1E"), Ok(1_000_000_000_000_000_000));
        assert_eq!(parse_bytes("1e9"), Ok(1_000_000_000));
    }

    #[test]
    fn a_sub_byte_quantity_is_refused_rather_than_truncated_to_zero() {
        // `0.5` is not zero, and `resolve` would report a zero as "must be
        // greater than zero".
        for input in ["0.5", "1e-3", "0.0001"] {
            let err = parse_bytes(input).expect_err("a sub-byte quantity is refused");
            assert!(
                err.contains("less than one byte"),
                "the error must say what happened: {err}"
            );
        }
        // A genuine zero is still a zero, for `resolve` to reject on its terms.
        assert_eq!(parse_bytes("0"), Ok(0));
    }

    #[test]
    fn an_over_long_mantissa_reports_an_overflow_not_a_bad_number() {
        // The digits are digits; they just do not fit the arithmetic.
        let err = parse_bytes(&"9".repeat(42)).expect_err("42 digits overflow a byte count");
        assert!(
            err.contains("overflows"),
            "an all-digit mantissa that will not fit is an overflow: {err}"
        );
    }

    #[test]
    fn fractional_and_exponent_quantities_are_accepted() {
        // Both are legal in `resources.limits.memory`.
        assert_eq!(parse_bytes("1.5Gi"), Ok(1_610_612_736));
        assert_eq!(parse_bytes("0.5Mi"), Ok(MIB / 2));
        assert_eq!(parse_bytes("1e9"), Ok(1_000_000_000));
        assert_eq!(parse_bytes("1.5E3"), Ok(1_500));
        // Exact, not an `f64` approximation of 1.1 x 2^30.
        assert_eq!(parse_bytes("1.1Gi"), Ok(1_181_116_006));
        // Truncates rather than rounding up, so it can never over-read.
        assert_eq!(parse_bytes("1.9"), Ok(1));
        // The suffix table claims `E` and `Ei` before the exponent branch.
        assert_eq!(parse_bytes("1E"), Ok(1_000_000_000_000_000_000));
        assert_eq!(parse_bytes("1Ei"), Ok(1024 * 1024 * 1024 * 1024 * MIB));
        assert!(parse_bytes("1.2.3Gi").is_err());
        assert!(parse_bytes("1eX").is_err());
    }

    #[test]
    fn unset_knobs_resolve_to_wasmtimes_own_defaults() {
        // The whole safety property of this change: setting nothing must leave
        // the host exactly as it was.
        let resolved = HostMemoryBudgets::resolve(None, None, None).expect("defaults are valid");
        assert_eq!(resolved.default_heap_memory, WASMTIME_DEFAULT_HEAP_MEMORY);
        assert_eq!(resolved.core_instances, WASMTIME_DEFAULT_CORE_INSTANCES);
        assert!(
            resolved.max_guest_memory >= MIN_DERIVED_MAX_GUEST_MEMORY,
            "an unset budget is derived, never zero or unbounded"
        );
    }

    #[test]
    fn string_knobs_resolve_the_same_as_parsed_ones() {
        // The string form is `resolve` plus a parse, nothing more.
        assert_eq!(
            HostMemoryBudgets::resolve_strs(Some("8GiB"), Some("128MiB"), Some(100)),
            HostMemoryBudgets::resolve(Some(8 * 1024 * MIB), Some(128 * MIB), Some(100))
        );
        // `2G` and `2Gi` are different numbers and stay different here.
        let decimal = HostMemoryBudgets::resolve_strs(Some("2G"), None, None).unwrap();
        let binary = HostMemoryBudgets::resolve_strs(Some("2Gi"), None, None).unwrap();
        assert_eq!(decimal.max_guest_memory, 2_000_000_000);
        assert_eq!(binary.max_guest_memory, 2 * 1024 * MIB);
        // The zero checks apply to a parsed size too, not just a passed one.
        assert!(HostMemoryBudgets::resolve_strs(Some("0"), None, None).is_err());
    }

    #[test]
    fn a_blank_knob_is_unset_rather_than_a_parse_failure() {
        // An empty ConfigMap key or `value: ""` reaches clap as `Some("")`.
        let blank = HostMemoryBudgets::resolve_strs(Some(""), Some("   "), None)
            .expect("a blank knob is unset, not a bad size");
        // Compared against the fixed defaults, not a second `resolve(None, ..)`:
        // the derived budget re-reads the cgroup and could differ.
        assert_eq!(blank.default_heap_memory, WASMTIME_DEFAULT_HEAP_MEMORY);
        assert!(blank.max_guest_memory >= MIN_DERIVED_MAX_GUEST_MEMORY);
    }

    #[test]
    fn an_unparseable_size_names_the_knob_it_came_from() {
        // Two of the three knobs are sizes; the parse error alone does not say
        // which one failed.
        let err = HostMemoryBudgets::resolve_strs(None, Some("512 gigabytes"), None)
            .expect_err("an unparseable size is refused");
        assert!(
            err.contains("default-heap-memory"),
            "the error must name the knob: {err}"
        );

        let err = HostMemoryBudgets::resolve_strs(Some("lots"), None, None)
            .expect_err("an unparseable size is refused");
        assert!(
            err.contains("max-guest-memory"),
            "the error must name the knob: {err}"
        );
    }

    #[test]
    fn a_zero_is_refused_for_each_knob() {
        // Zero means "this host runs nothing" in every case, which is never
        // what was meant — and two of the three would misbehave inside
        // wasmtime rather than saying so.
        assert!(HostMemoryBudgets::resolve(Some(0), None, None).is_err());
        assert!(HostMemoryBudgets::resolve(None, Some(0), None).is_err());
        assert!(HostMemoryBudgets::resolve(None, None, Some(0)).is_err());
    }

    #[test]
    fn the_pool_reservation_is_the_product_of_the_two_pool_knobs() {
        let resolved =
            HostMemoryBudgets::resolve(Some(8 * 1024 * MIB), Some(128 * MIB), Some(100)).unwrap();
        assert_eq!(resolved.pool_reservation(), 12800 * MIB);
        assert_eq!(resolved.advisory(), None, "a modest pool needs no advisory");
    }

    #[test]
    fn the_stock_defaults_sit_just_under_the_probe() {
        // Worth pinning, because it is the number nobody is told today: 4 GiB
        // per slot across 1000 slots is 3.9 TiB — close enough to the 4 TiB the
        // allocator is probed for that raising either knob crosses it, which is
        // exactly why both are worth naming.
        let stock = HostMemoryBudgets::resolve(None, None, None).unwrap();
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
        let resolved = HostMemoryBudgets::resolve(None, Some(16 * 1024 * MIB), Some(1000)).unwrap();
        let advisory = resolved.advisory().expect("15.6TiB is past the probe");
        assert!(
            advisory.contains("15.6TiB"),
            "names the reservation, to one decimal because it is not a round unit: {advisory}"
        );
        assert!(
            advisory.contains("1000"),
            "names the instance count: {advisory}"
        );
        assert!(
            advisory.contains("default-heap-memory") && advisory.contains("core-instances"),
            "names both knobs so it is actionable: {advisory}"
        );
    }

    /// The chart passes `resources.limits.memory` through as the guest budget
    /// verbatim, so an operator turning enforcement on gets a ceiling equal to
    /// 100% of the pod — and the host's own overhead, which this budget does
    /// not charge, then OOM-kills the pod before the budget refuses anything.
    #[test]
    fn enforcing_a_budget_that_leaves_the_host_no_room_is_called_out() {
        let limit = detected_memory_limit().expect("a test machine has a readable memory limit");

        // The whole limit: what the chart renders today.
        let whole = HostMemoryBudgets::resolve(Some(limit), None, None).unwrap();
        let advisory = whole
            .enforcement_advisory()
            .expect("a budget equal to the real limit leaves the host nothing");
        assert!(
            advisory.contains("max-guest-memory") && advisory.contains("OOM"),
            "the advisory must name the knob and the consequence: {advisory}"
        );

        // Half of it: room to spare, nothing to say.
        let modest = HostMemoryBudgets::resolve(Some(limit / 2), None, None).unwrap();
        assert_eq!(modest.enforcement_advisory(), None);
    }

    /// The derived budget is three quarters of the limit, so a host that named
    /// no budget must never trip the advisory — otherwise every host that
    /// turned enforcement on would be warned about a number it did not choose.
    #[test]
    fn a_derived_budget_is_never_warned_about() {
        let derived = HostMemoryBudgets::resolve(None, None, None).unwrap();
        // Only meaningful where the derivation was not clamped: the 256MiB
        // floor can legitimately exceed a tiny container's limit, and being
        // told so is correct.
        let limit = detected_memory_limit().expect("a test machine has a readable memory limit");
        if derived.max_guest_memory > MIN_DERIVED_MAX_GUEST_MEMORY
            && derived.max_guest_memory < MAX_DERIVED_MAX_GUEST_MEMORY
        {
            assert_eq!(
                derived.enforcement_advisory(),
                None,
                "a derived budget is {} of a {} limit and reserves its own headroom",
                render_bytes(derived.max_guest_memory),
                render_bytes(limit),
            );
        }
    }

    #[test]
    fn a_guest_ceiling_above_the_whole_budget_is_called_out() {
        // Not an error — reserving is not consuming — but an operator who has
        // done this has almost certainly mis-set one of the two.
        let resolved =
            HostMemoryBudgets::resolve(Some(512 * MIB), Some(2 * 1024 * MIB), Some(4)).unwrap();
        let advisory = resolved.advisory().expect("a guest can exhaust the host");
        assert!(advisory.contains("2GiB") && advisory.contains("512MiB"));
    }
}
