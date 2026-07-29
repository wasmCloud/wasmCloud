//! Common utilities for use during integration tests

use std::ffi::OsStr;
use std::future::Future;
use std::path::Path;
use std::process::Output;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail, ensure};
use reqwest::{Client, StatusCode, header::HOST};
use tokio::process::Command;
use tokio::time::sleep;

pub mod docker;
pub mod helm;
pub mod k8s;

use docker::kind::KindCluster;

pub const DEFAULT_K8S_NAMESPACE: &str = "wasmcloud-system";
pub const DEFAULT_HOST_IMAGE_REPOSITORY: &str = "localhost/wasmcloud-wash";
pub const DEFAULT_HOST_IMAGE_TAG: &str = "rust-k8s-e2e";
pub const DEFAULT_HOST_IMAGE: &str = "localhost/wasmcloud-wash:rust-k8s-e2e";
pub const DEFAULT_RELEASE: &str = "rust-host-e2e";
pub const DEFAULT_CLUSTER_API_PORT: u16 = 6443;
pub const DEFAULT_GATEWAY_NODE_PORT: u16 = 30950;

/// Execute a CLI command
pub async fn exec_command(description: &str, command: &mut Command) -> Result<Output> {
    let std_cmd = command.as_std();
    let command_arr = std::iter::once(std_cmd.get_program())
        .chain(std_cmd.get_args())
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    let output = command
        .output()
        .await
        .with_context(|| format!("failed to {description}: {command_arr}"))?;
    if !output.status.success() {
        bail!(
            "failed to {description}: {command_arr}\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output)
}

/// Wait for a given condition, with configurable timeout & interval
pub async fn wait_for<F, Fut>(
    description: &str,
    timeout: Duration,
    interval: Duration,
    mut check: F,
) -> Result<()>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let deadline = Instant::now() + timeout;
    let mut last_error = anyhow!("condition was never checked");
    while Instant::now() < deadline {
        match check().await {
            Ok(()) => return Ok(()),
            Err(err) => last_error = err,
        }
        sleep(interval).await;
    }
    Err(last_error).with_context(|| format!("timed out waiting for {description}"))
}

/// Deploy and start a basic HTTP wasmCloud workload on a given kind cluster
pub async fn deploy_and_start_basic_http_workload(
    workspace: &Path,
    cluster: &KindCluster,
    skip_image_build: bool,
) -> Result<()> {
    cluster.build_and_load_host_image(skip_image_build).await?;

    helm::install_chart(workspace, &cluster.kubeconfig_path).await?;

    wait_for(
        "a registered wasmCloud host",
        Duration::from_secs(180),
        Duration::from_secs(3),
        || async {
            let output = k8s::kubectl_output(
                &cluster.kubeconfig_path,
                [
                    "get",
                    "hosts.runtime.wasmcloud.dev",
                    "--namespace",
                    DEFAULT_K8S_NAMESPACE,
                    "--output",
                    "jsonpath={.items[0].metadata.name}",
                ],
            )
            .await?;
            ensure!(!output.trim().is_empty(), "no Host resource registered yet");
            Ok(())
        },
    )
    .await?;

    // Start the workload
    let workload = workspace.join("runtime-operator/config/samples/service_deployment.yaml");
    k8s::kubectl(
        &cluster.kubeconfig_path,
        [
            OsStr::new("apply"),
            OsStr::new("--namespace"),
            OsStr::new(DEFAULT_K8S_NAMESPACE),
            OsStr::new("--filename"),
            workload.as_os_str(),
        ],
    )
    .await?;

    wait_for(
        "the hello-workload WorkloadDeployment",
        Duration::from_secs(180),
        Duration::from_secs(3),
        || async {
            let ready = k8s::kubectl_output(
                &cluster.kubeconfig_path,
                [
                    "get",
                    // see: service_deployment.yaml
                    "workloaddeployment/hello-workload",
                    "--namespace",
                    DEFAULT_K8S_NAMESPACE,
                    "--output",
                    "jsonpath={.status.conditions[?(@.type==\"Ready\")].status}",
                ],
            )
            .await?;
            ensure!(ready.trim() == "True", "last Ready condition was {ready:?}");
            Ok(())
        },
    )
    .await?;

    let endpoint = format!("http://{}", cluster.gateway_addr);
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("failed to construct the HTTP client")?;
    wait_for(
        "the hello-world HTTP route",
        Duration::from_secs(15),
        Duration::from_secs(2),
        || async {
            let response_status = client
                .get(&endpoint)
                .header(HOST, "hello.localhost.direct")
                .send()
                .await
                .context("HTTP request failed")?
                .status();
            ensure!(
                response_status == StatusCode::OK,
                "HTTP route returned {response_status:#?}",
            );
            Ok(())
        },
    )
    .await?;

    for path in ["/", "/?source=rust-kubernetes-e2e", "/"] {
        let response = client
            .get(format!("{endpoint}{path}"))
            .header(HOST, "hello.localhost.direct")
            .send()
            .await
            .with_context(|| format!("request to {path} failed"))?;
        ensure!(
            response.status() == StatusCode::OK,
            "request to {path} returned {}",
            response.status()
        );
        let body = response
            .text()
            .await
            .with_context(|| format!("failed to read the response body for {path}"))?;
        ensure!(!body.is_empty(), "request to {path} returned an empty body");
    }

    Ok(())
}
