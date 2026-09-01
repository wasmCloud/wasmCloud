//! The chosen meter is the one that actually records.
//!
//! Its own binary, and one test: reading back what was recorded means installing
//! a global OTel meter provider, and that provider is process-wide. Three tests
//! doing it in parallel each see whichever provider won the race.
//!
//! Without reading the histogram back these assertions would be vacuous. A
//! `GuestMeter` whose meters are both inert runs the call and returns its value
//! exactly like one that measured it, so "the call succeeded" proves nothing
//! about metering. What distinguishes them is a data point.

use anyhow::{Context, Result};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use wash_runtime::{
    engine::{
        Engine,
        ctx::{Ctx, SharedCtx},
    },
    observability::{MeterKind, Meters},
};

/// Runs one measured call under `kind` and returns the metric names that
/// recorded a data point.
async fn record_one(kind: MeterKind) -> Result<Vec<String>> {
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(exporter.clone()).build())
        .build();
    // Installed before the meters are built: `Meters::new` resolves its
    // histograms from whatever provider is global at that moment.
    opentelemetry::global::set_meter_provider(provider.clone());

    let engine = Engine::builder()
        .with_fuel_consumption(kind.consumes_fuel())
        .build()?;
    let meter = Meters::new(kind).guest();
    let ctx = Ctx::builder("test-workload", "test-component").build();
    let mut store = wasmtime::Store::new(engine.inner(), SharedCtx::new(ctx));

    let measured = meter
        .observe(&[], &mut store, async |_store| Ok(7))
        .await
        .with_context(|| format!("the {kind:?} meter failed the call it was measuring"))?;
    assert_eq!(measured, 7, "the measured call's own value must come back");

    provider.force_flush()?;
    Ok(exporter
        .get_finished_metrics()?
        .iter()
        .flat_map(|rm| rm.scope_metrics())
        .flat_map(|sm| sm.metrics())
        .map(|m| m.name().to_string())
        .collect())
}

/// Each choice records its own histogram and only its own, on a path that hands
/// over a `&mut Store`.
///
/// The epoch case is what "epoch is universal" means. Such a path measures
/// through `GuestMeter`, which uses the meter the host chose; before, those
/// paths named the fuel meter, so under `epoch` the p2 HTTP path and both
/// messaging backends recorded nothing at all.
///
/// One test, sequentially, because the meter provider each case installs is
/// process-global.
#[tokio::test]
async fn a_meter_kind_records_duration_and_only_its_own_cpu_metric() -> Result<()> {
    // Duration is not the choice: it is what an operator asks for first, it
    // costs the guest two clock reads, and it is recorded under either kind.
    for kind in [MeterKind::Duration, MeterKind::Fuel] {
        let recorded = record_one(kind).await?;
        assert!(
            recorded
                .iter()
                .any(|name| name == "guest.invocation.duration"),
            "{kind:?} recorded no invocation duration; got {recorded:?}"
        );
    }

    // Fuel is the one that costs the guest, so only `fuel` records it.
    let duration = record_one(MeterKind::Duration).await?;
    assert!(
        !duration.iter().any(|name| name == "fuel.consumption"),
        "`duration` must not record fuel; got {duration:?}"
    );

    let fuel = record_one(MeterKind::Fuel).await?;
    assert!(
        fuel.iter().any(|name| name == "fuel.consumption"),
        "`fuel` recorded no consumption; got {fuel:?}"
    );

    // `off` is about these two only: the engine's own memory instruments are
    // not guest metering and are recorded regardless.
    let off = record_one(MeterKind::Off).await?;
    assert!(
        !off.iter()
            .any(|name| { name == "guest.invocation.duration" || name == "fuel.consumption" }),
        "a host metering nothing recorded a guest metric; got {off:?}"
    );

    Ok(())
}
