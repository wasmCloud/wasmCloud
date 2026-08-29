//! Guest code still runs when the engine is metering fuel.
//!
//! `--enable-meters` turns on `Config::consume_fuel`, and a wasmtime store built
//! on such an engine starts with **zero** fuel — calling into the guest traps
//! immediately. `FuelConsumptionMeter::observe` sets a budget itself around the
//! one call it brackets, so the paths it wraps were fine; every other path ran
//! the guest on an empty budget.
//!
//! One test, because it captures process stderr, which is shared. The no-fuel
//! control is `integration_cron_service`, which runs the same fixtures.

use std::collections::HashMap;
use std::io::Read;

use anyhow::{Context, Result};
use gag::BufferRedirect;
use wash_runtime::{
    engine::Engine,
    host::{HostApi, HostBuilder},
    types::{Component, Service, Workload, WorkloadStartRequest},
};

const CRON_SERVICE_WASM: &[u8] = include_bytes!("wasm/cron_service.wasm");
const CRON_COMPONENT_WASM: &[u8] = include_bytes!("wasm/cron_component.wasm");

/// A metering host runs its guests rather than trapping them.
///
/// The service calls the component on a timer, so reaching the component's
/// message at all means a store was built, instantiated and called under fuel.
#[tokio::test]
async fn a_fuel_metering_host_runs_guest_code() -> Result<()> {
    let mut stderr_capture = BufferRedirect::stderr().expect("failed to redirect stderr");

    let engine = Engine::builder().with_fuel_consumption(true).build()?;
    let host = HostBuilder::new()
        .with_engine(engine)
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
