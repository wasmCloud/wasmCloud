use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context as _;
use chrono::Utc;
use clap::{Args, Parser, Subcommand};
use tracing::instrument;
use wash_runtime::oci::{OciConfig, OciPullPolicy, pull_component, push_component};
use wasm_metadata::Payload;

pub(crate) const OCI_CACHE_DIR: &str = "oci";

use crate::cli::{CliCommand, CliContext, CommandOutput};

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
    /// Use HTTP or HTTPS protocol
    #[arg(long = "insecure", default_value_t = false)]
    pub insecure: bool,
    /// Username for basic authentication
    #[arg(short, long)]
    pub user: Option<String>,
    /// Password for basic authentication
    #[arg(short, long)]
    pub password: Option<String>,
}

impl PullCommand {
    /// Handle the OCI command
    #[instrument(level = "debug", skip_all, name = "oci")]
    pub async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let mut oci_config = OciConfig::new_with_cache(ctx.cache_dir().join(OCI_CACHE_DIR));
        oci_config.insecure = self.insecure;

        if let Some(ref user) = self.user
            && let Some(ref password) = self.password
        {
            oci_config.credentials = Some((user.clone(), password.clone()));
        } else if self.user.as_ref().or(self.password.as_ref()).is_some() {
            tracing::warn!("username or password provided without the other");
        }

        let (c, digest) =
            pull_component(&self.reference, oci_config, OciPullPolicy::Always).await?;

        // Resolve component path relative to project directory if not absolute
        let component_path = if self.component_path.is_absolute() {
            self.component_path.clone()
        } else {
            ctx.original_working_dir().join(&self.component_path)
        };

        // Write the component to the specified output path
        tokio::fs::write(&component_path, &c)
            .await
            .context("failed to write pulled component to output path")?;

        Ok(CommandOutput::ok(
            format!("Pulled and saved component to {}", component_path.display()),
            Some(serde_json::json!({
                "message": "OCI command executed successfully.",
                "output_path": component_path.to_string_lossy(),
                "bytes": c.len(),
                "digest": digest,
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
    /// Use HTTP or HTTPS protocol
    #[arg(long = "insecure", default_value_t = false)]
    pub insecure: bool,
    /// Username for basic authentication
    #[arg(short, long)]
    pub user: Option<String>,
    /// Password for basic authentication
    #[arg(short, long)]
    pub password: Option<String>,
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

        let mut oci_config = OciConfig::new_with_cache(ctx.cache_dir().join(OCI_CACHE_DIR));
        oci_config.insecure = self.insecure;

        if let Some(ref user) = self.user
            && let Some(ref password) = self.password
        {
            oci_config.credentials = Some((user.clone(), password.clone()));
        } else if self.user.as_ref().or(self.password.as_ref()).is_some() {
            tracing::warn!("username or password provided without the other");
        }

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
