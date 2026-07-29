//! Utilities for running helm in tests

use std::path::Path;

use anyhow::{Context, Result, ensure};
use tokio::process::Command;

use crate::common::{
    DEFAULT_HOST_IMAGE_REPOSITORY, DEFAULT_HOST_IMAGE_TAG, DEFAULT_K8S_NAMESPACE, DEFAULT_RELEASE,
    exec_command,
};

/// Ensure all required files and binaries are present
pub async fn validate_deploy_prerequisites(workspace: &Path) -> Result<()> {
    for path in [
        workspace.join("Dockerfile"),
        workspace.join("charts/runtime-operator/Chart.yaml"),
        workspace.join("deploy/kind/kind-config.yaml"),
        workspace.join("runtime-operator/config/samples/service_deployment.yaml"),
    ] {
        ensure!(
            path.is_file(),
            "required repository file is missing: {}",
            path.display()
        );
    }
    for (tool, args) in [
        ("helm", &["version", "--short"][..]),
        ("kubectl", &["version", "--client"][..]),
    ] {
        exec_command(
            &format!("find required tool {tool}"),
            Command::new(tool).args(args),
        )
        .await
        .with_context(|| format!("{tool} is required on PATH"))?;
    }
    Ok(())
}

/// Install a the runtime-operator helm chart in the workspace
pub async fn install_chart(workspace: &Path, kubeconfig: &Path) -> Result<()> {
    let chart = workspace.join("charts/runtime-operator");
    let mut command = Command::new("helm");
    command
        .arg("upgrade")
        .arg("--install")
        .arg("--create-namespace")
        .arg("--namespace")
        .arg(DEFAULT_K8S_NAMESPACE)
        .arg("--kubeconfig")
        .arg(kubeconfig)
        .arg("--wait")
        .arg("--timeout=5m")
        .arg("--set")
        .arg("gateway.enabled=false")
        .arg("--set")
        .arg("runtime.hostGroups[0].name=default")
        .arg("--set")
        .arg("runtime.hostGroups[0].replicas=1")
        .arg("--set")
        .arg("runtime.hostGroups[0].http.enabled=true")
        .arg("--set")
        .arg("runtime.hostGroups[0].http.port=80")
        .arg("--set")
        .arg("runtime.hostGroups[0].webgpu.enabled=false")
        .arg("--set")
        .arg("runtime.hostGroups[0].resources.requests.memory=64Mi")
        .arg("--set")
        .arg("runtime.hostGroups[0].resources.requests.cpu=250m")
        .arg("--set")
        .arg("runtime.hostGroups[0].resources.limits.memory=512Mi")
        .arg("--set")
        .arg("runtime.hostGroups[0].resources.limits.cpu=500m")
        .arg("--set")
        .arg("runtime.image.registry=")
        .arg("--set")
        .arg(format!(
            "runtime.image.repository={DEFAULT_HOST_IMAGE_REPOSITORY}"
        ))
        .arg("--set")
        .arg(format!("runtime.image.tag={DEFAULT_HOST_IMAGE_TAG}"))
        .arg("--set")
        .arg("runtime.image.pull_policy=Never")
        .arg(DEFAULT_RELEASE)
        .arg(chart);
    exec_command("install the runtime-operator chart", &mut command).await?;
    Ok(())
}

/// Print helm related diagnostics
pub async fn print_diagnostics(kubeconfig: &Path) {
    let mut helm = Command::new("helm");
    helm.arg("status")
        .arg(DEFAULT_RELEASE)
        .arg("--namespace")
        .arg(DEFAULT_K8S_NAMESPACE)
        .arg("--kubeconfig")
        .arg(kubeconfig);
    if let Err(err) = exec_command("collect Helm status", &mut helm).await {
        eprintln!("{err:#}");
    }
}
