use std::time::Duration;

use anyhow::Context;
use criterion::measurement::Measurement;
use criterion::BenchmarkGroup;
use criterion::Criterion;
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::Arc;
use testcontainers::{
    core::{ImageExt, Mount},
    runners::AsyncRunner,
    ContainerAsync,
};
use url::Url;
use wasmcloud_test_util::testcontainers::NatsServer;

fn main() -> anyhow::Result<()> {
    build_component()?;

    let mut c = Criterion::default().configure_from_args();

    // Host setup test
    {
        let mut group = c.benchmark_group("host");
        bench_host(&mut group);
        group.finish();
    }

    c.final_summary();

    Ok(())
}

/// Quick helper function to build an example component for loading in the host benchmark
pub fn build_component() -> anyhow::Result<()> {
    let current_dir = std::env::current_dir()
        .context("failed to get current dir")?
        .canonicalize()
        .context("failed to canonicalize current dir")?;
    std::env::set_current_dir("./examples/rust/components/blobby")
        .context("failed to set dir to example blobby component")?;
    std::process::Command::new("cargo")
        .arg("build")
        .arg("--target")
        .arg("wasm32-wasip2")
        // Release build for efficient component size + instantiation
        .arg("--release")
        .status()
        .context("failed to build component")?;
    std::fs::copy(
        "./target/wasm32-wasip2/release/blobby.wasm",
        current_dir.join("benches/blobby.wasm"),
    )
    .context("failed to copy component to benches dir")?;
    std::env::set_current_dir(current_dir)
        .context("failed to change directory back to benches dir")?;

    Ok(())
}

pub async fn setup_nats() -> anyhow::Result<(u16, ContainerAsync<NatsServer>)> {
    let nats_cfg = r#"
    max_connections: 1M
    jetstream {
        enabled: true
    }
    "#;

    let mut cfg =
        tempfile::NamedTempFile::new().expect("failed to create tempfile for nats config");
    cfg.write(nats_cfg.as_bytes())
        .map_err(|e| anyhow::anyhow!("Failed to write config: {}", e))
        .expect("failed to write to tempfile for nats config");

    let mount = Mount::bind_mount(cfg.path().to_str().unwrap(), "/nats.cfg");
    let nats = NatsServer::default()
        .with_startup_timeout(Duration::from_secs(15))
        .with_cmd(vec!["-js", "-c", "/nats.cfg", "-DV"])
        .with_mount(mount)
        .start()
        .await
        .context("should start nats-server")?;

    let nats_port = nats
        .get_host_port_ipv4(4222)
        .await
        .expect("should be able to find the NATS port");

    Ok((nats_port, nats))
}

pub fn bench_host(group: &mut BenchmarkGroup<'_, impl Measurement>) {
    let runtime = tokio::runtime::Runtime::new().expect("should create tokio runtime");
    let (nats_port, container) = runtime
        .block_on(setup_nats())
        .expect("should perform NATS container setup");
    let nats_url = format!("nats://127.0.0.1:{nats_port}")
        .parse::<Url>()
        .expect("should parse NATS URL");
    let config = wasmcloud_host::WasmbusHostConfig {
        ctl_nats_url: nats_url.clone(),
        rpc_nats_url: nats_url,
        ..Default::default()
    };
    let ctl_client = runtime.block_on(async {
        wasmcloud_control_interface::ClientBuilder::new(
            async_nats::connect(format!("nats://127.0.0.1:{nats_port}"))
                .await
                .expect("should connect"),
        )
        .build()
    });

    // The next set of benches require a running host, so we'll start one here
    let (host, _) = runtime
        .block_on(async {
            wasmcloud_host::WasmbusHost::new(wasmcloud_host::WasmbusHostConfig {
                // Load component from a file to avoid clogging benchmark with networking
                allow_file_load: true,
                ..config.clone()
            })
            .await
        })
        .expect("should be able to start a host for start_component bench");
    let inventory = runtime.block_on(async { host.inventory().await });
    let host_id = inventory.host_id();

    group.bench_function("handle_ctl_message", |b| {
        b.to_async(&runtime).iter(|| async {
            let links = ctl_client.get_links().await;
            assert!(links.is_ok_and(|c| c.succeeded()));
        });
    });

    // NOTE: we build this component above with `build_component`
    let wasm_bytes = std::fs::read("./benches/blobby.wasm").expect("should read wasm file");
    let component_ref: Arc<str> = "file:///./benches/blobby.wasm".into();

    group.bench_function("start_component", |b| {
        b.to_async(&runtime).iter_batched(
            || (uuid::Uuid::new_v4().to_string().into(), wasm_bytes.clone()),
            |(component_id, wasm)| async {
                let res = host
                    .clone()
                    .handle_scale_component_task(
                        component_ref.clone(),
                        component_id,
                        host_id,
                        10,
                        &BTreeMap::new(),
                        vec![],
                        wasm,
                        None,
                    )
                    .await;
                assert!(res.is_ok());
            },
            criterion::BatchSize::SmallInput,
        );
    });

    runtime
        .block_on(host.clear_inventory())
        .expect("failed to clear running components from host");

    group.bench_function("start_and_stop_component", |b| {
        b.to_async(&runtime).iter_batched(
            || (uuid::Uuid::new_v4().to_string().into(), wasm_bytes.clone()),
            |(component_id, wasm): (Arc<str>, Vec<u8>)| async {
                let host = host.clone();
                let res = host
                    .handle_scale_component_task(
                        component_ref.clone(),
                        component_id.clone(),
                        host_id,
                        10,
                        &BTreeMap::new(),
                        vec![],
                        wasm,
                        None,
                    )
                    .await;
                assert!(res.is_ok());
                let res = host
                    .handle_scale_component_task(
                        component_ref.clone(),
                        component_id,
                        host_id,
                        0,
                        &BTreeMap::new(),
                        vec![],
                        vec![],
                        None,
                    )
                    .await;
                assert!(res.is_ok());
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function("start", |b| {
        b.to_async(&runtime).iter(|| async {
            let host = wasmcloud_host::WasmbusHost::new(config.clone()).await;
            assert!(host.is_ok());
        });
    });

    runtime.block_on(async move {
        container.stop().await.expect("should have stopped NATS");
    });
}
