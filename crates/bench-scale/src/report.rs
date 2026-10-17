//! The human report (`--report`) and the JSON one (`--json`).

use serde::Serialize;

use crate::bench::{Flavour, FlavourReport, Settings, Step};
use crate::size::{render, render_opt};

#[derive(Debug, Clone, Serialize)]
pub struct Machine {
    pub cores: usize,
    pub memory_total_bytes: u64,
    pub memory_budget_bytes: Option<u64>,
    pub os: String,
    pub kernel: String,
    pub arch: String,
}

impl Machine {
    pub fn detect() -> Self {
        use sysinfo::{MemoryRefreshKind, RefreshKind, System};
        let system = System::new_with_specifics(
            RefreshKind::new().with_memory(MemoryRefreshKind::new().with_ram()),
        );
        Self {
            cores: std::thread::available_parallelism().map_or(1, |n| n.get()),
            memory_total_bytes: system.total_memory(),
            memory_budget_bytes: bench_tools::memory::memory_budget(system.total_memory()),
            os: System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_string()),
            kernel: System::kernel_version().unwrap_or_default(),
            arch: std::env::consts::ARCH.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ImageInfo {
    pub reference: String,
    /// `fixture:http-handler-p2`, `wasm:<path>` or `external`.
    pub source: String,
    pub size_bytes: Option<u64>,
    pub layer_digest: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub tool: &'static str,
    pub tool_version: &'static str,
    pub started_unix_secs: u64,
    pub machine: Machine,
    pub settings: Settings,
    pub image: ImageInfo,
    #[serde(serialize_with = "serialize_url")]
    pub nats_url: String,
    pub run_dir: String,
    pub flavours: Vec<FlavourReport>,
    pub exit_code: u8,
}

/// Reports retain endpoints, never URL credentials, query tokens or fragments.
/// Unparseable values are omitted rather than risking a partial credential leak.
pub fn redact_url(value: &str) -> String {
    let normalized = if value.contains("://") {
        value.to_string()
    } else {
        format!("nats://{value}")
    };
    let Ok(mut url) = reqwest::Url::parse(&normalized) else {
        return "[redacted address]".to_string();
    };
    if !url.username().is_empty() || url.password().is_some() {
        if url.set_username("redacted").is_err() || url.set_password(None).is_err() {
            return "[redacted address]".to_string();
        }
    }
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

/// Preserve known measurement settings. Unknown arguments may carry arbitrary
/// credentials, so their values and positional arguments are omitted.
pub fn redact_args(args: &[String]) -> Vec<String> {
    let mut output = Vec::new();
    let mut args = args.iter().peekable();
    while let Some(arg) = args.next() {
        if arg == "host" {
            output.push(arg.clone());
            continue;
        }
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(k, v)| (k, Some(v)));
        let safe = matches!(
            flag,
            "--name"
                | "--http-address"
                | "--max-process-memory"
                | "--max-guest-memory"
                | "--core-instances"
                | "--default-heap-memory"
                | "--guest-memory-mode"
                | "--log-level"
                | "--log-format"
        );
        let url = matches!(flag, "--scheduler-nats-url" | "--data-nats-url");
        let value = inline.or_else(|| {
            args.peek().filter(|next| !next.starts_with('-'))?;
            args.next().map(String::as_str)
        });
        if safe || url {
            output.push(flag.to_string());
            if let Some(value) = value {
                output.push(if url {
                    redact_url(value)
                } else {
                    value.to_string()
                });
            }
        } else {
            output.push("[redacted argument]".to_string());
        }
    }
    output
}

pub fn serialize_url<S: serde::Serializer>(value: &str, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&redact_url(value))
}

pub fn serialize_args<S: serde::Serializer>(
    args: &[String],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    redact_args(args).serialize(serializer)
}

fn ms(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_string(), |v| format!("{v:.1}"))
}

pub fn print_header(
    machine: &Machine,
    settings: &Settings,
    image: &ImageInfo,
    nats_url: &str,
    run_dir: &str,
) {
    println!("bench-scale {}", env!("CARGO_PKG_VERSION"));
    println!(
        "machine:  {} cores, {} RAM, {} (kernel {}, {})",
        machine.cores,
        render(machine.memory_total_bytes),
        machine.os,
        machine.kernel,
        machine.arch
    );
    println!(
        "limits:   memory {}, core instances {}{}, cap {}, step {}, sample {}, start concurrency {}",
        render(settings.memory_limit_bytes),
        settings.core_instances,
        settings
            .core_instances_per_workload
            .map_or_else(String::new, |n| format!(" ({n}/workload)")),
        settings.max_components.map_or_else(
            || format!("theoretical x {}", settings.overshoot),
            |n| n.to_string()
        ),
        settings.step,
        settings.sample,
        settings.start_concurrency,
    );
    println!(
        "image:    {} ({}{})",
        image.reference,
        image.source,
        image
            .size_bytes
            .map_or_else(String::new, |s| format!(", {}", render(s)))
    );
    println!("nats:     {}", redact_url(nats_url));
    println!("run dir:  {run_dir}");
    println!("cpu:      not limited; the host uses whatever the machine gives it");
}

pub fn print_flavour_header(flavour: Flavour, report_host: &crate::bench::HostInfo, memory: &str) {
    println!();
    println!("== {} ==", flavour.name());
    println!(
        "host:     {} (version {}, id {}, http {})",
        report_host.binary.as_deref().unwrap_or("attached"),
        if report_host.version.is_empty() {
            "?"
        } else {
            &report_host.version
        },
        report_host.id,
        report_host.http,
    );
    if !report_host.args.is_empty() {
        println!("args:     {}", redact_args(&report_host.args).join(" "));
    }
    if !report_host.unsupported_flags.is_empty() {
        println!(
            "note:     this host takes no --{}; the bench's own footprint check enforces the limit",
            report_host.unsupported_flags.join(", --")
        );
    }
    println!("memory:   {memory}");
}

pub fn print_step(step: &Step, first: bool) {
    if first {
        println!(
            "{:>9} {:>8} {:>15} {:>12} {:>10} {:>10} {:>11} {:>11} {:>9} {:>8} {:>8} {:>8} {:>9}",
            "workloads",
            "starts/s",
            "start p50/p99",
            "ref/to/nr",
            "footprint",
            "per-wkld",
            "hb wkld/cmp",
            "warm ok/f",
            "req ok/f",
            "p50 ms",
            "p99 ms",
            "down ms",
            "limit",
        );
    }
    println!(
        "{:>9} {:>8.0} {:>15} {:>12} {:>10} {:>10} {:>11} {:>11} {:>9} {:>8} {:>8} {:>8} {:>9}",
        step.workloads,
        step.starts_per_sec,
        format!("{}/{}", ms(step.start_p50_ms), ms(step.start_p99_ms)),
        format!(
            "{}/{}/{}",
            step.start_refused, step.start_timeouts, step.start_no_responders
        ),
        render_opt(step.footprint_bytes),
        render_opt(step.bytes_per_workload),
        format!(
            "{}/{}",
            step.heartbeat_workloads
                .map_or("-".into(), |n| n.to_string()),
            step.heartbeat_components
                .map_or("-".into(), |n| n.to_string())
        ),
        if step.warmup_ok + step.warmup_failed == 0 {
            "-".to_string()
        } else {
            format!("{}/{}", step.warmup_ok, step.warmup_failed)
        },
        format!("{}/{}", step.requests_ok, step.requests_failed),
        ms(step.request_p50_ms),
        ms(step.request_p99_ms),
        step.spin_down_ms.map_or("-".into(), |v| format!("{v:.0}")),
        step.theoretical_limit.map_or("-".into(), |n| n.to_string()),
    );
}

pub fn print_flavour_summary(report: &FlavourReport) {
    let s = &report.summary;
    println!(
        "result:   served {} ({} running), theoretical {}, {} per workload, {:.0} starts/s, request p50/p99 {}/{} ms",
        s.served,
        s.achieved,
        s.theoretical_limit.map_or("-".into(), |n| n.to_string()),
        render_opt(s.bytes_per_workload),
        s.starts_per_sec,
        ms(s.request_p50_ms),
        ms(s.request_p99_ms),
    );
    println!("stopped:  {}", report.stop);
    if let crate::bench::Stop::HostExited { log_tail, .. } = &report.stop
        && !log_tail.is_empty()
    {
        println!("host log tail:\n{log_tail}");
    }
}

/// The flavours side by side, one row per measure.
pub fn print_comparison(reports: &[FlavourReport]) {
    if reports.is_empty() {
        return;
    }
    println!();
    println!("== summary ==");
    let row = |label: &str, cell: &dyn Fn(&FlavourReport) -> String| {
        let cells: String = reports.iter().map(|r| format!("{:>18}", cell(r))).collect();
        println!("{label:<26}{cells}");
    };
    row("", &|r| r.flavour.name().to_string());
    row("served workloads", &|r| r.summary.served.to_string());
    row("achieved (running)", &|r| r.summary.achieved.to_string());
    row("theoretical limit", &|r| {
        r.summary
            .theoretical_limit
            .map_or("-".into(), |n| n.to_string())
    });
    row("baseline footprint", &|r| {
        render_opt(r.baseline_footprint_bytes)
    });
    row("peak footprint", &|r| {
        render_opt(r.summary.peak_footprint_bytes)
    });
    row("bytes per workload", &|r| {
        render_opt(r.summary.bytes_per_workload)
    });
    row("core instances (pool)", &|r| {
        r.core_instances.map_or("-".into(), |n| n.to_string())
    });
    row("core instances/workload", &|r| {
        r.core_instances_per_workload
            .map_or("-".into(), |n| n.to_string())
    });
    row("starts/s (overall)", &|r| {
        format!("{:.0}", r.summary.starts_per_sec)
    });
    row("start p50 / p99 ms", &|r| {
        format!(
            "{} / {}",
            ms(r.summary.start_p50_ms),
            ms(r.summary.start_p99_ms)
        )
    });
    row("first request p50/p99 ms", &|r| {
        format!(
            "{} / {}",
            ms(r.summary.warmup_p50_ms),
            ms(r.summary.warmup_p99_ms)
        )
    });
    row("request p50 / p99 ms", &|r| {
        format!(
            "{} / {}",
            ms(r.summary.request_p50_ms),
            ms(r.summary.request_p99_ms)
        )
    });
    row("start timeouts / no-resp", &|r| {
        format!(
            "{} / {}",
            r.summary.start_timeouts, r.summary.start_no_responders
        )
    });
    row("registry GET/HEAD/blobs", &|r| {
        r.registry_requests.map_or("-".into(), |c| {
            format!("{}/{}/{}", c.manifest_gets, c.manifest_heads, c.blob_gets)
        })
    });
    for report in reports {
        println!("{}: stopped by {}", report.flavour.name(), report.stop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_json_reports_omit_credentials_and_unknown_arguments() -> anyhow::Result<()> {
        use clap::Parser as _;
        let machine = Machine::detect();
        let settings = crate::resolve_settings(
            &crate::cli::Args::try_parse_from(["bench-scale"])?,
            &machine,
        )?;
        let report = Report {
            tool: "bench-scale", tool_version: "test", started_unix_secs: 0,
            machine, settings,
            image: ImageInfo { reference: "example/component:v1".into(), source: "external".into(), size_bytes: None, layer_digest: None },
            run_dir: String::new(), flavours: Vec::new(), exit_code: 0,
            nats_url: "nats://SENTINEL_USER:SENTINEL_PASSWORD@localhost:4222?token=SENTINEL_QUERY#SENTINEL_FRAGMENT".into(),
        };
        let host = crate::bench::HostInfo {
            binary: None,
            unsupported_flags: Vec::new(),
            id: String::new(),
            version: String::new(),
            hostname: String::new(),
            http: String::new(),
            pid: None,
            log: None,
            args: [
                "host",
                "--scheduler-nats-url",
                "nats://SENTINEL_TOKEN@localhost:4222",
                "--max-process-memory=2GiB",
                "--password",
                "SENTINEL_ARG",
                "--custom-secret=SENTINEL_CUSTOM",
                "SENTINEL_POSITIONAL",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        };
        let text = format!(
            "{} {}",
            redact_url(&report.nats_url),
            redact_args(&host.args).join(" ")
        );
        let json = serde_json::to_string(&(report, host))?;
        for rendered in [text, json] {
            assert!(!rendered.contains("SENTINEL"), "{rendered}");
            assert!(rendered.contains("localhost:4222"));
            assert!(rendered.contains("2GiB"));
        }
        assert_eq!(redact_url("nats://bad SENTINEL@"), "[redacted address]");
        assert_eq!(redact_url("localhost:4222"), "nats://localhost:4222");
        Ok(())
    }
}
