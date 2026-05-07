use anyhow::{Context, Result};
use async_nats::jetstream::object_store;
use std::collections::HashMap;
use testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};

use wash_runtime::{
    engine::Engine,
    fetch_precompiled,
    host::{HostApi, HostBuilder},
    types::{Component, Workload, WorkloadStartRequest},
};

#[tokio::test]
#[ignore = "network: requires Docker for NATS container"]
async fn workload_starts_with_precompiled_component() -> Result<()> {
    let (_container, nats_url) = start_nats().await?;

    let cwasm = precompile_minimal_component()?;

    let bucket = "test-precompiled";
    let key = "minimal.cwasm";
    push_to_nats_bucket(&nats_url, bucket, key, &cwasm).await?;

    let url = format!("nats://{bucket}/{key}");
    let fetched = fetch_precompiled_via(&nats_url, &url).await?;

    let engine = Engine::builder().build()?;
    let host = HostBuilder::new().with_engine(engine.clone()).build()?;
    let host = host.start().await.context("Failed to start host")?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "precompiled-workload".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "precompiled-component".to_string(),
                digest: None,
                bytes: fetched.into(),
                local_resources: Default::default(),
                max_invocations: 1,
                pool_size: 0,
                is_precompiled: true,
            }],
            host_interfaces: vec![],
            volumes: vec![],
        },
    };

    host.workload_start(req).await.context(
        "workload_start failed — if it's a wasm validation error, \
                    the is_precompiled flag was ignored",
    )?;

    Ok(())
}

async fn start_nats() -> Result<(ContainerAsync<GenericImage>, String)> {
    let container = GenericImage::new("nats", "2-alpine")
        .with_exposed_port(4222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .with_cmd(["-js"])
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start NATS container: {e}"))?;

    let port = container
        .get_host_port_ipv4(4222)
        .await
        .map_err(|e| anyhow::anyhow!("failed to get NATS host port: {e}"))?;
    let nats_url = format!("nats://127.0.0.1:{port}");

    Ok((container, nats_url))
}

fn precompile_minimal_component() -> Result<Vec<u8>> {
    let mut config = wasmtime::Config::new();
    config.wasm_component_model(true);
    let engine = wasmtime::Engine::new(&config)?;
    let wasm = wat::parse_str("(component)")?;
    let cwasm = engine.precompile_component(&wasm)?;
    Ok(cwasm)
}

async fn push_to_nats_bucket(nats_url: &str, bucket: &str, key: &str, bytes: &[u8]) -> Result<()> {
    let client = async_nats::connect(nats_url)
        .await
        .with_context(|| format!("failed to connect to NATS at {nats_url}"))?;
    let jetstream = async_nats::jetstream::new(client);
    let store = jetstream
        .create_object_store(object_store::Config {
            bucket: bucket.to_string(),
            ..Default::default()
        })
        .await
        .map_err(|e| anyhow::anyhow!("failed to create object store '{bucket}': {e}"))?;

    let mut reader: &[u8] = bytes;
    store
        .put(key, &mut reader)
        .await
        .map_err(|e| anyhow::anyhow!("failed to put object '{key}' in '{bucket}': {e}"))?;
    Ok(())
}

async fn fetch_precompiled_via(nats_url: &str, url: &str) -> Result<Vec<u8>> {
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("NATS_URL", nats_url);
    }
    fetch_precompiled::fetch(url).await
}

