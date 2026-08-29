//! The metrics contract for the guest memory budget.
//!
//! Two things nothing else covers.
//!
//! **That a real [`Engine`] publishes at all.** The unit tests in
//! `engine::guest_memory` call `register_metrics` directly with a meter of
//! their own, so they would still pass if `Engine::build` stopped registering
//! the budget it just built. Only building an engine the way a host does
//! catches that.
//!
//! **That the names, units and instrument kinds do not move.** These are a
//! public interface: an operator's dashboard queries them by name and a panel
//! divides `in_use` by `limit`. Renaming one, or turning a gauge into a
//! counter, breaks every dashboard silently and at a distance — nothing in the
//! runtime fails. So they are pinned here as a spec, and changing one should
//! mean deliberately editing this list.
//!
//! # Why this is a file of its own
//!
//! It installs the *process-wide* meter provider, which is what `Engine::build`
//! reads. Cargo gives each `tests/*.rs` its own binary and therefore its own
//! process, so doing that here cannot disturb another test's metrics — but any
//! test added to this file shares that provider, and will see this engine's
//! instruments too.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};

use opentelemetry_sdk::metrics::{
    InMemoryMetricExporter, PeriodicReader, SdkMeterProvider,
    data::{AggregatedMetrics, MetricData},
};
use wash_runtime::engine::{Engine, guest_memory::GuestMemoryMode, host_memory::HostMemoryBudgets};

const MIB: u64 = 1024 * 1024;
const BUDGET: u64 = 512 * MIB;

/// One exported instrument, reduced to the parts a dashboard depends on.
#[derive(Debug, PartialEq, Eq)]
struct Exported {
    kind: &'static str,
    unit: String,
    value: u64,
}

#[test]
fn a_built_engine_publishes_its_guest_memory_budget() {
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(exporter.clone()).build())
        .build();
    // Before the engine is built: `Engine::build` resolves its meter from
    // whatever the global provider is at that moment, so a provider installed
    // afterwards would leave the instruments bound to the no-op one.
    opentelemetry::global::set_meter_provider(provider.clone());

    // Held, not dropped: the callbacks hold a weak reference to the budget, so
    // an engine that went away before the flush would export nothing and this
    // test would pass for the wrong reason.
    let engine = Engine::builder()
        .with_host_memory(HostMemoryBudgets::resolve(Some(BUDGET), None, None).unwrap())
        .with_guest_memory_mode(GuestMemoryMode::Enforce)
        .build()
        .expect("engine builds");

    provider.force_flush().expect("flush");
    let exported = collect(&exporter);

    let names: BTreeSet<&str> = exported.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        BTreeSet::from([
            "guest_memory.high_water",
            "guest_memory.in_use",
            "guest_memory.limit",
            "guest_memory.refused",
            "guest_memory.would_refuse",
        ]),
        "the exported metric names are a dashboard's interface; changing this \
         set means editing dashboards too"
    );

    // A fresh engine has run no guest, so every figure but the cap is zero —
    // and the cap proves these are *this* engine's instruments rather than
    // some default.
    assert_eq!(
        exported.get("guest_memory.limit"),
        Some(&Exported {
            kind: "gauge",
            unit: "By".to_string(),
            value: BUDGET,
        }),
    );
    for name in ["guest_memory.in_use", "guest_memory.high_water"] {
        assert_eq!(
            exported.get(name),
            Some(&Exported {
                kind: "gauge",
                unit: "By".to_string(),
                value: 0,
            }),
            "{name} must be a byte gauge, at zero on a host that has run nothing"
        );
    }
    for name in ["guest_memory.refused", "guest_memory.would_refuse"] {
        assert_eq!(
            exported.get(name),
            Some(&Exported {
                kind: "sum",
                unit: "{growth}".to_string(),
                value: 0,
            }),
            "{name} counts events and must be a monotonic sum, not a gauge"
        );
    }

    drop(engine);
}

fn collect(exporter: &InMemoryMetricExporter) -> BTreeMap<String, Exported> {
    let mut exported = BTreeMap::new();
    for resource in exporter.get_finished_metrics().expect("finished metrics") {
        for scope in resource.scope_metrics() {
            for metric in scope.metrics() {
                if !metric.name().starts_with("guest_memory.") {
                    continue;
                }
                // An unexpected representation is recorded rather than
                // rejected here, so it fails the assertions below naming the
                // metric and what it turned into — a bare panic from inside
                // the collector would say only that something was wrong.
                let (kind, value) = match metric.data() {
                    AggregatedMetrics::U64(MetricData::Gauge(gauge)) => {
                        ("gauge", gauge.data_points().map(|p| p.value()).next())
                    }
                    AggregatedMetrics::U64(MetricData::Sum(sum)) => {
                        ("sum", sum.data_points().map(|p| p.value()).next())
                    }
                    AggregatedMetrics::U64(_) => ("neither a gauge nor a sum", None),
                    _ => ("not a u64 metric", None),
                };
                exported.insert(
                    metric.name().to_string(),
                    Exported {
                        kind,
                        unit: metric.unit().to_string(),
                        value: value.unwrap_or_default(),
                    },
                );
            }
        }
    }
    exported
}
