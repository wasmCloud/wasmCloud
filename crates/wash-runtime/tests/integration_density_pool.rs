//! The density benchmark's P3 fixture must retain a warm instance per workload.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::Result;
use wash_runtime::{
    engine::{Engine, host_memory::HostMemoryBudgets},
    host::{
        HostApi, HostBuilder,
        http::{DynamicRouter, Ingress},
    },
    types::{Component, Workload, WorkloadStartRequest, WorkloadState},
    wit::WitInterface,
};

const WASM: &[u8] = include_bytes!("wasm/http_handler_p3.wasm");

#[tokio::test]
async fn density_fixture_retains_warm_instances_and_releases_cold_ones() -> Result<()> {
    for pool_size in [1, 0] {
        let engine = Engine::builder()
            .with_host_memory(
                HostMemoryBudgets::resolve(Some(1 << 30), None, Some(64), None)
                    .map_err(anyhow::Error::msg)?,
            )
            .build()?;
        let ingress = Ingress::new(DynamicRouter::default(), "127.0.0.1:0".parse()?).await?;
        let addr = ingress.addr();
        let host = HostBuilder::new()
            .with_engine(engine.clone())
            .with_http_handler(Arc::new(ingress))
            .build()?
            .start()
            .await?;
        let baseline = engine.pool_usage().unwrap().core_instances.in_use;
        for index in 0..3 {
            let hostname = format!("w{index}.scale.test");
            let response = host
                .workload_start(WorkloadStartRequest {
                    workload_id: format!("density-{index}"),
                    workload: Workload {
                        namespace: "test".into(),
                        name: hostname.clone(),
                        annotations: HashMap::new(),
                        service: None,
                        components: vec![Component {
                            name: "handler".into(),
                            bytes: bytes::Bytes::from_static(WASM),
                            pool_size,
                            ..Default::default()
                        }],
                        host_interfaces: vec![WitInterface {
                            namespace: "wasi".into(),
                            package: "http".into(),
                            interfaces: ["handler".into()].into(),
                            version: Some(semver::Version::new(0, 3, 0)),
                            config: [("host".into(), hostname)].into(),
                            name: None,
                        }],
                        volumes: vec![],
                    },
                })
                .await?;
            assert_eq!(
                response.workload_status.workload_state,
                WorkloadState::Running
            );
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?;
        let mut retained = None;
        for _ in 0..2 {
            for index in 0..3 {
                let response = client
                    .get(format!("http://{addr}/"))
                    .header("host", format!("w{index}.scale.test"))
                    .send()
                    .await?;
                assert!(response.status().is_success());
                assert_eq!(response.text().await?, "hello from p3");
            }
            // Response bodies finish before the driver's final cleanup is necessarily polled.
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let in_use = engine.pool_usage().unwrap().core_instances.in_use;
                    if (pool_size == 0 && in_use == baseline)
                        || (pool_size == 1 && in_use >= baseline + 3)
                    {
                        if let Some(previous) = retained {
                            assert_eq!(previous, in_use);
                        }
                        retained = Some(in_use);
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await?;
        }
        host.stop().await?;
    }
    Ok(())
}
