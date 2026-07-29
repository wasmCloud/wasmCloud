//! End-to-end smoke test for running the Rust wasmCloud host on Kubernetes.
//!
//! This test is intentionally ignored: it builds the host image and creates a
//! complete kind cluster inside a testcontainers-managed Docker-in-Docker
//! container. Run it from the repository root with:
//!
//! ```text
//! cargo test -p wash --test integration_k8s_host -- --include-ignored --nocapture
//! ```
//!
//! Prerequisites: a reachable Docker daemon plus `helm` and `kubectl` on
//! `PATH`. The test downloads pinned kind/Kubernetes images and the chart's
//! service images.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tempfile::TempDir;

mod common;
use common::deploy_and_start_basic_http_workload;
use common::docker::kind::KindCluster;
use common::helm::validate_deploy_prerequisites;
use common::k8s::print_diagnostics;

/// Get the workspace root for the project
fn workspace_root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .context("failed to locate the workspace root")
}

#[tokio::test]
#[ignore = "requires Docker, kind, Helm, kubectl"]
async fn k8s_host_serves_http_workload() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    // Find workspace root, ensure we have all required deps
    let cargo_workspace = workspace_root()?;
    validate_deploy_prerequisites(&cargo_workspace).await?;

    // Create a new temporary directory for all the intermediate files,
    // then star ta KinD cluster
    let temp = TempDir::new().context("failed to create E2E temporary directory")?;
    let temp_path = temp.keep();
    eprintln!("TEMP PATH [{}]", temp_path.display());
    let cluster = KindCluster::start(&temp_path, &cargo_workspace, "rust-host-e2e").await?;

    // Deploy wasmCloud and a basic HTTP workload
    let skip_image_build = std::env::var("TEST_WASH_INT_K8S_SKIP_HOST_IMAGE_BUILD").is_ok();
    let result =
        deploy_and_start_basic_http_workload(&cargo_workspace, &cluster, skip_image_build).await;
    if let Err(err) = &result {
        eprintln!("k8s host E2E failed: {err:#}");
        print_diagnostics(&cluster.kubeconfig_path).await;
    }

    cluster.delete().await?;
    result
}
