//! k8s utilities

use std::ffi::OsStr;
use std::path::Path;
use std::process::Output;

use anyhow::Result;
use tokio::process::Command;

use crate::common::{DEFAULT_K8S_NAMESPACE, exec_command, helm};

/// Run a k8s command
pub async fn kubectl<I, S>(kubeconfig: &Path, args: I) -> Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("kubectl");
    command.arg("--kubeconfig").arg(kubeconfig).args(args);
    exec_command("run kubectl", &mut command).await
}

/// Parse kubectl command output
pub async fn kubectl_output<const N: usize>(kubeconfig: &Path, args: [&str; N]) -> Result<String> {
    let output = kubectl(kubeconfig, args).await?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Print k8s cluster diagnostics
pub async fn print_diagnostics(kubeconfig: &Path) {
    let commands: &[&[&str]] = &[
        &[
            "get",
            "pods,services,deployments,endpointslices",
            "--all-namespaces",
            "--output",
            "wide",
        ],
        &[
            "get",
            "hosts.runtime.wasmcloud.dev,workloaddeployments.runtime.wasmcloud.dev,workloads.runtime.wasmcloud.dev",
            "--all-namespaces",
            "--output",
            "yaml",
        ],
        &[
            "get",
            "events",
            "--all-namespaces",
            "--sort-by=.lastTimestamp",
        ],
        &[
            "logs",
            "--namespace",
            DEFAULT_K8S_NAMESPACE,
            "--selector",
            "wasmcloud.com/name=runtime-operator",
            "--tail=500",
        ],
        &[
            "logs",
            "--namespace",
            DEFAULT_K8S_NAMESPACE,
            "--selector",
            "wasmcloud.com/name=hostgroup",
            "--tail=500",
        ],
        &[
            "logs",
            "--namespace",
            DEFAULT_K8S_NAMESPACE,
            "--selector",
            "wasmcloud.com/name=runtime-gateway",
            "--tail=500",
        ],
    ];
    for args in commands {
        let rendered = args.join(" ");
        match kubectl(kubeconfig, args.iter().copied()).await {
            Ok(output) => {
                eprintln!(
                    "\n>>> kubectl {rendered}\n{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Err(err) => eprintln!("\n>>> kubectl {rendered}\n{err:#}"),
        }
    }

    helm::print_diagnostics(kubeconfig).await;
}
