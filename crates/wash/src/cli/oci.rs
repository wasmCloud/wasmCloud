use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context as _;
use chrono::Utc;
use clap::{Args, Parser, Subcommand};
use tracing::instrument;
use wash_runtime::component_source::ComponentSource;
use wash_runtime::oci::{OciConfig, OciPullPolicy, push_component};
use wasm_metadata::Payload;

pub(crate) const OCI_CACHE_DIR: &str = "oci";

use crate::cli::{CliCommand, CliContext, CommandOutput};

/// Add a registry CA hint only when the registry's certificate chains to an
/// untrusted root. Expired, revoked or wrong-name certificates get no hint,
/// because adding a CA cannot fix them.
///
/// rustls reports an untrusted root as `UnknownIssuer`. The macOS platform
/// verifier reports it as `errSecNotTrusted` (-67843) inside an `Other`.
fn with_registry_ca_hint(error: anyhow::Error) -> anyhow::Error {
    let is_untrusted_root = error.chain().any(|cause| {
        let message = cause.to_string();
        message.contains("invalid peer certificate")
            && (message.contains("UnknownIssuer") || message.contains("-67843"))
    });

    if is_untrusted_root {
        error.context(
            "the registry's certificate is not signed by a trusted CA; pass --ca-path <bundle.pem> (or set WASH_OCI_CA_PATHS)",
        )
    } else {
        error
    }
}

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
    /// Extra CA certificate bundle files (PEM) to trust for this registry:
    /// one behind a private or in-cluster CA that neither the OS trust store
    /// nor SSL_CERT_FILE / SSL_CERT_DIR covers.
    #[arg(long = "ca-path", env = "WASH_OCI_CA_PATHS", value_delimiter = ',')]
    pub ca_paths: Vec<PathBuf>,
}

impl RegistryArgs {
    /// Build the config these flags describe, cached under the shared OCI cache
    /// so a pull by one command is a cache hit for the next.
    ///
    /// Supplying only one half of a credential pair is a no-op rather than an
    /// error: the ambient docker credential helper may well have the answer.
    pub fn oci_config(&self, ctx: &CliContext) -> anyhow::Result<OciConfig> {
        // Trust is process-wide (see `wash_runtime::oci`); a CLI invocation
        // performs one operation, so setting it here is the same as setting it
        // at startup. Fails rather than falling back to the public roots, which
        // would surface as a verification error against the registry the
        // bundle was meant to cover.
        wash_runtime::oci::set_extra_ca_certificates(&self.ca_paths)
            .context("failed to load --ca-path CA certificates")?;

        let mut oci_config = OciConfig::new_with_cache(ctx.cache_dir().join(OCI_CACHE_DIR));
        oci_config.insecure = self.insecure;

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
}

impl CliCommand for OciArgs {
    #[instrument(level = "debug", skip_all, name = "oci")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        self.command.handle(ctx).await
    }
}

impl CliCommand for OciCommand {
    /// Handle the OCI command
    #[instrument(level = "debug", skip_all, name = "oci")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        match self {
            OciCommand::Pull(cmd) => cmd.handle(ctx).await,
            OciCommand::Push(cmd) => cmd.handle(ctx).await,
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
    /// Handle the OCI command
    #[instrument(level = "debug", skip_all, name = "oci")]
    pub async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let oci_config = self.registry.oci_config(ctx)?;

        // `pull` means pull: go to the registry even when the cache already has
        // this reference.
        let loaded = ComponentSource::Oci {
            image: self.reference.clone(),
            pull_policy: OciPullPolicy::Always,
        }
        .load(oci_config)
        .await
        .map_err(with_registry_ca_hint)?;

        // Resolve component path relative to project directory if not absolute
        let component_path = if self.component_path.is_absolute() {
            self.component_path.clone()
        } else {
            ctx.original_working_dir().join(&self.component_path)
        };

        // Write the component to the specified output path
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

impl PushCommand {
    /// Handle the OCI command
    #[instrument(level = "debug", skip_all, name = "oci")]
    pub async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        // Resolve component path relative to project directory if not absolute
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

        let oci_config = self.registry.oci_config(ctx)?;

        let digest = push_component(
            &self.reference,
            &component,
            oci_config,
            Some(all_annotations),
        )
        .await
        .map_err(with_registry_ca_hint)?;

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
    use super::with_registry_ca_hint;

    #[test]
    fn certificate_verification_errors_include_ca_guidance() {
        let error =
            with_registry_ca_hint(anyhow::anyhow!("invalid peer certificate: UnknownIssuer"));

        assert!(
            error.to_string().contains("--ca-path <bundle.pem>"),
            "certificate verification failures should explain how to add a CA"
        );
        assert!(
            error.to_string().contains("WASH_OCI_CA_PATHS"),
            "the hint should name the environment variable"
        );
    }

    #[test]
    fn macos_untrusted_root_includes_ca_guidance() {
        let error = with_registry_ca_hint(anyhow::anyhow!(
            "invalid peer certificate: Other(OtherError(\"“registry.test” certificate is not trusted: -67843\"))"
        ));

        assert!(error.to_string().contains("--ca-path <bundle.pem>"));
    }

    #[test]
    fn certificate_errors_a_ca_cannot_fix_do_not_include_ca_guidance() {
        for message in [
            "invalid peer certificate: Expired",
            "invalid peer certificate: certificate not valid for name \"registry.test\"; certificate is only valid for other.test",
            "invalid peer certificate: Revoked",
        ] {
            let error = with_registry_ca_hint(anyhow::anyhow!(message));
            assert!(
                !error.to_string().contains("--ca-path"),
                "{message:?} should not suggest adding a CA"
            );
        }
    }

    #[test]
    fn unrelated_errors_do_not_include_ca_guidance() {
        let error = with_registry_ca_hint(anyhow::anyhow!("failed to read component file"));

        assert!(
            !error.to_string().contains("--ca-path"),
            "unrelated failures should keep their original message"
        );
    }
}
