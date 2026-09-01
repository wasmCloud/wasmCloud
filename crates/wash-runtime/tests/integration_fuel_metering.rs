//! Choosing a guest-execution meter, and the fuel budget the fuel one needs.
//!
//! `MeterKind::Fuel` turns on `Config::consume_fuel`, and a wasmtime store built
//! on such an engine starts with **zero** fuel — calling into the guest traps
//! immediately. `FuelConsumptionMeter::observe` sets a budget itself around the
//! one call it brackets, so the paths it wraps were fine; every other path ran
//! the guest on an empty budget.
//!
//! The rest of these pin the choice itself: fuel is not free, so a host that did
//! not ask for it must not compile its counters into every guest.

use std::collections::HashMap;
use std::io::Read;

use anyhow::{Context, Result};
use gag::BufferRedirect;
use wash_runtime::{
    engine::Engine,
    host::{HostApi, HostBuilder},
    observability::{MeterKind, Meters},
    types::{Component, Service, Workload, WorkloadStartRequest},
};

const CRON_SERVICE_WASM: &[u8] = include_bytes!("wasm/cron_service.wasm");
const CRON_COMPONENT_WASM: &[u8] = include_bytes!("wasm/cron_component.wasm");

/// Whether this engine compiles fuel counters into its guests.
///
/// `get_fuel` is the observable form of `Config::consume_fuel`: it errors on an
/// engine that is not metering fuel, which is what makes this an assertion about
/// the engine rather than about a budget someone set.
fn meters_fuel(engine: &Engine) -> bool {
    wasmtime::Store::new(engine.inner(), ()).get_fuel().is_ok()
}

/// An engine built for a given meter, the way the CLI builds one.
fn engine_for(kind: MeterKind) -> Result<Engine> {
    Engine::builder()
        .with_fuel_consumption(kind.consumes_fuel())
        .build()
}

/// Fuel is off unless it is asked for.
///
/// `consume_fuel` compiles a counter into every block of guest code, and the
/// guest pays it whether or not anyone reads the number. A default host must
/// not.
#[test]
fn a_default_engine_does_not_meter_fuel() -> Result<()> {
    assert!(
        !meters_fuel(&Engine::builder().build()?),
        "the default engine must not compile fuel counters into guests"
    );
    assert!(
        !meters_fuel(&engine_for(MeterKind::Off)?),
        "`off` must not compile fuel counters into guests"
    );
    Ok(())
}

/// Asking for duration must not quietly turn fuel on.
///
/// This is the point of the choice: timing a call costs two clock reads, so a
/// host that wants latency must not also pay fuel's per-block counters, which
/// are compiled into every block of guest code.
#[test]
fn duration_metering_does_not_meter_fuel() -> Result<()> {
    assert!(
        !meters_fuel(&engine_for(MeterKind::Duration)?),
        "`duration` must not compile fuel counters into guests"
    );
    // Meters are host state: building them decides which histograms exist,
    // never how guests are compiled.
    let _meters = Meters::new(MeterKind::Duration);
    assert!(
        !meters_fuel(&Engine::builder().build()?),
        "building duration meters must not turn fuel on for an engine that did not ask"
    );
    Ok(())
}

/// The positive control, so the assertions above cannot pass by being incapable
/// of firing.
#[test]
fn fuel_metering_meters_fuel() -> Result<()> {
    assert!(
        meters_fuel(&engine_for(MeterKind::Fuel)?),
        "`fuel` must compile fuel counters into guests"
    );
    Ok(())
}

/// A fuel-metering host runs its guests rather than trapping them.
///
/// The service calls the component on a timer, so reaching the component's
/// message at all means a store was built, instantiated and called under fuel.
/// Captures process stderr, which is shared, so it is the only test here that
/// starts a host.
#[tokio::test]
async fn a_fuel_metering_host_runs_guest_code() -> Result<()> {
    let mut stderr_capture = BufferRedirect::stderr().expect("failed to redirect stderr");

    let host = HostBuilder::new()
        .with_engine(engine_for(MeterKind::Fuel)?)
        .with_meters(Meters::new(MeterKind::Fuel))
        .build()?
        .start()
        .await
        .context("failed to start the fuel-metering host")?;

    host.workload_start(WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "fuel-metered".to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(CRON_SERVICE_WASM),
                local_resources: Default::default(),
                max_restarts: 0,
            }),
            components: vec![Component {
                name: "cron-component".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(CRON_COMPONENT_WASM),
                local_resources: Default::default(),
                max_invocations: 1,
                max_concurrency: 1,
                pool_size: 0,
                ..Default::default()
            }],
            host_interfaces: vec![],
            volumes: vec![],
        },
    })
    .await
    .context("a fuel-metering host refused to start the workload")?;

    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    let mut output = String::new();
    stderr_capture
        .read_to_string(&mut output)
        .expect("failed to read captured stderr");
    drop(stderr_capture);

    assert!(
        output.contains("Hello from the cron-component!"),
        "the component never ran under fuel metering.\nCaptured stderr:\n{output}"
    );

    host.stop().await?;
    Ok(())
}
