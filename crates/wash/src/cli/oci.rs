use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use chrono::Utc;
use clap::{Args, Parser, Subcommand};
use sha2::{Digest, Sha256};
use tracing::instrument;
use wash_runtime::component_source::ComponentSource;
use wash_runtime::oci::{OciConfig, OciPullPolicy, fetch_manifest, push_component};
use wash_topology::Topology;
use wasm_metadata::Payload;

pub(crate) const OCI_CACHE_DIR: &str = "oci";

/// The workload shape, derived at push time; only on the primary component.
pub(crate) const TOPOLOGY_ANNOTATION: &str = "dev.wasm.topology";

/// Digest of that shape (of the annotation JSON bytes), on every component pushed.
pub(crate) const TOPOLOGY_DIGEST_ANNOTATION: &str = "dev.wasm.topology.digest";

use crate::cli::{CliCommand, CliContext, CommandOutput};

/// How to reach a registry, for every command that names an OCI reference.
#[derive(Args, Debug, Clone, Default)]
pub struct RegistryArgs {
    /// Use HTTP or HTTPS protocol
    #[arg(long = "insecure", default_value_t = false)]
    pub insecure: bool,
    /// Username for basic authentication
    #[arg(short, long)]
    pub user: Option<String>,
    /// Password for basic authentication
    #[arg(short, long)]
    pub password: Option<String>,
    /// Extra CA certificate bundle files (PEM) to trust for this registry
    #[arg(long = "ca-path", env = "WASH_OCI_CA_PATHS", value_delimiter = ',')]
    pub ca_paths: Vec<PathBuf>,
}

impl RegistryArgs {
    /// Build the config these flags describe, cached under the shared OCI cache.
    pub fn oci_config(&self, ctx: &CliContext) -> anyhow::Result<OciConfig> {
        self.oci_config_for(ctx, None)
    }

    /// Like [`oci_config`](Self::oci_config); loopback registries default to plain HTTP.
    pub fn oci_config_for(
        &self,
        ctx: &CliContext,
        reference: Option<&str>,
    ) -> anyhow::Result<OciConfig> {
        // Trust is process-wide; failing beats falling back to the public roots.
        if !self.ca_paths.is_empty() {
            wash_runtime::oci::set_extra_ca_certificates(&self.ca_paths)
                .context("failed to load --ca-path CA certificates")?;
        }

        let loopback = reference.is_some_and(|r| {
            r.starts_with("localhost") || r.starts_with("127.") || r.starts_with("[::1]")
        });
        let mut oci_config = OciConfig::new_with_cache(ctx.cache_dir().join(OCI_CACHE_DIR));
        oci_config.insecure = self.insecure || loopback;

        if let (Some(user), Some(password)) = (&self.user, &self.password) {
            oci_config.credentials = Some((user.clone(), password.clone()));
        } else if self.user.as_ref().or(self.password.as_ref()).is_some() {
            tracing::warn!("username or password provided without the other");
        }

        Ok(oci_config)
    }
}

/// Push or pull Wasm components to/from an OCI registry
#[derive(Parser, Debug, Clone)]
#[command(subcommand_required = true, arg_required_else_help = true)]
pub struct OciArgs {
    #[command(subcommand)]
    command: OciCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum OciCommand {
    /// Pull a Wasm component from an OCI registry
    Pull(PullCommand),
    /// Push a Wasm component to an OCI registry
    Push(PushCommand),
    /// Read a pushed component's workload shape without pulling it
    Inspect(InspectCommand),
}

impl CliCommand for OciArgs {
    #[instrument(level = "debug", skip_all, name = "oci")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        self.command.handle(ctx).await
    }
}

impl CliCommand for OciCommand {
    #[instrument(level = "debug", skip_all, name = "oci")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        match self {
            OciCommand::Pull(cmd) => cmd.handle(ctx).await,
            OciCommand::Push(cmd) => cmd.handle(ctx).await,
            OciCommand::Inspect(cmd) => cmd.handle(ctx).await,
        }
    }
}

#[derive(Args, Debug, Clone)]
pub struct PullCommand {
    /// The OCI reference to pull
    pub reference: String,
    /// The path to write the pulled component to
    #[arg(default_value = "component.wasm")]
    pub component_path: PathBuf,
    #[command(flatten)]
    pub registry: RegistryArgs,
}

impl PullCommand {
    #[instrument(level = "debug", skip_all, name = "oci")]
    pub async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let oci_config = self.registry.oci_config_for(ctx, Some(&self.reference))?;

        // `pull` means pull: bypass the cache and go to the registry.
        let loaded = ComponentSource::Oci {
            image: self.reference.clone(),
            pull_policy: OciPullPolicy::Always,
        }
        .load(oci_config)
        .await?;

        let component_path = if self.component_path.is_absolute() {
            self.component_path.clone()
        } else {
            ctx.original_working_dir().join(&self.component_path)
        };

        tokio::fs::write(&component_path, &loaded.bytes)
            .await
            .context("failed to write pulled component to output path")?;

        Ok(CommandOutput::ok(
            format!("Pulled and saved component to {}", component_path.display()),
            Some(serde_json::json!({
                "message": "OCI command executed successfully.",
                "output_path": component_path.to_string_lossy(),
                "bytes": loaded.bytes.len(),
                "digest": loaded.digest,
                "success": true,
            })),
        ))
    }
}

#[derive(Args, Debug, Clone)]
pub struct PushCommand {
    /// The OCI reference to push
    pub reference: String,
    /// The path to the component to push
    pub component_path: PathBuf,
    #[command(flatten)]
    pub registry: RegistryArgs,
}

#[derive(Args, Debug, Clone)]
pub struct InspectCommand {
    /// The OCI reference to inspect
    pub reference: String,
    #[command(flatten)]
    pub registry: RegistryArgs,
}

impl InspectCommand {
    /// Fetch only the manifest, decode the shape, and report drift against a local project.
    #[instrument(level = "debug", skip_all, name = "oci")]
    pub async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let oci_config = self.registry.oci_config_for(ctx, Some(&self.reference))?;
        let (topology, digest) = read_topology_annotation(&self.reference, oci_config).await?;

        let mut message = String::new();
        match (&topology, &digest) {
            (None, None) => message.push_str(
                "no workload shape recorded on this artifact.\n\
                 A `wash oci push` from inside a project records one, derived \
                 from the project as it is pushed.\n",
            ),
            _ => {
                if let Some(topology) = &topology {
                    for line in crate::wizard::picker::detail_lines(topology) {
                        message.push_str(&line);
                        message.push('\n');
                    }
                }
                if let Some(digest) = &digest {
                    message.push_str(&format!("\nshape digest  {digest}\n"));
                    // Derived fresh: "does the artifact match my *current* source".
                    if let Some(local) = derived_shape_json(ctx.project_dir()).await {
                        let local_digest =
                            format!("sha256:{:x}", Sha256::digest(local.as_bytes()));
                        if &local_digest == digest {
                            message.push_str("this project matches the artifact\n");
                        } else {
                            message.push_str(&format!(
                                "DRIFTED from this project ({local_digest}):\n\
                                 the artifact was pushed from a different shape\n"
                            ));
                        }
                    }
                }
            }
        }

        Ok(CommandOutput::ok(
            message,
            Some(serde_json::json!({
                "reference": self.reference,
                "digest": digest,
                "topology": topology,
            })),
        ))
    }
}

/// Fetch a reference's topology annotations: the decoded shape and its digest.
/// A present-but-undecodable shape annotation is an error, never treated as absent.
pub(crate) async fn read_topology_annotation(
    reference: &str,
    config: OciConfig,
) -> anyhow::Result<(Option<Topology>, Option<String>)> {
    let manifest = fetch_manifest(reference, config).await?;
    let annotations = manifest.annotations.unwrap_or_default();
    let digest = annotations.get(TOPOLOGY_DIGEST_ANNOTATION).cloned();
    let topology = annotations
        .get(TOPOLOGY_ANNOTATION)
        .map(|raw| {
            serde_json::from_str::<Topology>(raw).with_context(|| {
                format!("{reference} carries a shape annotation this wash cannot decode")
            })
        })
        .transpose()?;
    Ok((topology, digest))
}

/// Annotations describing the workload a component belongs to, derived at push time.
/// Compact JSON — registries reject raw newlines in annotations (zot).
async fn topology_annotations(project: &Path, component: &Path) -> HashMap<String, String> {
    let Some(json) = derived_shape_json(project).await else {
        return HashMap::new();
    };

    let mut annotations = HashMap::new();
    annotations.insert(
        TOPOLOGY_DIGEST_ANNOTATION.to_string(),
        format!("sha256:{:x}", Sha256::digest(json.as_bytes())),
    );
    if is_primary(project, component).await {
        annotations.insert(TOPOLOGY_ANNOTATION.to_string(), json);
    }
    annotations
}

/// Derive and serialise the project's shape as the one canonical compact-JSON
/// form the digest is defined over; `None` when the directory is not a project.
async fn derived_shape_json(project: &Path) -> Option<String> {
    let dir = project.to_path_buf();
    let id = dir.file_name()?.to_string_lossy().to_string();
    // Derivation is synchronous fs/wasm probing; keep it off the async executor.
    let topology = tokio::task::spawn_blocking(move || wash_topology::derive(&dir, &id))
        .await
        .ok()?
        .ok()?;
    serde_json::to_string(&topology).ok()
}

/// Whether this component is the project's named build target, and so carries the
/// shape. Compared canonically — `build.component_path` is project-relative.
async fn is_primary(project: &Path, component: &Path) -> bool {
    let config_path = crate::config::locate_project_config(project);
    let Ok(config) = crate::config::load_config_from_file(&config_path) else {
        return false;
    };
    let Some(target) = config.build.and_then(|build| build.component_path) else {
        return false;
    };
    let target = if target.is_absolute() {
        target
    } else {
        project.join(target)
    };
    match (
        tokio::fs::canonicalize(&target).await,
        tokio::fs::canonicalize(component).await,
    ) {
        (Ok(target), Ok(component)) => target == component,
        // An unbuilt target cannot be the thing being pushed.
        _ => false,
    }
}

impl PushCommand {
    #[instrument(level = "debug", skip_all, name = "oci")]
    pub async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let component_path = if self.component_path.is_absolute() {
            self.component_path.clone()
        } else {
            ctx.original_working_dir().join(&self.component_path)
        };

        let component = tokio::fs::read(&component_path)
            .await
            .context("failed to read component file")?;

        let payload = Payload::from_binary(&component)?;
        let metadata = payload.metadata();

        let mut all_annotations = HashMap::new();
        if let Some(name) = &metadata.name {
            all_annotations.insert("org.opencontainers.image.title".into(), name.to_string());
        }
        if let Some(description) = &metadata.description {
            all_annotations.insert(
                "org.opencontainers.image.description".into(),
                description.to_string(),
            );
        }
        if let Some(authors) = &metadata.authors {
            all_annotations.insert(
                "org.opencontainers.image.authors".into(),
                authors.to_string(),
            );
        }
        if let Some(source) = &metadata.source {
            all_annotations.insert("org.opencontainers.image.source".into(), source.to_string());
        }
        if let Some(homepage) = &metadata.homepage {
            all_annotations.insert("org.opencontainers.image.url".into(), homepage.to_string());
        }
        if let Some(version) = &metadata.version {
            all_annotations.insert(
                "org.opencontainers.image.version".into(),
                version.to_string(),
            );
        }
        if let Some(revision) = &metadata.revision {
            all_annotations.insert(
                "org.opencontainers.image.revision".into(),
                revision.to_string(),
            );
        }
        if let Some(licenses) = &metadata.licenses {
            all_annotations.insert(
                "org.opencontainers.image.licenses".into(),
                licenses.to_string(),
            );
        }

        all_annotations.insert(
            "org.opencontainers.image.created".into(),
            Utc::now().to_rfc3339(),
        );
        all_annotations.extend(topology_annotations(ctx.project_dir(), &component_path).await);

        let oci_config = self.registry.oci_config_for(ctx, Some(&self.reference))?;

        let digest = push_component(
            &self.reference,
            &component,
            oci_config,
            Some(all_annotations),
        )
        .await?;

        Ok(CommandOutput::ok(
            "OCI command executed successfully.".to_string(),
            Some(serde_json::json!({
                "message": "OCI command executed successfully.",
                "success": true,
                "digest": digest,
            })),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project naming `built.wasm` as its build target; the bytes are not decodable wasm.
    async fn project(root: &Path) -> PathBuf {
        tokio::fs::create_dir_all(root.join(".wash"))
            .await
            .expect("mkdir");
        tokio::fs::create_dir_all(root.join("target"))
            .await
            .expect("mkdir");
        tokio::fs::write(
            root.join(".wash").join("config.yaml"),
            "build:\n  component_path: target/built.wasm\n",
        )
        .await
        .expect("write config");
        let built = root.join("target").join("built.wasm");
        tokio::fs::write(&built, b"\0asm")
            .await
            .expect("write component");
        built
    }

    #[tokio::test]
    async fn the_primary_carries_the_shape_and_its_digest() {
        let root = tempfile::tempdir().expect("tempdir");
        let built = project(root.path()).await;

        let annotations = topology_annotations(root.path(), &built).await;

        // Compact JSON: zot rejects an annotation carrying a raw newline.
        let inline = annotations
            .get(TOPOLOGY_ANNOTATION)
            .expect("the primary carries the shape");
        assert!(
            !inline.contains('\n'),
            "a newline makes the manifest invalid: {inline}"
        );
        let round_tripped: Topology =
            serde_json::from_str(inline).expect("the annotation is a Topology");
        assert_eq!(round_tripped.nodes.len(), 1, "derived from the config");
        assert_eq!(round_tripped.nodes[0].id, "built");

        assert_eq!(
            annotations
                .get(TOPOLOGY_DIGEST_ANNOTATION)
                .map(String::as_str),
            Some(format!("sha256:{:x}", Sha256::digest(inline.as_bytes())).as_str()),
            "the digest must be of the annotation bytes, or it identifies nothing"
        );
    }

    #[tokio::test]
    async fn a_sibling_carries_only_the_digest_and_the_same_one() {
        // Only the build target carries the shape, so N pushes cannot disagree.
        let root = tempfile::tempdir().expect("tempdir");
        let built = project(root.path()).await;
        let sibling = root.path().join("target").join("worker.wasm");
        tokio::fs::write(&sibling, b"\0asm").await.expect("write");

        let primary = topology_annotations(root.path(), &built).await;
        let annotations = topology_annotations(root.path(), &sibling).await;

        assert!(
            !annotations.contains_key(TOPOLOGY_ANNOTATION),
            "only the primary carries the document"
        );
        assert_eq!(
            annotations.get(TOPOLOGY_DIGEST_ANNOTATION),
            primary.get(TOPOLOGY_DIGEST_ANNOTATION),
        );
    }

    #[tokio::test]
    async fn a_directory_that_is_not_a_project_annotates_nothing() {
        // Pushing a component is not the moment to demand a project layout.
        let root = tempfile::tempdir().expect("tempdir");
        let stray = root.path().join("built.wasm");
        tokio::fs::write(&stray, b"\0asm").await.expect("write");

        assert!(topology_annotations(root.path(), &stray).await.is_empty());
    }
}
