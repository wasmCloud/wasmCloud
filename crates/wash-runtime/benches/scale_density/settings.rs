//! Settings for the density experiment.
use anyhow::{Context as _, Result, bail};
use std::{
    collections::HashMap,
    hash::{Hash as _, Hasher as _},
    path::Path,
    time::Duration,
};
use wash_runtime::{
    engine::{
        guest_memory::GuestMemoryMode,
        host_memory::{self, parse_bytes},
    },
    wit::WitInterface,
};

pub(super) use bench_tools::density::Flavour;

pub(super) fn parse_flavour(name: &str) -> Result<Flavour> {
    Flavour::ALL
        .into_iter()
        .find(|f| f.name() == name)
        .with_context(|| format!("unknown flavour {name}"))
}

/// The guest memory mode, or a comma-separated list of modes to run in turn.
pub(super) const GUEST_MEMORY_MODE: &str = "WASH_SCALE_GUEST_MEMORY_MODE";

/// What a run was told, from the environment.
#[derive(Debug, Clone)]
pub(super) struct Settings {
    pub(super) report_json: Option<String>,
    pub(super) memory_limit: u64,
    /// Unset, the effective machine/container budget, so the process limit binds first.
    pub(super) guest_memory: u64,
    /// How the guest budget is applied; `enforce` charges declared initial memory.
    /// A run's mode: the first of `guest_modes`, and the only one a child sees.
    pub(super) guest_mode: GuestMemoryMode,
    /// One run per entry, each in a child told only its own mode.
    pub(super) guest_modes: Vec<GuestMemoryMode>,
    /// Under `growth`, the cap on uncharged initial memory.
    /// Unset, the engine's own default, read back from the engine once it is built.
    pub(super) initial_guest_memory: Option<u64>,
    pub(super) max_components: Option<usize>,
    pub(super) max_duration: Option<Duration>,
    pub(super) step: usize,
    /// Unset, as many as the host has cores.
    pub(super) start_concurrency: Option<usize>,
    pub(super) sample: usize,
    /// Linear memories the pool holds; the count that sizes the pool.
    pub(super) max_memories: u32,
    pub(super) fixture: &'static str,
    /// One run per entry; `None` is no limit.
    pub(super) cpus: Vec<Option<usize>>,
}

impl Settings {
    pub(super) fn validate_flavour(&self, flavour: Flavour) -> Result<()> {
        if flavour == Flavour::Warm && self.fixture != "http_handler_p3" {
            bail!(
                "warm runs require the P3 HTTP fixture; use WASH_SCALE_FIXTURE=p3 or run scale-to-zero"
            );
        }
        Ok(())
    }

    pub(super) fn from_env() -> Result<Self> {
        Self::from_values(|key| std::env::var(key).ok().filter(|v| !v.trim().is_empty()))
    }

    pub(super) fn from_values(var: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let number = |key: &str| -> Result<Option<usize>> {
            var(key)
                .map(|v| v.trim().parse().with_context(|| format!("{key}={v}")))
                .transpose()
        };
        let memory_limit = match var("WASH_SCALE_MEMORY_LIMIT") {
            Some(v) => parse_bytes(v.trim())
                .map_err(|e| anyhow::anyhow!("WASH_SCALE_MEMORY_LIMIT={v}: {e}"))?,
            None => (effective_memory()? / 2).min(2 << 30),
        };
        let guest_memory = match var("WASH_SCALE_GUEST_MEMORY") {
            Some(v) => parse_bytes(v.trim())
                .map_err(|e| anyhow::anyhow!("WASH_SCALE_GUEST_MEMORY={v}: {e}"))?,
            None => effective_memory()?,
        };
        // The runtime's own parser, the one `wash --guest-memory-mode` uses,
        // once per entry of a list (`enforce,growth`).
        let mut guest_modes: Vec<GuestMemoryMode> = match var(GUEST_MEMORY_MODE) {
            None => vec![GuestMemoryMode::Enforce],
            Some(list) => list
                .split(',')
                .map(|mode| {
                    mode.parse::<GuestMemoryMode>()
                        .map_err(|e| anyhow::anyhow!("{GUEST_MEMORY_MODE}={list}: {e}"))
                })
                .collect::<Result<_>>()?,
        };
        // A mode named twice would publish the same series twice.
        let mut seen = Vec::new();
        guest_modes.retain(|mode| {
            let first = !seen.contains(mode);
            seen.push(*mode);
            first
        });
        // `split` yields at least one entry, so the default is never reached.
        let guest_mode = guest_modes.first().copied().unwrap_or_default();
        let initial_guest_memory = var("WASH_SCALE_INITIAL_GUEST_MEMORY")
            .map(|v| {
                parse_bytes(v.trim())
                    .map_err(|e| anyhow::anyhow!("WASH_SCALE_INITIAL_GUEST_MEMORY={v}: {e}"))
            })
            .transpose()?;
        // `WASH_SCALE_CORE_INSTANCES` named this count before the pool's
        // totals were derived from its memory count; still read.
        let max_memories = match number("WASH_SCALE_MAX_MEMORIES")? {
            Some(n) => u32::try_from(n).context("WASH_SCALE_MAX_MEMORIES")?,
            None => match number("WASH_SCALE_CORE_INSTANCES")? {
                Some(n) => u32::try_from(n).context("WASH_SCALE_CORE_INSTANCES")?,
                None => host_memory::derive_max_memories(),
            },
        };
        let fixture = match var("WASH_SCALE_FIXTURE").as_deref().map(str::trim) {
            None | Some("p3") => "http_handler_p3",
            Some("p2") => "http_handler_p2",
            Some(other) => bail!("WASH_SCALE_FIXTURE must be p2 or p3, not {other}"),
        };
        Ok(Self {
            report_json: var("WASH_SCALE_REPORT_JSON"),
            memory_limit,
            guest_memory,
            guest_mode,
            guest_modes,
            initial_guest_memory,
            max_components: number("WASH_SCALE_MAX_COMPONENTS")?,
            max_duration: match number("WASH_SCALE_MAX_SECS")? {
                Some(0) => None,
                Some(secs) => Some(Duration::from_secs(secs as u64)),
                None => Some(Duration::from_secs(600)),
            },
            step: number("WASH_SCALE_STEP")?.unwrap_or(250).max(1),
            start_concurrency: number("WASH_SCALE_START_CONCURRENCY")?.map(|n| n.max(1)),
            sample: bench_tools::density::sample_size(number("WASH_SCALE_SAMPLE")?.unwrap_or(200))
                .map_err(anyhow::Error::msg)
                .context("WASH_SCALE_SAMPLE")?,
            max_memories,
            fixture,
            cpus: match var("WASH_SCALE_CPUS") {
                None => vec![None],
                Some(list) => list
                    .split(',')
                    .map(|n| match n.trim().parse::<usize>() {
                        Ok(0) | Err(_) => {
                            bail!("WASH_SCALE_CPUS={list}: each entry must be a core count")
                        }
                        Ok(n) => Ok(Some(n)),
                    })
                    .collect::<Result<_>>()?,
            },
        })
    }
}

pub(super) fn effective_memory() -> Result<u64> {
    host_memory::detected_memory_limit().context(
        "cannot detect a memory budget; set WASH_SCALE_MEMORY_LIMIT and WASH_SCALE_GUEST_MEMORY",
    )
}

pub(super) fn fixture_bytes(name: &str) -> Result<bytes::Bytes> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/wasm")
        .join(format!("{name}.wasm"));
    let bytes = std::fs::read(&path).with_context(|| {
        format!(
            "missing {}; build it with `cargo xtask build-fixtures {}`",
            path.display(),
            name.replace('_', "-"),
        )
    })?;
    Ok(bytes.into())
}

/// A digest for the fixture, so every workload shares one compile the way
/// workloads pulled from one image do. Without one, each start compiles the
/// component again and the bench measures cranelift, not density.
pub(super) fn fixture_digest(wasm: &[u8]) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    wasm.hash(&mut hasher);
    format!("scale-density:{:016x}", hasher.finish())
}

pub(super) fn http_interface(host_header: &str, fixture: &str) -> WitInterface {
    let (interface, version) = if fixture == "http_handler_p2" {
        ("incoming-handler", semver::Version::new(0, 2, 2))
    } else {
        ("handler", semver::Version::new(0, 3, 0))
    };
    WitInterface {
        namespace: "wasi".to_string(),
        package: "http".to_string(),
        interfaces: [interface.to_string()].into_iter().collect(),
        version: Some(version),
        config: HashMap::from([("host".to_string(), host_header.to_string())]),
        name: None,
    }
}

pub(super) fn host_header(index: usize) -> String {
    format!("w{index}.scale.test")
}
