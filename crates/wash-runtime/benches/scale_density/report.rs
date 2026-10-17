//! Report for the density experiment.
use super::{Report, Settings, Step, or_unknown};
use wash_runtime::engine::host_memory::render_bytes;

fn milliseconds(value: Option<f64>) -> String {
    value.map_or_else(|| "?".into(), |value| format!("{value:.1}"))
}

pub(super) fn print_step(header: bool, step: &Step) {
    if header {
        eprintln!(
            "{:>9} {:>8} {:>10} {:>10} {:>9} {:>6} {:>13} {:>9} {:>7} {:>8} {:>8} {:>7} {:>11} {:>5} {:>10}",
            "workloads",
            "starts/s",
            "footprint",
            "guest mem",
            "per-wkld",
            "guard",
            "core inst",
            "ok/fail",
            "req/s",
            "p50 ms",
            "p99 ms",
            "down ms",
            "refused",
            "cpu%",
            "limit",
        );
    }
    eprintln!(
        "{:>9} {:>8.0} {:>10} {:>10} {:>9} {:>6} {:>13} {:>9} {:>7.0} {:>8} {:>8} {:>7} {:>11} {:>5} {:>10}",
        step.workloads,
        step.starts_per_sec,
        render_bytes(step.footprint_bytes),
        render_bytes(step.guest_memory_bytes),
        render_bytes(step.bytes_per_workload),
        step.guard,
        format!(
            "{}/{}",
            step.core_instances_in_use, step.core_instances_total
        ),
        format!("{}/{}", step.requests_ok, step.requests_failed),
        step.requests_per_sec,
        milliseconds(step.latency_p50_ms),
        milliseconds(step.latency_p99_ms),
        step.spin_down_ms
            .map_or_else(|| "-".to_string(), |ms| format!("{ms:.0}")),
        format!(
            "{}/{}/{}",
            step.memory_refusals, step.guest_refusals, step.pool_refusals
        ),
        step.cpu_percent
            .map_or_else(|| "-".to_string(), |p| format!("{p:.0}")),
        or_unknown(step.theoretical_limit),
    );
}

/// The flavours side by side, one row per measure.
pub(super) fn print_comparison(settings: &Settings, reports: &[Report]) {
    let Some(first) = reports.first() else {
        return;
    };
    eprintln!();
    eprintln!(
        "== {}, max-process-memory {}, max-guest-memory {}, pool {} core instances ==",
        first.fixture,
        render_bytes(settings.memory_limit),
        render_bytes(settings.guest_memory),
        settings.core_instances,
    );
    let row = |label: &str, cell: &dyn Fn(&Report) -> String| {
        let cells: Vec<String> = reports.iter().map(|r| format!("{:>20}", cell(r))).collect();
        eprintln!("{label:<24}{}", cells.join(""));
    };
    let last = |r: &Report| r.steps.last().cloned();
    // Request figures from the last step that still served, not the one the
    // limit stopped, whose refusals would read as zero latency.
    let served = |r: &Report| r.steps.iter().rev().find(|s| s.clean).cloned();
    row("", &|r| r.label.clone());
    row("cores", &|r| match (r.cpu_limit, r.cpu_pinned) {
        (Some(n), true) => format!("{n} pinned"),
        (Some(n), false) => format!("{n} workers"),
        (None, _) => r.cores.to_string(),
    });
    row("served workloads", &|r| r.served.to_string());
    row("limit: max-process-memory", &|r| {
        or_unknown(r.limits.memory)
    });
    row("limit: max-guest-memory", &|r| {
        or_unknown(r.limits.guest_memory)
    });
    row("limit: pool core inst", &|r| {
        or_unknown(r.limits.pool_core_instances)
    });
    row("limit: pool memories", &|r| {
        or_unknown(r.limits.pool_memories)
    });
    row("limit: cpu", &|_| "unbounded (idle)".into());
    row("binding limit", &|r| {
        let (name, limit) = r.limits.binding();
        format!("{} ({name})", or_unknown(limit))
    });
    row("marginal per workload", &|r| {
        r.limits
            .marginal_bytes_per_workload
            .map_or("-".into(), render_bytes)
    });
    row("guest per workload", &|r| {
        r.limits
            .guest_bytes_per_workload
            .map_or_else(|| "0B".into(), render_bytes)
    });
    row("cpu max starts/s", &|r| {
        r.limits
            .max_starts_per_sec
            .map_or("?".into(), |n| format!("{n:.0}"))
    });
    row("cpu max req/s", &|r| {
        r.limits
            .max_requests_per_sec
            .map_or("?".into(), |n| format!("{n:.0}"))
    });
    row("baseline footprint", &|r| {
        render_bytes(r.baseline_footprint_bytes)
    });
    row("final footprint", &|r| {
        last(r).map_or("-".into(), |s| render_bytes(s.footprint_bytes))
    });
    row("guest memory charged", &|r| {
        last(r).map_or("-".into(), |s| render_bytes(s.guest_memory_bytes))
    });
    row("bytes per workload", &|r| {
        last(r).map_or("-".into(), |s| render_bytes(s.bytes_per_workload))
    });
    row("core inst / workload", &|r| {
        r.core_instances_per_workload
            .map_or("-".into(), |n| n.to_string())
    });
    row("starts/s (mean)", &|r| {
        let rates: Vec<f64> = r.steps.iter().map(|s| s.starts_per_sec).collect();
        format!(
            "{:.0}",
            rates.iter().sum::<f64>() / rates.len().max(1) as f64
        )
    });
    row("req/s (last served)", &|r| {
        served(r).map_or("-".into(), |s| format!("{:.0}", s.requests_per_sec))
    });
    row("p50 / p99 ms (served)", &|r| {
        served(r).map_or("-".into(), |s| {
            format!(
                "{} / {}",
                milliseconds(s.latency_p50_ms),
                milliseconds(s.latency_p99_ms)
            )
        })
    });
    row("spin-down ms (served)", &|r| {
        served(r)
            .and_then(|s| s.spin_down_ms)
            .map_or("-".into(), |ms| format!("{ms:.0}"))
    });
    row("refused starts", &|r| {
        last(r).map_or("-".into(), |s| s.refused_starts.to_string())
    });
    row("refused proc/guest/pool", &|r| {
        last(r).map_or("-".into(), |s| {
            format!(
                "{} / {} / {}",
                s.memory_refusals, s.guest_refusals, s.pool_refusals
            )
        })
    });
    row("cpu-s total", &|r| {
        r.cpu_secs_total.map_or("-".into(), |s| format!("{s:.1}"))
    });
    row("cpu-ms per start", &|r| {
        r.cpu_secs_total
            .zip(r.steps.last())
            .filter(|(_, s)| s.workloads > 0)
            .map_or("-".into(), |(secs, s)| {
                format!("{:.2}", secs * 1000.0 / s.workloads as f64)
            })
    });
    row("cpu% (mean)", &|r| {
        let shares: Vec<f64> = r.steps.iter().filter_map(|s| s.cpu_percent).collect();
        if shares.is_empty() {
            return "-".into();
        }
        format!("{:.0}", shares.iter().sum::<f64>() / shares.len() as f64)
    });
    if reports.iter().any(|r| !r.cpu_host_only) {
        eprintln!("CPU figures include the load generator's");
    }
    for report in reports {
        eprintln!("{}: stopped by {}", report.label, report.stop);
    }
}
