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
//! | [`HostMemoryBudgets::max_memories`] | Linear memories the pooling allocator keeps room for, and with them the fiber stacks and component instances it admits | wasmtime's own default (1000) |
//! | [`HostMemoryBudgets::core_instances`] | Core instances (live core modules) the pool admits, a counter that reserves nothing | [`crate::engine::CORE_INSTANCES_PER_MEMORY`] per memory |
//!
//! Both pool knobs fall through to exactly the values wasmtime has always
//! used when unset. The budget is derived when unset, never absent, and
//! [`crate::engine::guest_memory`] enforces it by default: a host that names
//! nothing still refuses guest growth past three quarters of its real limit.
//!
//! # Why the budget is worth naming apart from enforcing it
//!
//! Every pool slot reserves address space for the largest memory it might hold,
//! whether or not anything grows into it: wasmtime's 4 GiB
//! `memory_reservation`, plus a guard region (see [`pool_slot_bytes`]). A
//! `default_heap_memory` past the reservation does not make the slot larger;
//! wasmtime refuses to build that pool at all. Today a host claims 1000 such slots, 3.9 TiB of
//! address space, and nobody is told; the 128 TiB a 64-bit process can map
//! holds about 30,000 of them, so `core_instances` has a ceiling. Naming the
//! budget lets the host say, at startup, whether the pool it is about to build
//! is one the machine could actually back.
//!
//! # The reservation is not a knob
//!
//! Lowering `default_heap_memory` below 4 GiB caps how far a guest may grow,
//! but it does not shrink the slot: wasmtime still reserves 4 GiB for it. That
//! reservation, with its guard, is what lets wasmtime elide bounds checks on
//! 32-bit memories and keeps one guest's out-of-bounds access from reaching
//! another's slot. It is an isolation boundary, so the host accounts for it
//! rather than offering a way to shrink it; the slot count is what sizes the
//! pool.

use std::{
    collections::BTreeSet,
    fmt,
    path::{Component, Path, PathBuf},
};

use super::pool_usage::PoolTotals;

/// One mebibyte.
pub const MIB: u64 = 1024 * 1024;

/// wasmtime's default for `PoolingAllocationConfig::max_memory_size`.
///
/// Restated here so the startup log can say what the host resolved even when
/// the operator set nothing — "unset" is not an answer to "how large may a
/// linear memory get".
pub const WASMTIME_DEFAULT_HEAP_MEMORY: u64 = 4 * 1024 * MIB;

/// wasmtime's default for the pooling allocator's memory count, and what
/// [`crate::engine::EngineBuilder`] has always passed when `max_instances` is
/// unset.
pub const WASMTIME_DEFAULT_MEMORIES: u32 = 1000;

/// The old name for [`WASMTIME_DEFAULT_MEMORIES`], from when one count sized
/// every pool total.
#[deprecated(since = "2.12.0", note = "use WASMTIME_DEFAULT_MEMORIES")]
pub const WASMTIME_DEFAULT_CORE_INSTANCES: u32 = WASMTIME_DEFAULT_MEMORIES;

/// wasmtime's default `memory_reservation`: the address space each pooled
/// linear memory slot reserves, however low its heap ceiling.
///
/// Restated, not configured: the engine never changes it, and the module docs
/// say why it is not a knob.
#[cfg(target_pointer_width = "64")]
pub const WASMTIME_MEMORY_RESERVATION: u64 = 4 * 1024 * MIB;
#[cfg(not(target_pointer_width = "64"))]
pub const WASMTIME_MEMORY_RESERVATION: u64 = 10 * MIB;

/// wasmtime's default `memory_guard_size`: unmapped address space after each
/// pooled memory, and once more before the first.
#[cfg(target_pointer_width = "64")]
pub const WASMTIME_MEMORY_GUARD: u64 = 32 * MIB;
#[cfg(not(target_pointer_width = "64"))]
pub const WASMTIME_MEMORY_GUARD: u64 = 64 * 1024;

/// Address space one pooled memory slot reserves when the heap ceiling is
/// `max_memory_size`: the larger of the ceiling and
/// [`WASMTIME_MEMORY_RESERVATION`], plus [`WASMTIME_MEMORY_GUARD`].
///
/// wasmtime's own slab layout without memory protection keys, which it leaves
/// off by default. It assumes the engine's reservation and guard are
/// wasmtime's defaults; a caller-supplied base config that changes them makes
/// this an estimate.
///
/// A ceiling above the reservation does not make a bigger slot in practice:
/// wasmtime refuses to build a pool whose `max_memory_size` exceeds its
/// `memory_reservation`, and the engine never raises the reservation (see
/// [`pool_heap_ceiling_advisory`]). So every pool that builds has slots of
/// exactly the reservation plus the guard; the larger figure is only what
/// such a pool would have needed.
pub fn pool_slot_bytes(max_memory_size: u64) -> u64 {
    max_memory_size
        .max(WASMTIME_MEMORY_RESERVATION)
        .saturating_add(WASMTIME_MEMORY_GUARD)
}

/// Address space a pool of `slots` memories reserves: one [`pool_slot_bytes`]
/// each, plus the guard wasmtime places before the first.
///
/// Saturating for the reason [`HostMemoryBudgets::pool_reservation`] gives.
pub fn pool_slab_bytes(max_memory_size: u64, slots: u32) -> u64 {
    pool_slot_bytes(max_memory_size)
        .saturating_mul(u64::from(slots))
        .saturating_add(WASMTIME_MEMORY_GUARD)
}

/// The page the table pool rounds each table up to: the host's, as wasmtime
/// reads it. 4 KiB on x86-64 and on Windows, 16 KiB on Apple silicon, where a
/// one-element table costs four times what the Linux default would suggest
/// and a 20,000-element one rounds to the same ten 16 KiB pages as forty 4 KiB
/// ones. Only the estimate's input: wasmtime maps the real pool.
fn table_pool_page() -> u64 {
    #[cfg(unix)]
    {
        u64::try_from(rustix::param::page_size()).unwrap_or(4096)
    }
    #[cfg(not(unix))]
    {
        4096
    }
}

/// Bytes the table pool maps for `tables` tables of `table_elements` pointers
/// each, every table rounded up to the host's page.
///
/// Unlike the memory slab this is mapped accessible in one piece when the
/// engine is built. What that costs depends on the platform: see
/// [`TableMappingCharge`].
pub fn pool_table_bytes(tables: u32, table_elements: usize) -> u64 {
    let page = table_pool_page();
    let pointer = u64::try_from(size_of::<usize>()).unwrap_or(8);
    let table = u64::try_from(table_elements)
        .unwrap_or(u64::MAX)
        .saturating_mul(pointer)
        .div_ceil(page)
        .saturating_mul(page);
    table.saturating_mul(u64::from(tables))
}

/// Core instances `memories` memories derive, at the engine's ratio.
fn derived_core_instances(memories: u32) -> u32 {
    super::derived_pool_totals(memories).core_instances
}

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
/// Kubernetes semantics rather than "binary everywhere", because an operator
/// sizing a host pod writes this next to `resources.limits.memory` — a
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
/// Every field bounds memory: `max_memories` is a count of pool memory
/// slots, each sized by [`pool_slot_bytes`], and `core_instances` a count of
/// live core modules, not a CPU-side knob. Compute is metered separately via
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
    /// Linear memories the pooling allocator keeps room for, and with them
    /// the fiber stacks, component instances and GC heaps it admits: one
    /// count for all, since a component instance of the usual shape takes one
    /// of each. Each memory is a slot of [`pool_slot_bytes`], so this is the
    /// count that sizes the pool. `--max-memories`.
    pub max_memories: u32,
    /// Core instances (live core modules) the pool admits at once: a counter
    /// that reserves nothing. `--core-instances`. Unset, it is
    /// [`crate::engine::CORE_INSTANCES_PER_MEMORY`] per memory, so a
    /// component made of several core modules does not fill it before the
    /// memories run out; set alone, `max_memories` is derived from it by the
    /// same ratio; set beside `max_memories`, both stand as given and
    /// [`Self::core_instance_advisory`] says so when they disagree.
    pub core_instances: u32,
}

impl Default for HostMemoryBudgets {
    fn default() -> Self {
        Self::derived_within(detected_memory_limit())
    }
}

impl HostMemoryBudgets {
    /// [`Self::default`] from a limit the caller already detected.
    pub(crate) fn derived_within(memory_limit: Option<u64>) -> Self {
        Self {
            max_guest_memory: derive_max_guest_memory_within(memory_limit),
            default_heap_memory: WASMTIME_DEFAULT_HEAP_MEMORY,
            max_memories: WASMTIME_DEFAULT_MEMORIES,
            core_instances: derived_core_instances(WASMTIME_DEFAULT_MEMORIES),
        }
    }
}

impl HostMemoryBudgets {
    /// Resolve the four knobs, falling back to the derived budget and to
    /// wasmtime's own defaults.
    ///
    /// `max_memories` and `core_instances` derive from each other at
    /// [`crate::engine::CORE_INSTANCES_PER_MEMORY`] when only one is given:
    /// a memory count takes that many core instances per memory, and a
    /// core-instance count the memories that many fit in. Given both, both
    /// stand as the operator set them, whether or not they agree; see
    /// [`Self::core_instance_advisory`].
    ///
    /// # Errors
    ///
    /// Rejects a zero for any of them: each would mean "this host runs
    /// nothing", which is never what an operator meant, and some would panic
    /// or misbehave inside wasmtime rather than saying so.
    ///
    /// Errors name knobs canonically (`max-guest-memory`), not as flags. A
    /// value may arrive from YAML or the environment, never from a flag.
    pub fn resolve(
        max_guest_memory: Option<u64>,
        default_heap_memory: Option<u64>,
        max_memories: Option<u32>,
        core_instances: Option<u32>,
    ) -> Result<Self, String> {
        if max_guest_memory == Some(0) {
            return Err("max-guest-memory must be greater than zero".to_string());
        }
        if default_heap_memory == Some(0) {
            return Err("default-heap-memory must be greater than zero".to_string());
        }
        if max_memories == Some(0) {
            return Err("max-memories must be at least 1".to_string());
        }
        if core_instances == Some(0) {
            return Err("core-instances must be at least 1".to_string());
        }
        let (max_memories, core_instances) = match (max_memories, core_instances) {
            (None, None) => (
                WASMTIME_DEFAULT_MEMORIES,
                derived_core_instances(WASMTIME_DEFAULT_MEMORIES),
            ),
            (Some(memories), None) => (memories, derived_core_instances(memories)),
            (None, Some(core)) => (super::memories_for_core_instances(core), core),
            (Some(memories), Some(core)) => (memories, core),
        };
        Ok(Self {
            max_guest_memory: max_guest_memory.unwrap_or_else(derive_max_guest_memory),
            default_heap_memory: default_heap_memory.unwrap_or(WASMTIME_DEFAULT_HEAP_MEMORY),
            max_memories,
            core_instances,
        })
    }

    /// What to say when `core_instances` is not what `max_memories` would
    /// derive, if anything: an operator set both, and they disagree. Both are
    /// installed as given, so the pool runs out of whichever is spent first.
    pub fn core_instance_advisory(&self) -> Option<String> {
        let derived = derived_core_instances(self.max_memories);
        (self.core_instances != derived).then(|| {
            format!(
                "core-instances {} does not match max-memories {}, which would derive {derived} \
                 core instances at {} per memory; both are installed as given, so the pool holds \
                 {} memories and {} core instances and refuses an instantiation once either is \
                 spent. Set one and let the other derive, or set both to match",
                self.core_instances,
                self.max_memories,
                super::CORE_INSTANCES_PER_MEMORY,
                self.max_memories,
                self.core_instances,
            )
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
        max_memories: Option<u32>,
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
            max_memories,
            core_instances,
        )
    }

    /// Address space the pooling allocator will reserve: wasmtime's 4 GiB
    /// reservation and a guard for every slot, however small its heap (see
    /// [`pool_slab_bytes`]).
    ///
    /// Saturating rather than checked because the answer to an overflow here is
    /// the same as the answer to a merely enormous number — refuse it — and a
    /// saturated `u64` is still comfortably past any real limit.
    pub fn pool_reservation(&self) -> u64 {
        pool_slab_bytes(self.default_heap_memory, self.max_memories)
    }

    /// What the host should say about these knobs at startup, if anything.
    ///
    /// Only what the knobs say on their own: a heap ceiling larger than the
    /// whole budget. What the pool reserves is judged against the pool the
    /// engine actually installs (see [`pool_advisories`]), which the
    /// environment, a caller's own pooling config, or no pool at all can make
    /// different from what these knobs ask for.
    pub fn advisory(&self) -> Option<String> {
        (self.default_heap_memory > self.max_guest_memory).then(|| {
            format!(
                "default-heap-memory {} is larger than this host's whole memory budget \
                 ({}), so a single guest could exhaust the host on its own",
                render_bytes(self.default_heap_memory),
                render_bytes(self.max_guest_memory),
            )
        })
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
    /// or a manifest setting it to the pod's own `resources.limits.memory`.
    /// A *derived* budget is three quarters of that limit and never trips it.
    pub fn enforcement_advisory(&self) -> Option<String> {
        self.enforcement_advisory_within(detected_memory_limit())
    }

    /// [`Self::enforcement_advisory`] against a limit the caller already
    /// detected.
    pub(crate) fn enforcement_advisory_within(&self, memory_limit: Option<u64>) -> Option<String> {
        let limit = memory_limit?;
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

/// What to say about an installed pool, with memories sized for a
/// `max_memory_size` heap, at startup: each check that has something to say,
/// independently, so one finding never hides the other.
///
/// This is the whole reason the reservation is worth naming before anything
/// enforces it: it is a product of two knobs an operator sets independently,
/// and the failure it causes — the engine or instantiation refusing, with
/// neither number in the message — is unpleasant to diagnose.
pub(crate) fn pool_advisories(
    totals: &PoolTotals,
    max_memory_size: u64,
    table_elements: usize,
    memory_limit: Option<u64>,
) -> Vec<String> {
    let charge = table_mapping_charge();
    [
        pool_heap_ceiling_advisory(max_memory_size),
        pool_reservation_advisory(totals.memories, max_memory_size, usable_address_space()),
        max_map_count().and_then(|limit| pool_map_count_advisory(totals, limit)),
        pool_table_commit_advisory(
            totals.tables,
            table_elements,
            table_commit_ceiling(charge, memory_limit),
            charge,
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// What mapping the table pool accessible costs on this platform, which
/// decides whether its size is worth a word at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TableMappingCharge {
    /// Every accessible page is committed when mapped: Windows, and Linux
    /// with `vm.overcommit_memory=2`. The whole table pool counts against the
    /// commit limit from startup.
    Committed,
    /// Linux's default heuristic (`vm.overcommit_memory=0`): a page costs
    /// nothing until written, but one mapping obviously larger than memory
    /// and swap together is refused outright, and the table pool is one
    /// mapping.
    Heuristic,
    /// Nothing until written, whatever the size: Linux with
    /// `vm.overcommit_memory=1`, macOS and the BSDs.
    Free,
}

pub(crate) fn table_mapping_charge() -> TableMappingCharge {
    if cfg!(windows) {
        return TableMappingCharge::Committed;
    }
    match std::fs::read_to_string("/proc/sys/vm/overcommit_memory")
        .as_deref()
        .map(str::trim)
    {
        Ok("2") => TableMappingCharge::Committed,
        Ok("0") => TableMappingCharge::Heuristic,
        _ => TableMappingCharge::Free,
    }
}

/// What the table pool's mapping is measured against on this platform: the
/// smaller of what the kernel accounts the mapping against and what this
/// process may use, or nothing where mapping is free.
///
/// The kernel's figure is host-wide — the commit limit where mappings are
/// committed, memory and swap together under the overcommit heuristic — and
/// it alone decides whether the mapping is refused: a cgroup charges a page
/// once it is touched, and the pool's are not until a table is used. But a
/// pool that could touch a large share of a container's limit is one the
/// container's operator should hear about, so `memory_limit` (the cgroup
/// limit the builder detected, or physical memory where there is none) caps
/// the ceiling rather than being ignored.
fn table_commit_ceiling(charge: TableMappingCharge, memory_limit: Option<u64>) -> Option<u64> {
    let kernel = match charge {
        TableMappingCharge::Committed => commit_limit().or_else(system_memory_and_swap),
        TableMappingCharge::Heuristic => system_memory_and_swap(),
        TableMappingCharge::Free => return None,
    };
    kernel.into_iter().chain(memory_limit).min()
}

/// Linux's `CommitLimit`: what `vm.overcommit_memory=2` lets the whole host
/// commit, swap plus `vm.overcommit_ratio` of RAM. `None` elsewhere.
fn commit_limit() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kib: u64 = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("CommitLimit:"))?
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    Some(kib.saturating_mul(1024))
}

/// Physical memory and swap together: what Linux's overcommit heuristic
/// compares one mapping against, and the most Windows can commit, where the
/// swap is the page file.
fn system_memory_and_swap() -> Option<u64> {
    use sysinfo::{MemoryRefreshKind, RefreshKind, System};

    let system = System::new_with_specifics(
        RefreshKind::new().with_memory(MemoryRefreshKind::new().with_ram().with_swap()),
    );
    let total = system.total_memory().saturating_add(system.total_swap());
    (total > 0).then_some(total)
}

/// Share of the commit limit the table pool may take before it is worth a
/// word: past this, a count chosen for the memories is costing real memory
/// through a total the operator never set.
const TABLE_COMMIT_PERCENT: u64 = 10;

/// What to say when the table pool's one accessible mapping is large enough
/// to matter on this platform ([`TableMappingCharge`]), if it is: a
/// noticeable share of `ceiling` where the mapping is committed, more than
/// `ceiling` where a kernel heuristic would refuse it or a container would
/// be killed for touching it. See [`pool_table_bytes`]; `ceiling` is
/// [`table_commit_ceiling`]'s.
///
/// The table total is a multiple of the memory count (see
/// [`crate::engine`]), so it grows with `--max-memories` though that flag
/// says nothing about tables, and nothing else bounds it: the memory
/// reservation is address space, which the reservation advisory covers, and
/// the table pool is one mapping, which the map-count advisory ignores. A
/// ceiling that cannot be read means no threshold to compare against, so
/// nothing is said.
fn pool_table_commit_advisory(
    tables: u32,
    table_elements: usize,
    ceiling: Option<u64>,
    charge: TableMappingCharge,
) -> Option<String> {
    let ceiling = ceiling?;
    let mapped = pool_table_bytes(tables, table_elements);
    let remedy = "Lower max-memories, which sizes the table total from the memory count, or set \
                  WASMTIME_POOLING_TOTAL_TABLES (or WASMTIME_POOLING_TABLE_ELEMENTS) on its own.";
    match charge {
        TableMappingCharge::Committed if mapped > ceiling / 100 * TABLE_COMMIT_PERCENT => {
            Some(format!(
                "{tables} pooled tables of {table_elements} elements commit {} at startup, since \
                 this platform commits every accessible mapping, past {TABLE_COMMIT_PERCENT}% of \
                 the {} this process may commit. The engine may fail to start, or guests may be \
                 left little to grow into. {remedy}",
                render_bytes(mapped),
                render_bytes(ceiling),
            ))
        }
        TableMappingCharge::Heuristic if mapped > ceiling => Some(format!(
            "{tables} pooled tables of {table_elements} elements are mapped in one piece at \
             startup, {} against the {} this process may use. This kernel's overcommit \
             heuristic (vm.overcommit_memory=0) refuses one mapping past the host's memory and \
             swap together, and a container is killed for tables that fill past its limit. \
             The engine may fail to start. {remedy}",
            render_bytes(mapped),
            render_bytes(ceiling),
        )),
        TableMappingCharge::Committed
        | TableMappingCharge::Heuristic
        | TableMappingCharge::Free => None,
    }
}

/// What to say when the heap ceiling is past wasmtime's memory reservation,
/// if it is.
///
/// wasmtime refuses to build a pool whose `max_memory_size` exceeds its
/// `memory_reservation`, and the engine leaves the reservation at wasmtime's
/// default (see the module docs), so the engine will not start. The error
/// wasmtime gives names neither knob.
fn pool_heap_ceiling_advisory(max_memory_size: u64) -> Option<String> {
    (max_memory_size > WASMTIME_MEMORY_RESERVATION).then(|| {
        format!(
            "default-heap-memory {} is past wasmtime's {} memory reservation, and the pooling \
             allocator refuses a heap ceiling larger than the reservation, so the engine will \
             fail to start. Set default-heap-memory to {} or less.",
            render_bytes(max_memory_size),
            render_bytes(WASMTIME_MEMORY_RESERVATION),
            render_bytes(WASMTIME_MEMORY_RESERVATION),
        )
    })
}

/// What to say when the pool would use up the kernel's per-process limit
/// on memory mappings, if anything.
///
/// Each pooled stack's guard page splits the stack pool into two mappings when
/// the engine is built. Each live linear memory splits its reservation into
/// about four: the copy-on-write image, the rest of its accessible pages
/// (measured as two mappings), and the inaccessible remainder. The core
/// instance, component instance and table totals cost none: the first two are
/// counters, and the table pool is one mapping that stays whole. Memories and
/// stacks are independent pool totals, so the estimate reads each installed
/// count rather than the knob they were derived from.
fn pool_map_count_advisory(totals: &PoolTotals, max_map_count: u64) -> Option<String> {
    let PoolTotals {
        memories, stacks, ..
    } = *totals;
    let needed = u64::from(memories) * 4 + u64::from(stacks) * 2;
    (needed > max_map_count).then(|| {
        format!(
            "a pool of {memories} memories and {stacks} stacks needs about {needed} memory \
             mappings once every memory is live, past this kernel's vm.max_map_count of \
             {max_map_count}. Engine startup or later instantiation may fail. Raise \
             vm.max_map_count (sysctl), reduce the installed WASMTIME_POOLING_TOTAL_MEMORIES \
             and WASMTIME_POOLING_TOTAL_STACKS overrides, or lower max-memories when no \
             overrides are set. Aim for fewer than {} pool mappings to leave room for \
             libraries, thread stacks and compiled code.",
            max_map_count / 10 * 9,
        )
    })
}

/// What to say when the pool's reservation crowds or overflows
/// `address_space`, the most this process can map. Taken as an argument so
/// the threshold can be tested apart from the machine running it.
fn pool_reservation_advisory(
    memories: u32,
    max_memory_size: u64,
    address_space: u64,
) -> Option<String> {
    // Reserving address space is not consuming memory: on 64-bit the kernel
    // backs it only when written to. So a reservation past the budget is a
    // warning about over-commitment, not an error — but the pool maps its
    // whole slab when the engine is built, so past what this process can
    // map, the engine does not start.
    // A ceiling past the reservation is a pool wasmtime will not build at
    // all, which `pool_heap_ceiling_advisory` says; how much it would have
    // reserved is beside the point.
    if max_memory_size > WASMTIME_MEMORY_RESERVATION {
        return None;
    }
    let reservation = pool_slab_bytes(max_memory_size, memories);
    if reservation <= address_space / 100 * POOL_ADDRESS_SPACE_PERCENT {
        return None;
    }
    // The heap ceiling does not size the slot, so advising an operator to
    // lower it would change nothing.
    let remedy = format!(
        "Lower max-memories (or WASMTIME_POOLING_TOTAL_MEMORIES). Lowering \
         default-heap-memory will not help: every slot reserves wasmtime's {} plus a {} \
         guard however small its heap.",
        render_bytes(WASMTIME_MEMORY_RESERVATION),
        render_bytes(WASMTIME_MEMORY_GUARD),
    );
    let consequence = if reservation > address_space {
        "The pool cannot be mapped, so the engine will fail to start."
    } else {
        "That leaves little for everything else the process maps, so the engine may fail \
         to start, or later mappings may."
    };
    Some(format!(
        "default-heap-memory {} across {memories} pool memories (max-memories, or \
         WASMTIME_POOLING_TOTAL_MEMORIES) would reserve {} of address space ({} per slot), \
         past {POOL_ADDRESS_SPACE_PERCENT}% of the {} this process can map. {consequence} \
         {remedy}",
        render_bytes(max_memory_size),
        render_bytes(reservation),
        render_bytes(pool_slot_bytes(max_memory_size)),
        render_bytes(address_space),
    ))
}

/// Share of the real memory limit an *enforced* guest budget may claim before
/// the host says the number leaves it no room to be a host.
///
/// Above the derived 75%, so a host that named no budget never trips it, and
/// below 100%, which is the value a budget set to the pod's own
/// `limits.memory` produces.
const MAX_ENFORCED_SHARE_PERCENT: u64 = 90;

/// Address space a process can map with no `RLIMIT_AS`: the lower half of a
/// 48-bit virtual address space, which is what Linux on x86_64 and aarch64
/// (4-level page tables) and macOS give user space.
///
/// Not the 4 TiB that `is_pooling_allocator_supported` probes. That probe asks
/// whether the pool is worth using at all; a host passes it and can then map
/// a pool many times larger.
#[cfg(target_pointer_width = "64")]
const USER_ADDRESS_SPACE: u64 = 128 * 1024 * 1024 * MIB;
#[cfg(not(target_pointer_width = "64"))]
const USER_ADDRESS_SPACE: u64 = 2 * 1024 * MIB;

/// Share of the usable address space the pool may reserve before the host
/// says so. The rest is for everything else the process maps — heap, thread
/// stacks, compiled code, libraries — small beside the pool, but not nothing.
const POOL_ADDRESS_SPACE_PERCENT: u64 = 90;

/// The most address space this process can map: [`USER_ADDRESS_SPACE`],
/// lowered by `RLIMIT_AS` where one is set.
///
/// The limit is read on Linux only, from `/proc/self/limits`, which needs no
/// FFI. Elsewhere it is rarely set, and a host that sets one is told by the
/// engine failing to build.
pub fn usable_address_space() -> u64 {
    address_space_limit().map_or(USER_ADDRESS_SPACE, |limit| limit.min(USER_ADDRESS_SPACE))
}

#[cfg(target_os = "linux")]
fn address_space_limit() -> Option<u64> {
    let limits = std::fs::read_to_string("/proc/self/limits").ok()?;
    soft_address_space_limit(&limits)
}

#[cfg(not(target_os = "linux"))]
fn address_space_limit() -> Option<u64> {
    None
}

/// The soft `Max address space` out of `/proc/self/limits`, `None` when it is
/// `unlimited`.
#[cfg(any(target_os = "linux", test))]
fn soft_address_space_limit(limits: &str) -> Option<u64> {
    limits
        .lines()
        .find_map(|line| line.strip_prefix("Max address space"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// The kernel's per-process limit on memory mappings, where there is one to
/// read.
#[cfg(target_os = "linux")]
fn max_map_count() -> Option<u64> {
    std::fs::read_to_string("/proc/sys/vm/max_map_count")
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(not(target_os = "linux"))]
fn max_map_count() -> Option<u64> {
    None
}

/// The memory limit this process is really subject to.
///
/// The smallest readable limit for this process's cgroup and its ancestors,
/// capped at physical RAM. Membership comes from `/proc/self/cgroup`, using
/// the conventional v2 `/sys/fs/cgroup` and v1 `/sys/fs/cgroup/memory` mounts.
/// Mount-root limits are checked even when membership cannot be resolved.
///
/// A cgroup limit is never taken above physical memory: cgroup v1 spells
/// "unlimited" as `PAGE_COUNTER_MAX * PAGE_SIZE`, a number just under
/// `i64::MAX`, and a limit past RAM can't be reached before the machine runs out.
pub fn detected_memory_limit() -> Option<u64> {
    detected_memory_limit_with(system_total_memory(), |path| {
        std::fs::read_to_string(path).ok()
    })
}

fn detected_memory_limit_with(
    physical: Option<u64>,
    read: impl Fn(&Path) -> Option<String>,
) -> Option<u64> {
    let membership = read(Path::new("/proc/self/cgroup")).unwrap_or_default();
    let mut paths = BTreeSet::from([
        PathBuf::from("/sys/fs/cgroup/memory.max"),
        PathBuf::from("/sys/fs/cgroup/memory/memory.limit_in_bytes"),
    ]);
    for line in membership.lines() {
        let mut parts = line.splitn(3, ':');
        let (Some(_), Some(controllers), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let (root, file) = if controllers.is_empty() {
            (Path::new("/sys/fs/cgroup"), "memory.max")
        } else if controllers.split(',').any(|c| c == "memory") {
            (Path::new("/sys/fs/cgroup/memory"), "memory.limit_in_bytes")
        } else {
            continue;
        };
        let Some(relative) = path.strip_prefix('/') else {
            continue;
        };
        let relative = Path::new(relative);
        // Membership outside the visible cgroup namespace can contain '..'.
        // Keep the mount-root fallback, but never walk outside that mount.
        if relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            continue;
        }
        let mut directory = root.join(relative);
        while directory.starts_with(root) {
            paths.insert(directory.join(file));
            if directory == root || !directory.pop() {
                break;
            }
        }
    }

    let cgroup = paths
        .iter()
        .filter_map(|path| read(path))
        .filter_map(|raw| parse_cgroup_limit(&raw))
        .min();
    effective_memory_limit(cgroup, physical)
}

/// A cgroup memory limit file's value, or `None` when it sets no limit.
fn parse_cgroup_limit(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    if raw == "max" {
        return None;
    }
    let value: u64 = raw.parse().ok()?;
    // cgroup v1 spells "unlimited" as a very large number rather than a word;
    // anything this large is treated as absent rather than believed.
    // [`effective_memory_limit`] also caps what gets through at physical RAM.
    (value < u64::MAX / 2).then_some(value)
}

/// The smaller of a cgroup limit and physical memory, whichever are known.
fn effective_memory_limit(cgroup: Option<u64>, physical: Option<u64>) -> Option<u64> {
    cgroup.into_iter().chain(physical).min()
}

/// Total physical memory, capping cgroup limits and providing the fallback
/// where no cgroup limit is readable.
///
/// Swap is left out on purpose. Counting it would let the derived limits wait
/// until the machine is already swapping hard before refusing anything, and
/// on macOS the swap total grows and shrinks with load. An operator who wants
/// the host to use swap sets `max_process_memory` and `max_guest_memory`
/// explicitly.
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

/// Share of the detected memory limit the whole process may reach before it
/// refuses guest growth. Above the guest share, because the process holds the
/// guests and everything else; short of the whole, because past it the kernel
/// is already deciding for the host — by OOM kill in a container, by swapping
/// the machine to a halt on a laptop.
const PROCESS_MEMORY_PERCENT: u64 = 90;

/// `max_process_memory` when an operator names none: [`PROCESS_MEMORY_PERCENT`]
/// of the cgroup limit, or of physical memory where there is no cgroup.
/// `u64::MAX`, never enforced, where neither can be read.
pub fn derive_max_process_memory_bytes() -> u64 {
    derive_max_process_memory_bytes_within(detected_memory_limit())
}

/// [`derive_max_process_memory_bytes`] from a limit the caller already detected.
pub(crate) fn derive_max_process_memory_bytes_within(memory_limit: Option<u64>) -> u64 {
    memory_limit.map_or(u64::MAX, |limit| limit / 100 * PROCESS_MEMORY_PERCENT)
}

/// What to say about `max_process_memory` at startup, if anything: a limit
/// past the memory this process can actually have is one the kernel enforces
/// first, and on macOS that means the whole machine stalling.
pub fn process_memory_advisory(max_process_memory_bytes: u64) -> Option<String> {
    process_memory_advisory_within(max_process_memory_bytes, detected_memory_limit())
}

/// [`process_memory_advisory`] against a limit the caller already detected.
pub(crate) fn process_memory_advisory_within(
    max_process_memory_bytes: u64,
    memory_limit: Option<u64>,
) -> Option<String> {
    let limit = memory_limit?;
    (max_process_memory_bytes > limit).then(|| {
        format!(
            "max-process-memory {} is more than the {} this process can actually have, so it \
             will never refuse anything; the kernel will get there first. Set it below {}",
            render_bytes(max_process_memory_bytes),
            render_bytes(limit),
            render_bytes(limit),
        )
    })
}

/// The guest memory budget when an operator names none.
///
/// A share of the limit that will actually kill the process, clamped. Mirrors
/// [`crate::host::quota::default_max_connections`], which takes the same shape
/// of a share of `RLIMIT_NOFILE`: an unset flag means the derived ceiling, never
/// "unbounded".
pub fn derive_max_guest_memory() -> u64 {
    derive_max_guest_memory_within(detected_memory_limit())
}

/// [`derive_max_guest_memory`] from a limit the caller already detected.
pub(crate) fn derive_max_guest_memory_within(memory_limit: Option<u64>) -> u64 {
    let Some(limit) = memory_limit else {
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
            "max_guest_memory={} default_heap_memory={} max_memories={} core_instances={} \
             pool_reservation={}",
            render_bytes(self.max_guest_memory),
            render_bytes(self.default_heap_memory),
            self.max_memories,
            self.core_instances,
            render_bytes(self.pool_reservation()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pool a host with these knobs and no environment overrides installs:
    /// one memory per core instance.
    fn reservation_advisory(budgets: &HostMemoryBudgets, address_space: u64) -> Option<String> {
        pool_reservation_advisory(
            budgets.max_memories,
            budgets.default_heap_memory,
            address_space,
        )
    }

    fn map_count_advisory(budgets: &HostMemoryBudgets, max_map_count: u64) -> Option<String> {
        let mut config = wasmtime::PoolingAllocationConfig::default();
        config.total_memories(budgets.max_memories);
        config.total_stacks(budgets.max_memories);
        config.total_tables(budgets.max_memories);
        pool_map_count_advisory(&PoolTotals::of(&config), max_map_count)
    }

    #[test]
    fn the_table_pool_is_sized_by_the_host_page_each_table_rounds_up_to() {
        // wasmtime's default of 20,000 elements is 160,000 bytes of pointers
        // on a 64-bit host, which the pool rounds up to the next host page:
        // 163,840 on 4 KiB and 16 KiB pages alike.
        let page = table_pool_page();
        assert!(page >= 4096 && page.is_power_of_two(), "{page}");
        let one = pool_table_bytes(1, 20_000);
        assert!(one >= 160_000 && one < 160_000 + page, "{one}");
        assert_eq!(one % page, 0);
        assert_eq!(pool_table_bytes(4000, 20_000), 4000 * one);
        // One element still takes a whole page; nothing never overflows.
        assert_eq!(pool_table_bytes(1, 1), page);
        assert_eq!(pool_table_bytes(0, 20_000), 0);
        assert_eq!(pool_table_bytes(u32::MAX, usize::MAX), u64::MAX);
    }

    #[test]
    fn the_table_commit_advisory_names_the_count_that_grew_it() {
        use TableMappingCharge::{Committed, Free, Heuristic};
        // The smaller of the kernel's figure and the process's limit.
        let limit = Some(8 * 1024 * MIB);
        // Where mappings are committed, the stock host's 4000 tables commit
        // 625 MiB: within a tenth of 8 GiB, so nothing is said; a host told
        // --max-memories 10000 commits 6.25 GiB, which an 8 GiB box must
        // hear about.
        assert!(pool_table_commit_advisory(4000, 20_000, limit, Committed).is_none());
        let advisory = pool_table_commit_advisory(40_000, 20_000, limit, Committed)
            .expect("6.25 GiB of tables is most of an 8 GiB limit");
        assert!(advisory.contains("40000 pooled tables"), "{advisory}");
        assert!(advisory.contains("max-memories"), "{advisory}");
        assert!(
            advisory.contains("WASMTIME_POOLING_TOTAL_TABLES"),
            "{advisory}"
        );
        // Under Linux's heuristic a mapping costs nothing until it is larger
        // than memory itself: 6.25 GiB passes on 8 GiB, 18.75 GiB does not.
        assert!(pool_table_commit_advisory(40_000, 20_000, limit, Heuristic).is_none());
        let advisory = pool_table_commit_advisory(120_000, 20_000, limit, Heuristic)
            .expect("one 18.75 GiB mapping is refused on an 8 GiB box");
        assert!(advisory.contains("overcommit_memory=0"), "{advisory}");
        assert!(advisory.contains("max-memories"), "{advisory}");
        // Where it is free there is nothing to say, however large.
        assert!(pool_table_commit_advisory(120_000, 20_000, limit, Free).is_none());
        // No limit to compare against, nothing to say.
        assert!(pool_table_commit_advisory(120_000, 20_000, None, Committed).is_none());
    }

    #[test]
    fn sizes_parse_in_binary_units() {
        assert_eq!(parse_bytes("128MiB"), Ok(128 * MIB));
        assert_eq!(parse_bytes("4GiB"), Ok(4 * 1024 * MIB));
        assert_eq!(parse_bytes("512"), Ok(512));
        assert_eq!(parse_bytes(" 256 MiB "), Ok(256 * MIB));
        // Kubernetes quantities, as an operator writes them next to
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

    /// cgroup v1's "unlimited" is just under `u64::MAX / 2`, so the parse
    /// lets it through; the limit must still fall back to physical memory.
    #[test]
    fn an_unlimited_cgroup_v1_limit_falls_back_to_physical_memory() {
        const CGROUP_V1_UNLIMITED: &str = "9223372036854771712\n";
        let physical = 16 * 1024 * MIB;

        let cgroup = parse_cgroup_limit(CGROUP_V1_UNLIMITED);
        assert_eq!(
            effective_memory_limit(cgroup, Some(physical)),
            Some(physical)
        );

        assert_eq!(parse_cgroup_limit("max\n"), None);
        assert_eq!(effective_memory_limit(None, Some(physical)), Some(physical));

        let container = 512 * MIB;
        assert_eq!(parse_cgroup_limit("536870912\n"), Some(container));
        assert_eq!(
            effective_memory_limit(Some(container), Some(physical)),
            Some(container)
        );
        assert_eq!(
            effective_memory_limit(Some(container), None),
            Some(container)
        );
        assert_eq!(effective_memory_limit(None, None), None);
    }

    /// Fake Linux files use `/` even when these tests run on Windows.
    fn cgroup_fixture_path(path: &Path) -> Option<String> {
        Some(path.to_str()?.replace('\\', "/"))
    }

    #[test]
    fn automatic_process_limit_honors_service_and_parent_cgroups() {
        // These are complete detector inputs, including the unbounded mount
        // root that previously hid every service/parent limit below it.
        for (parent, expected, process_limit) in [
            (None, 1024 * MIB, 966_367_620),
            (Some("536870912"), 512 * MIB, 483_183_810),
        ] {
            let read = |path: &Path| match cgroup_fixture_path(path)?.as_str() {
                "/proc/self/cgroup" => Some("0::/system.slice/wash.service\n".into()),
                "/sys/fs/cgroup/memory.max" => Some("max\n".into()),
                "/sys/fs/cgroup/system.slice/memory.max" => parent.map(str::to_owned),
                "/sys/fs/cgroup/system.slice/wash.service/memory.max" => {
                    Some("1073741824\n".into())
                }
                _ => None,
            };
            let limit = detected_memory_limit_with(Some(16 * 1024 * MIB), read);
            assert_eq!(limit, Some(expected));
            assert_eq!(derive_max_process_memory_bytes_within(limit), process_limit);
            assert_eq!(
                detected_memory_limit_with(None, read),
                Some(expected),
                "readable cgroup limits still work without physical RAM"
            );
            assert_eq!(
                detected_memory_limit_with(Some(256 * MIB), read),
                Some(256 * MIB),
                "cgroup limits never override a smaller physical limit"
            );
        }
    }

    #[test]
    fn legacy_memory_membership_checks_ancestors_and_ignores_other_controllers() {
        let read = |path: &Path| match cgroup_fixture_path(path)?.as_str() {
            "/proc/self/cgroup" => Some("3:cpu,cpuacct:/unrelated\n4:memory:/tenant/wash\n".into()),
            "/sys/fs/cgroup/memory/memory.limit_in_bytes" => Some("9223372036854771712\n".into()),
            "/sys/fs/cgroup/memory/tenant/memory.limit_in_bytes" => Some("536870912".into()),
            "/sys/fs/cgroup/memory/tenant/wash/memory.limit_in_bytes" => Some("1073741824".into()),
            "/sys/fs/cgroup/memory/unrelated/memory.limit_in_bytes" => {
                panic!("a non-memory controller must not supply a memory limit")
            }
            _ => None,
        };
        let limit = detected_memory_limit_with(Some(16 * 1024 * MIB), read);
        assert_eq!(limit, Some(512 * MIB));
        assert_eq!(derive_max_process_memory_bytes_within(limit), 483_183_810);
    }

    #[test]
    fn namespace_root_and_unresolvable_membership_keep_mount_root_limits() {
        for membership in [
            "0::/\n",
            "0::/../outside\n",
            "0::/tenant/../../outside\n",
            "0::relative\n",
            "malformed\n",
        ] {
            let read = |path: &Path| match cgroup_fixture_path(path)?.as_str() {
                "/proc/self/cgroup" => Some(membership.into()),
                "/sys/fs/cgroup/memory.max" => Some("1073741824".into()),
                "/sys/fs/cgroup/memory/memory.limit_in_bytes" => None,
                unexpected => panic!("unexpected cgroup lookup {unexpected:?} for {membership:?}"),
            };
            let limit = detected_memory_limit_with(Some(16 * 1024 * MIB), read);
            assert_eq!(limit, Some(1024 * MIB));
            assert_eq!(derive_max_process_memory_bytes_within(limit), 966_367_620);
        }
    }

    #[test]
    fn missing_membership_and_unlimited_root_still_fall_back_to_physical_ram() {
        let read = |path: &Path| match cgroup_fixture_path(path)?.as_str() {
            "/sys/fs/cgroup/memory.max" => Some("max".into()),
            "/sys/fs/cgroup/memory/memory.limit_in_bytes" => Some("9223372036854771712".into()),
            _ => None,
        };
        let limit = detected_memory_limit_with(Some(16 * 1024 * MIB), read);
        assert_eq!(limit, Some(16 * 1024 * MIB));
        assert_eq!(
            derive_max_process_memory_bytes_within(limit),
            15_461_882_190
        );
        assert_eq!(detected_memory_limit_with(None, |_| None), None);
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
        let resolved =
            HostMemoryBudgets::resolve(None, None, None, None).expect("defaults are valid");
        assert_eq!(resolved.default_heap_memory, WASMTIME_DEFAULT_HEAP_MEMORY);
        assert_eq!(resolved.max_memories, WASMTIME_DEFAULT_MEMORIES);
        assert_eq!(
            resolved.core_instances,
            derived_core_instances(WASMTIME_DEFAULT_MEMORIES)
        );
        assert!(resolved.core_instance_advisory().is_none());
        assert!(
            resolved.max_guest_memory >= MIN_DERIVED_MAX_GUEST_MEMORY,
            "an unset budget is derived, never zero or unbounded"
        );
    }

    #[test]
    fn string_knobs_resolve_the_same_as_parsed_ones() {
        // The string form is `resolve` plus a parse, nothing more.
        assert_eq!(
            HostMemoryBudgets::resolve_strs(Some("8GiB"), Some("128MiB"), Some(100), None),
            HostMemoryBudgets::resolve(Some(8 * 1024 * MIB), Some(128 * MIB), Some(100), None)
        );
        // `2G` and `2Gi` are different numbers and stay different here.
        let decimal = HostMemoryBudgets::resolve_strs(Some("2G"), None, None, None).unwrap();
        let binary = HostMemoryBudgets::resolve_strs(Some("2Gi"), None, None, None).unwrap();
        assert_eq!(decimal.max_guest_memory, 2_000_000_000);
        assert_eq!(binary.max_guest_memory, 2 * 1024 * MIB);
        // The zero checks apply to a parsed size too, not just a passed one.
        assert!(HostMemoryBudgets::resolve_strs(Some("0"), None, None, None).is_err());
    }

    #[test]
    fn a_blank_knob_is_unset_rather_than_a_parse_failure() {
        // An empty ConfigMap key or `value: ""` reaches clap as `Some("")`.
        let blank = HostMemoryBudgets::resolve_strs(Some(""), Some("   "), None, None)
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
        let err = HostMemoryBudgets::resolve_strs(None, Some("512 gigabytes"), None, None)
            .expect_err("an unparseable size is refused");
        assert!(
            err.contains("default-heap-memory"),
            "the error must name the knob: {err}"
        );

        let err = HostMemoryBudgets::resolve_strs(Some("lots"), None, None, None)
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
        assert!(HostMemoryBudgets::resolve(Some(0), None, None, None).is_err());
        assert!(HostMemoryBudgets::resolve(None, Some(0), None, None).is_err());
        assert!(HostMemoryBudgets::resolve(None, None, Some(0), None).is_err());
        assert!(HostMemoryBudgets::resolve(None, None, None, Some(0)).is_err());
    }

    #[test]
    fn memories_and_core_instances_derive_from_each_other_unless_both_are_set() {
        let per = super::super::CORE_INSTANCES_PER_MEMORY;
        // Memories alone take the ratio's core instances per memory.
        let memories = HostMemoryBudgets::resolve(None, None, Some(100), None).unwrap();
        assert_eq!(memories.max_memories, 100);
        assert_eq!(memories.core_instances, 100 * per);
        assert!(memories.core_instance_advisory().is_none());
        // Core instances alone take the memories they fit in, rounded up and
        // never zero: the flag keeps meaning core instances, and the pool is
        // sized to run that many of the usual shape.
        let cores = HostMemoryBudgets::resolve(None, None, None, Some(100 * per + 1)).unwrap();
        assert_eq!(cores.core_instances, 100 * per + 1);
        assert_eq!(cores.max_memories, 101);
        assert!(cores.core_instance_advisory().is_none());
        assert_eq!(
            HostMemoryBudgets::resolve(None, None, None, Some(1))
                .unwrap()
                .max_memories,
            1
        );
        // Both set: both stand, and a mismatch is said, with both numbers.
        let both = HostMemoryBudgets::resolve(None, None, Some(100), Some(100 * per)).unwrap();
        assert!(both.core_instance_advisory().is_none());
        let apart = HostMemoryBudgets::resolve(None, None, Some(100), Some(500)).unwrap();
        assert_eq!((apart.max_memories, apart.core_instances), (100, 500));
        let advisory = apart
            .core_instance_advisory()
            .expect("500 is not 100 memories' worth");
        assert!(advisory.contains("core-instances 500"), "{advisory}");
        assert!(advisory.contains("max-memories 100"), "{advisory}");
        assert!(
            advisory.contains(&format!("derive {} core instances", 100 * per)),
            "{advisory}"
        );
    }

    #[test]
    fn a_slot_is_never_smaller_than_the_reservation() {
        // A 128 MiB heap ceiling still reserves 4 GiB per slot: multiplying
        // the ceiling by the count would report 12.5 GiB for what is 403 GiB.
        let resolved =
            HostMemoryBudgets::resolve(Some(8 * 1024 * MIB), Some(128 * MIB), Some(100), None)
                .unwrap();
        assert_eq!(pool_slot_bytes(128 * MIB), 4096 * MIB + 32 * MIB);
        assert_eq!(
            resolved.pool_reservation(),
            100 * (4096 * MIB + 32 * MIB) + 32 * MIB
        );
        assert_eq!(resolved.advisory(), None, "a modest pool needs no advisory");
    }

    #[test]
    fn a_heap_ceiling_past_the_reservation_is_named_as_the_cause() {
        // wasmtime refuses this pool outright, so the advice is about the
        // ceiling, not about how much the pool would have reserved.
        let resolved =
            HostMemoryBudgets::resolve(None, Some(16 * 1024 * MIB), Some(10_000), None).unwrap();
        let advisory = pool_heap_ceiling_advisory(resolved.default_heap_memory)
            .expect("16 GiB is past the 4 GiB reservation");
        assert!(advisory.contains("default-heap-memory 16GiB"), "{advisory}");
        assert!(advisory.contains("4GiB or less"), "{advisory}");
        assert_eq!(reservation_advisory(&resolved, USER_ADDRESS_SPACE), None);
        assert_eq!(pool_heap_ceiling_advisory(4 * 1024 * MIB), None);
        assert_eq!(pool_heap_ceiling_advisory(128 * MIB), None);
    }

    #[test]
    fn a_small_heap_over_the_address_space_is_told_to_lower_the_count() {
        // The trap this is here for: lowering the heap ceiling to fit more
        // slots, and finding the reservation unchanged.
        let resolved =
            HostMemoryBudgets::resolve(None, Some(64 * MIB), Some(30_000), None).unwrap();
        let advisory = reservation_advisory(&resolved, USER_ADDRESS_SPACE)
            .expect("30,000 slots of 4 GiB are 118.6 TiB, past 90% of 128 TiB");
        assert!(advisory.contains("may fail"), "{advisory}");
        assert!(advisory.contains("Lower max-memories"), "{advisory}");
        assert!(
            advisory.contains("Lowering default-heap-memory will not help"),
            "{advisory}"
        );
        assert!(advisory.contains("per slot"), "{advisory}");
    }

    #[test]
    fn the_stock_defaults_are_named_and_quiet() {
        // Worth pinning, because it is the number nobody is told today: 4 GiB
        // and a 32 MiB guard per slot across 1000 slots is 3.9 TiB.
        let stock = HostMemoryBudgets::resolve(None, None, None, None).unwrap();
        assert_eq!(
            stock.pool_reservation(),
            1000 * (4096 * MIB + 32 * MIB) + 32 * MIB
        );
        assert_eq!(render_bytes(stock.pool_reservation()), "3.9TiB");
        assert_eq!(
            reservation_advisory(&stock, USER_ADDRESS_SPACE),
            None,
            "the stock configuration must not warn — it is what every host runs today"
        );
    }

    #[test]
    fn a_pool_well_past_the_old_probe_is_quiet_while_it_fits() {
        // 20,000 slots is 78.8 TiB: far past the 4 TiB the support probe maps,
        // and comfortably inside what the process can.
        let resolved = HostMemoryBudgets::resolve(None, None, Some(20_000), None).unwrap();
        assert_eq!(reservation_advisory(&resolved, USER_ADDRESS_SPACE), None);
        // Under an address-space limit of 64 TiB, the same pool cannot map.
        let advisory = reservation_advisory(&resolved, 64 * 1024 * 1024 * MIB)
            .expect("78.8 TiB does not fit in 64 TiB");
        assert!(advisory.contains("cannot be mapped"), "{advisory}");
    }

    #[test]
    fn a_pool_past_the_kernels_mapping_limit_is_called_out() {
        // 20,000 slots fit the address space but not Linux's default 65530
        // mappings: 20,000 stack guards split 40,000 at startup and 20,000 live
        // memories another 80,000.
        let resolved = HostMemoryBudgets::resolve(None, None, Some(20_000), None).unwrap();
        let advisory =
            map_count_advisory(&resolved, 65_530).expect("120,000 mappings are past 65530");
        assert!(advisory.contains("vm.max_map_count"), "{advisory}");
        assert!(
            advisory.contains("58977"),
            "names a mapping target with headroom: {advisory}"
        );
        assert_eq!(map_count_advisory(&resolved, 1_048_576), None);
        let stock = HostMemoryBudgets::resolve(None, None, None, None).unwrap();
        assert_eq!(map_count_advisory(&stock, 65_530), None);
    }

    #[test]
    fn mapping_advisories_use_independent_installed_pool_totals() {
        let mut config = wasmtime::PoolingAllocationConfig::default();
        config.total_memories(1000);
        config.total_stacks(40_000);
        config.total_tables(1000);
        let advisory = pool_map_count_advisory(&PoolTotals::of(&config), 65_530)
            .expect("stack guard mappings alone exceed the kernel limit");
        assert!(advisory.contains("84000 memory mappings"), "{advisory}");
        assert!(advisory.contains("40000 stacks"), "{advisory}");
        assert!(advisory.contains("Engine startup"), "{advisory}");
        assert!(
            advisory.contains("WASMTIME_POOLING_TOTAL_STACKS"),
            "{advisory}"
        );

        config.total_stacks(1000);
        assert!(pool_map_count_advisory(&PoolTotals::of(&config), 65_530).is_none());
        // Tables are one mapping however many there are.
        config.total_tables(70_000);
        assert!(pool_map_count_advisory(&PoolTotals::of(&config), 65_530).is_none());
        config.total_memories(20_000);
        let advisory = pool_map_count_advisory(&PoolTotals::of(&config), 65_530)
            .expect("the memory count uses its own installed total");
        assert!(advisory.contains("82000 memory mappings"), "{advisory}");
        assert!(
            advisory.contains("WASMTIME_POOLING_TOTAL_MEMORIES"),
            "{advisory}"
        );
    }

    #[test]
    fn the_address_space_limit_is_read_from_proc_limits() {
        let unlimited = "Limit                     Soft Limit           Hard Limit           Units\n\
                         Max address space         unlimited            unlimited            bytes\n";
        assert_eq!(soft_address_space_limit(unlimited), None);
        let limited = "Max address space         8589934592           unlimited            bytes\n";
        assert_eq!(soft_address_space_limit(limited), Some(8 * 1024 * MIB));
        assert!(usable_address_space() <= USER_ADDRESS_SPACE);
    }

    #[test]
    fn an_unreservable_combination_is_reported_with_both_numbers() {
        let resolved = HostMemoryBudgets::resolve(None, None, Some(35_000), None).unwrap();
        let advisory = reservation_advisory(&resolved, USER_ADDRESS_SPACE)
            .expect("137.8 TiB is past the 128 TiB a process can map");
        assert!(advisory.contains("cannot be mapped"), "{advisory}");
        assert!(
            advisory.contains("137.8TiB"),
            "names the reservation, to one decimal because it is not a round unit: {advisory}"
        );
        assert!(
            advisory.contains("35000"),
            "names the instance count: {advisory}"
        );
        assert!(
            advisory.contains("default-heap-memory") && advisory.contains("max-memories"),
            "names both knobs so it is actionable: {advisory}"
        );
        assert!(advisory.contains("Lower max-memories"), "{advisory}");
    }

    /// A budget set to the pod's own `resources.limits.memory` is a ceiling
    /// equal to 100% of the pod — and the host's own overhead, which this
    /// budget does not charge, then OOM-kills the pod before the budget
    /// refuses anything.
    #[test]
    fn enforcing_a_budget_that_leaves_the_host_no_room_is_called_out() {
        let limit = detected_memory_limit().expect("a test machine has a readable memory limit");

        // The whole limit.
        let whole = HostMemoryBudgets::resolve(Some(limit), None, None, None).unwrap();
        let advisory = whole
            .enforcement_advisory()
            .expect("a budget equal to the real limit leaves the host nothing");
        assert!(
            advisory.contains("max-guest-memory") && advisory.contains("OOM"),
            "the advisory must name the knob and the consequence: {advisory}"
        );

        // Half of it: room to spare, nothing to say.
        let modest = HostMemoryBudgets::resolve(Some(limit / 2), None, None, None).unwrap();
        assert_eq!(modest.enforcement_advisory(), None);
    }

    /// The derived budget is three quarters of the limit, so a host that named
    /// no budget must never trip the advisory — otherwise every host would be
    /// warned, by default, about a number it did not choose.
    #[test]
    fn a_derived_budget_is_never_warned_about() {
        let derived = HostMemoryBudgets::resolve(None, None, None, None).unwrap();
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
            HostMemoryBudgets::resolve(Some(512 * MIB), Some(2 * 1024 * MIB), Some(4), None)
                .unwrap();
        let advisory = resolved.advisory().expect("a guest can exhaust the host");
        assert!(advisory.contains("2GiB") && advisory.contains("512MiB"));
    }

    /// The knobs alone say nothing about a reservation: whether there is a
    /// pool, and how many memories it holds, is only known once it is built.
    #[test]
    fn the_knobs_advisory_leaves_the_reservation_to_the_installed_pool() {
        let resolved = HostMemoryBudgets::resolve(None, None, Some(30_000), None).unwrap();
        assert_eq!(resolved.advisory(), None);
        assert!(reservation_advisory(&resolved, USER_ADDRESS_SPACE).is_some());
        assert!(map_count_advisory(&resolved, 65_530).is_some());
    }

    #[test]
    fn the_derived_process_limit_fits_the_machine() {
        let limit = detected_memory_limit().expect("a test machine has a readable memory limit");
        let derived = derive_max_process_memory_bytes();
        assert!(derived < limit, "{derived} must leave room under {limit}");
        assert_eq!(process_memory_advisory(derived), None);
    }

    #[test]
    fn a_process_limit_past_the_machine_is_called_out() {
        let limit = detected_memory_limit().expect("a test machine has a readable memory limit");
        let advisory = process_memory_advisory(limit.saturating_mul(2))
            .expect("twice the machine can never be reached");
        assert!(advisory.contains(&render_bytes(limit)), "{advisory}");
    }
}
