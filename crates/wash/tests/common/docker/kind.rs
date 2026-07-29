//! Utilities for managing [kind][kind] clusters during tests
//!
//! [kind]: <https://kind.sigs.k8s.io/>

use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use tokio::process::Command;
use tracing::debug;
use url::Url;

use crate::common::{DEFAULT_CLUSTER_API_PORT, DEFAULT_GATEWAY_NODE_PORT, DEFAULT_HOST_IMAGE};

const KIND_NODE_IMAGE: &str = "kindest/node:v1.36.1";

const DOCKER_BIN: &str = "docker";
const KIND_BIN: &str = "kind";

/// Representation of a kind cluster that may or may not have been started
#[allow(unused)]
pub struct KindCluster {
    // pub dind_container: ContainerAsync<GenericImage>,
    pub work_dir: PathBuf,
    pub cargo_workspace_dir: PathBuf,
    pub kubeconfig_path: PathBuf,
    pub cluster_url: url::Url,
    pub cluster_name: String,
    pub gateway_addr: SocketAddr,
}

impl KindCluster {
    pub async fn start(
        work_dir: &Path,
        cargo_workspace_dir: &Path,
        cluster_name: &str,
    ) -> Result<Self> {
        let shared_dir = work_dir.canonicalize().with_context(|| {
            format!(
                "failed to canonicalize the working dir [{}]",
                work_dir.display()
            )
        })?;

        // Determine path to kubeconfig that will be written
        let kubeconfig_path = shared_dir.join("kubeconfig");
        debug!(?kubeconfig_path, "wrote kubeconfig");

        // Write out kind config
        let kind_config_path = shared_dir.join("kind-config.yaml");
        tokio::fs::write(&kind_config_path, kind_config_yaml())
            .await
            .context("failed to write the derived kind config")?;

        // Create kind cluster
        debug!(
            cluster_name,
            kind_docker_image = KIND_NODE_IMAGE,
            "creating kind cluster..."
        );
        let kind_create_output = Command::new(KIND_BIN)
            .args([
                "create",
                "cluster",
                "--name",
                cluster_name,
                "--image",
                KIND_NODE_IMAGE,
                "--config",
                &format!("{}", kind_config_path.display()),
                "--kubeconfig",
                &format!("{}", kubeconfig_path.display()),
                "--wait",
                "5m",
            ])
            .output()
            .await
            .context("failed to create the kind cluster")?;
        if !kind_create_output.status.success() {
            eprintln!(
                "kind cluster create command failed\nSTDOUT:\n{}\nSTDERR:\n{}",
                String::from_utf8_lossy(&kind_create_output.stdout),
                String::from_utf8_lossy(&kind_create_output.stderr),
            );
        }
        ensure!(
            kind_create_output.status.success(),
            "kind cluster create succeeded"
        );

        let cluster_url = get_server_url_from_kubeconfig(&kubeconfig_path).await?;
        let gateway_addr = match cluster_url.host().context("cluster URL has no host")? {
            url::Host::Ipv4(addr) => {
                SocketAddr::V4(SocketAddrV4::new(addr, DEFAULT_GATEWAY_NODE_PORT))
            }
            url::Host::Ipv6(addr) => {
                SocketAddr::V6(SocketAddrV6::new(addr, DEFAULT_GATEWAY_NODE_PORT, 0, 0))
            }
            _ => bail!("unexpected host for cluster URL"),
        };

        Ok(Self {
            work_dir: PathBuf::from(work_dir),
            cargo_workspace_dir: PathBuf::from(cargo_workspace_dir),
            kubeconfig_path,
            cluster_url,
            cluster_name: cluster_name.into(),
            gateway_addr,
        })
    }

    pub async fn build_and_load_host_image(&self, skip_image_build: bool) -> Result<()> {
        debug!("building host image with docker...");

        // If the docker image tag exists, we can skip building it
        if !skip_image_build {
            debug!(tag = DEFAULT_HOST_IMAGE, "building host image...");
            let docker_build_output = Command::new(DOCKER_BIN)
                .args([
                    "build",
                    "--tag",
                    DEFAULT_HOST_IMAGE,
                    &format!("{}", self.cargo_workspace_dir.display()),
                ])
                .output()
                .await
                .context("failed to build the host image in the isolated Docker daemon")?;
            ensure!(docker_build_output.status.success(), "docker build failed");
        } else {
            debug!(tag = DEFAULT_HOST_IMAGE, "skipping host image build...");
        }

        debug!("loading host image into kind...");
        let kind_load_output = Command::new(KIND_BIN)
            .args([
                "load",
                "docker-image",
                "--name",
                &self.cluster_name,
                DEFAULT_HOST_IMAGE,
            ])
            .output()
            .await
            .context("failed to load the host image into the kind node")?;
        ensure!(kind_load_output.status.success(), "kind image load failed");

        Ok(())
    }

    pub async fn delete(self) -> Result<()> {
        debug!("shutting down kind cluster...");
        let kind_delete_output = Command::new(KIND_BIN)
            .args(["delete", "cluster", "--name", &self.cluster_name])
            .output()
            .await
            .context("kind delete failed")?;
        if !kind_delete_output.status.success() {
            eprintln!(
                "kind cluster delete command failed\nSTDOUT:\n{}\nSTDERR:\n{}",
                String::from_utf8_lossy(&kind_delete_output.stdout),
                String::from_utf8_lossy(&kind_delete_output.stderr),
            );
        }
        ensure!(
            kind_delete_output.status.success(),
            "kind delete cluster command failed"
        );
        Ok(())
    }
}

/// Generate configuration for a default-ish kind config
pub fn kind_config_yaml() -> String {
    format!(
        r#"kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
networking:
  apiServerAddress: "0.0.0.0"
  apiServerPort: {DEFAULT_CLUSTER_API_PORT}
kubeadmConfigPatches:
  - |
    kind: ClusterConfiguration
    apiServer:
      extraArgs:
        enable-admission-plugins: NodeRestriction,OwnerReferencesPermissionEnforcement
nodes:
  - role: control-plane
    extraPortMappings:
      - containerPort: {DEFAULT_GATEWAY_NODE_PORT}
        hostPort: {DEFAULT_GATEWAY_NODE_PORT}
        listenAddress: "0.0.0.0"
        protocol: TCP
"#
    )
}

/// Retrieve serve details from the kubeconfig
async fn get_server_url_from_kubeconfig(kubeconfig_path: &Path) -> Result<Url> {
    let contents = tokio::fs::read_to_string(kubeconfig_path)
        .await
        .with_context(|| {
            format!(
                "failed to read the generated kubeconfig @ [{}]",
                kubeconfig_path.display()
            )
        })?;
    let mut document: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&contents).context("failed to parse the generated kubeconfig")?;
    let cluster = document
        .get_mut("clusters")
        .and_then(serde_yaml_ng::Value::as_sequence_mut)
        .context("generated kubeconfig has no clusters")?
        .first_mut()
        .and_then(|entry| entry.get_mut("cluster"))
        .and_then(serde_yaml_ng::Value::as_mapping_mut)
        .context("generated kubeconfig has no cluster configuration")?;
    let server_addr = cluster
        .get("server")
        .and_then(serde_yaml_ng::Value::as_str)
        .context("failed to parse server addresss string")?;
    Url::parse(server_addr).context("failed to parse server URL")
}
