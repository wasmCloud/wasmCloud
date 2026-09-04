//! Two hosts in one process measure through their own meters, not each other's.
//!
//! Its own binary, because reading metrics back means installing a global OTel
//! meter provider and that provider is process-wide.
//!
//! This is the wiring `host::tests` cannot reach: `Host::meters` →
//! `UnresolvedWorkload::resolve` → the store builders → the stamp every call
//! path reads. Asserting on `Meters` alone would pass with none of it connected.

use anyhow::{Context, Result};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use wash_runtime::engine::Engine;
use wash_runtime::host::http::{DevRouter, Ingress};
use wash_runtime::host::{Host, HostApi};
use wash_runtime::observability::{MeterKind, Meters};
use wash_runtime::types::{LocalResources, WorkloadStopRequest};

mod common;
use common::{component_workload_request, http_only_host_interfaces};

/// p3, deliberately: the p2 path records through the ingress's own injected
/// `GuestMeter`, which was always per-host. `InvocationSample` — the reader that
/// takes its meter off the store's stamp — is on the p3 path.
const HTTP_HANDLER_P3_WASM: &[u8] = include_bytes!("wasm/http_handler_p3.wasm");

/// A host that meters nothing, started alongside one that meters, must leave
/// the histogram empty. It shares the process, the OTel provider and the
/// instrument name with the other host; only its own meter says otherwise.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_metering_nothing_records_nothing_beside_one_that_meters() -> Result<()> {
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(exporter.clone()).build())
        .build();
    // Installed before any meter is built: `Meters::new` resolves its
    // instruments from whatever provider is global at that moment.
    opentelemetry::global::set_meter_provider(provider.clone());

    // The metering host goes first, so it is also the one a process-wide meter
    // would have published — making the quiet host's silence meaningful.
    let metered = start_workload(MeterKind::Duration, "metered").await?;
    let quiet = start_workload(MeterKind::Off, "quiet").await?;

    // Only the quiet host is called, so any data point at all is one it
    // recorded.
    for _ in 0..3 {
        quiet
            .get()
            .await
            .context("the quiet host must serve requests")?;
    }

    provider.force_flush()?;
    let names: Vec<String> = exporter
        .get_finished_metrics()?
        .iter()
        .flat_map(|rm| rm.scope_metrics())
        .flat_map(|sm| sm.metrics())
        .map(|m| m.name().to_string())
        .collect();

    assert!(
        !names.iter().any(|name| name == "guest.invocation.duration"),
        "a host built with `MeterKind::Off` recorded its calls through another host's \
         meter; got {names:?}"
    );

    // The same instrument does record for a host that asked, so the assertion
    // above is about whose meter ran, not about the plumbing being dead.
    for _ in 0..3 {
        metered
            .get()
            .await
            .context("the metering host must serve requests")?;
    }
    provider.force_flush()?;
    let names: Vec<String> = exporter
        .get_finished_metrics()?
        .iter()
        .flat_map(|rm| rm.scope_metrics())
        .flat_map(|sm| sm.metrics())
        .map(|m| m.name().to_string())
        .collect();
    assert!(
        names.iter().any(|name| name == "guest.invocation.duration"),
        "a host built with `MeterKind::Duration` recorded nothing; got {names:?}"
    );

    metered.stop().await?;
    quiet.stop().await?;
    Ok(())
}

/// A started workload and the host running it, kept together so the host
/// outlives it.
struct Started {
    host: std::sync::Arc<Host>,
    workload_id: String,
    ingress: std::net::SocketAddr,
}

impl Started {
    /// Drive one request through this host's ingress into its guest.
    async fn get(&self) -> Result<()> {
        let status = reqwest::get(format!("http://{}/", self.ingress))
            .await
            .context("failed to reach the host's ingress")?
            .status();
        anyhow::ensure!(status.is_success(), "guest answered {status}");
        Ok(())
    }

    async fn stop(self) -> Result<()> {
        self.host
            .workload_stop(WorkloadStopRequest {
                workload_id: self.workload_id,
            })
            .await
            .context("failed to stop the workload")?;
        self.host.stop().await.context("failed to stop the host")?;
        Ok(())
    }
}

/// Start one host under `kind` with a single p2 HTTP component on it, so its
/// stores go through the stamping path this test is about.
async fn start_workload(kind: MeterKind, name: &str) -> Result<Started> {
    let engine = Engine::builder()
        .with_pooling_allocator(false)
        .with_fuel_consumption(kind.consumes_fuel())
        .build()
        .context("failed to build the engine")?;
    // `DevRouter`, so a plain GET reaches the workload without this test also
    // having to arrange hostname routing.
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?)
        .await
        .context("failed to bind an ingress")?;
    let bound = ingress.addr();
    let host = Host::builder()
        .with_engine(engine)
        .with_http_handler(std::sync::Arc::new(ingress))
        .with_meters(Meters::new(kind))
        .build()
        .context("failed to build the host")?;
    let host = host.start().await.context("failed to start the host")?;

    let request = component_workload_request(
        "http-handler-p3.wasm",
        name,
        HTTP_HANDLER_P3_WASM,
        LocalResources {
            memory_limit_mb: 128,
            cpu_limit: 1,
            ..Default::default()
        },
        http_only_host_interfaces(name),
    );
    let workload_id = request.workload_id.clone();
    host.workload_start(request)
        .await
        .with_context(|| format!("failed to start the {name} workload"))?;

    Ok(Started {
        host,
        workload_id,
        ingress: bound,
    })
}
