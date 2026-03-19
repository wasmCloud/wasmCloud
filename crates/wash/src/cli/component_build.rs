//! CLI command for building components

use std::{path::PathBuf, process::Stdio, vec};

use anyhow::{Context as _, anyhow, bail};
use clap::Args;
use serde::Serialize;
use tokio::process::Command;
use tracing::{debug, error, info, instrument, trace};

use crate::wit::WitConfig;
use crate::{
    cli::{CliCommand, CliContext, CommandOutput},
    config::Config,
    wit::{CommonPackageArgs, WkgFetcher, load_lock_file},
};

/// CLI command for building components
#[derive(Debug, Clone, Args, Serialize)]
pub struct ComponentBuildCommand {
    /// Skip fetching WIT dependencies, useful for offline builds
    #[arg(long = "skip-fetch")]
    skip_fetch: bool,
}

impl CliCommand for ComponentBuildCommand {
    #[instrument(level = "debug", skip(self, ctx), name = "component_build")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        // Load configuration with CLI arguments override
        let mut config = ctx.load_config(None::<Config>)?;
        // Ensure the CLI argument takes precedence
        if let Some(wit) = config.wit.as_mut() {
            wit.skip_fetch = self.skip_fetch;
        } else {
            config.wit = Some(WitConfig {
                skip_fetch: self.skip_fetch,
                ..Default::default()
            })
        }

        let result = build_component(ctx, &config).await?;

        Ok(CommandOutput::ok(
            format!(
                "Successfully built component at: {}",
                result.component_path.display()
            ),
            Some(serde_json::json!({
                "component_path": result.component_path,
                "project_path": result.project_path,
            })),
        ))
    }
}

/// Result of a component build operation
#[derive(Debug, Clone)]
pub struct ComponentBuildResult {
    /// Path to the built component artifact
    pub component_path: PathBuf,
    /// Original project path
    pub project_path: PathBuf,
}

/// Build a component at the specified project path
///
/// This is the main public interface for building components that can be reused
/// throughout the project. It handles project detection, tool validation, and
/// the actual build process.
pub async fn build_component(
    ctx: &CliContext,
    config: &Config,
) -> anyhow::Result<ComponentBuildResult> {
    let command = &config
        .build()
        .command
        .ok_or(anyhow!("build.command is required in wash config"))?;
    perform_component_build(ctx, config, command).await
}

pub async fn build_dev_component(
    ctx: &CliContext,
    config: &Config,
) -> anyhow::Result<ComponentBuildResult> {
    let dev_config = config.dev();

    let build_command = if let Some(dev_command) = &dev_config.command {
        dev_command.clone()
    } else {
        config
            .build()
            .command
            .ok_or(anyhow!("build.command is required in wash config"))?
    };
    perform_component_build(ctx, config, &build_command).await
}

/// Build a component at the specified project path
///
/// This is the main public interface for building components that can be reused
/// throughout the project. It handles project detection, tool validation, and
/// the actual build process.
#[instrument(skip(ctx, config), name = "perform_component_build")]
pub async fn perform_component_build(
    ctx: &CliContext,
    config: &Config,
    command: &String,
) -> anyhow::Result<ComponentBuildResult> {
    let skip_fetch = config.wit.as_ref().map(|w| w.skip_fetch).unwrap_or(false);
    let wit_dir = config.wit.as_ref().and_then(|w| w.wit_dir.clone());
    debug!(
        project_path = ?ctx.project_dir().display(),
        wit_dir = ?wit_dir.as_ref().map(|p| p.display()),
        command = %command,
        "building component at specified project path",
    );

    let builder = ComponentBuilder::new(ctx.project_dir().into(), command, wit_dir, skip_fetch);
    builder.build(ctx, config).await
}

/// Component builder that handles the actual build process
#[derive(Debug, Clone)]
pub struct ComponentBuilder {
    project_path: PathBuf,
    wit_dir: Option<PathBuf>,
    skip_wit_fetch: bool,
    command: String,
}

impl ComponentBuilder {
    /// Create a new component builder for the specified project path
    pub fn new(
        project_path: PathBuf,
        command: &str,
        wit_dir: Option<PathBuf>,
        skip_wit_fetch: bool,
    ) -> Self {
        Self {
            project_path,
            wit_dir,
            skip_wit_fetch,
            command: command.to_owned(),
        }
    }

    /// Get the WIT directory, defaulting to project_path/wit if not specified
    fn get_wit_dir(&self) -> PathBuf {
        match &self.wit_dir {
            Some(wit_dir) if wit_dir.is_absolute() => wit_dir.clone(),
            Some(wit_dir) => self.project_path.join(wit_dir),
            None => self.project_path.join("wit"),
        }
    }

    /// Build the component
    #[instrument(level = "debug", skip(self, ctx, config))]
    pub async fn build(
        &self,
        ctx: &CliContext,
        config: &Config,
    ) -> anyhow::Result<ComponentBuildResult> {
        debug!(
            path = ?self.project_path.display(),
            "building component",
        );

        // Validate project path exists
        if !self.project_path.exists() {
            bail!(
                "project path does not exist: {}",
                self.project_path.display()
            );
        }

        // Fetch WIT dependencies if needed
        if !self.skip_wit_fetch {
            debug!("fetching WIT dependencies for project");
            if let Err(e) = self.fetch_wit_dependencies(ctx, config).await {
                error!(err = ?e, "unable to fetch WIT dependencies. If dependencies are already present locally, you can skip this step with --skip-fetch");
                bail!(e);
            }
        } else {
            debug!("skipping WIT dependency fetching as per configuration");
        }

        // Run pre-build hook
        self.run_pre_build_hook().await?;

        info!(path = ?self.project_path.display(), "building component");
        // Build the component using the language toolchain
        let component_path = self.build_component(config).await?;

        // Run post-build hook
        self.run_post_build_hook().await?;

        debug!(
            component_path = ?component_path.display(),
            "component build completed successfully",
        );

        Ok(ComponentBuildResult {
            component_path,
            project_path: self.project_path.clone(),
        })
    }

    /// Fetch WIT dependencies if the project has any
    #[instrument(
        level = "debug",
        skip(self, ctx, config),
        name = "fetch_wit_dependencies"
    )]
    async fn fetch_wit_dependencies(
        &self,
        ctx: &CliContext,
        config: &Config,
    ) -> anyhow::Result<()> {
        let wit_dir = self.get_wit_dir();

        // Check if WIT directory exists - if not, skip dependency fetching
        if !wit_dir.exists() {
            debug!(
                "WIT directory does not exist, skipping dependency fetching: {}",
                wit_dir.display()
            );
            return Ok(());
        }

        debug!(path = ?wit_dir.display(), "fetching WIT dependencies");

        // Create WIT fetcher from configuration
        let mut lock_file = load_lock_file(&self.project_path).await?;
        let args = CommonPackageArgs {
            config: None, // TODO(#1): config
            cache: Some(ctx.cache_dir().join("package_cache")),
        };
        let wkg_config = wasm_pkg_core::config::Config::default();
        let mut fetcher = WkgFetcher::from_common(&args, wkg_config).await?;

        // Apply WIT source overrides if present in configuration
        if let Some(wit_config) = &config.wit
            && !wit_config.sources.is_empty()
        {
            debug!("applying WIT source overrides: {:?}", wit_config.sources);
            fetcher
                .resolve_extended_pull_configs(&wit_config.sources, &self.project_path)
                .await
                .context("failed to resolve WIT source overrides")?;
        }

        // Fetch dependencies
        fetcher
            .fetch_wit_dependencies(&wit_dir, &mut lock_file)
            .await?;

        lock_file
            .write()
            .await
            .context("failed to write lock file")?;

        debug!("WIT dependencies fetched successfully");
        Ok(())
    }

    async fn build_component(&self, config: &Config) -> anyhow::Result<PathBuf> {
        let build_config = config.build.clone().unwrap_or_default();

        let project_dir_name = self
            .project_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("component");

        let component_path = build_config
            .component_path
            .clone()
            .unwrap_or(format!("{}.wasm", project_dir_name).into());

        let (cmd_bin, first_arg) = {
            #[cfg(not(windows))]
            {
                ("sh".to_string(), "-c".to_string())
            }

            #[cfg(windows)]
            {
                ("cmd".to_string(), "/c".to_string())
            }
        };

        let cmd_args = vec![first_arg, self.command.clone()];

        info!(command = self.command,  component_path = ?component_path, "executing build command");
        let mut cmd = Command::new(cmd_bin)
            .envs(&build_config.env)
            .env("WASH_COMPONENT_PATH", &component_path)
            .args(cmd_args)
            .stderr(Stdio::inherit())
            .stdout(Stdio::inherit())
            .current_dir(&self.project_path)
            .spawn()
            .context("failed to execute build command")?;

        let exit_status = cmd
            .wait()
            .await
            .context("failed to wait for build command to complete")?;

        if !exit_status.success() {
            bail!("build command '{}' failed", self.command);
        }

        // Attempt to canonicalize the component path
        let component_path = component_path.canonicalize().unwrap_or(component_path);

        match std::fs::exists(&component_path) {
            Ok(true) => Ok(component_path),
            Ok(false) => {
                anyhow::bail!(
                    "build command completed successfully but component not found at expected path: {}",
                    component_path.display()
                )
            }
            Err(e) => {
                anyhow::bail!(
                    "failed to check if component exists at expected path {}: {}",
                    component_path.display(),
                    e
                )
            }
        }
    }

    /// Placeholder for pre-build hook
    async fn run_pre_build_hook(&self) -> anyhow::Result<()> {
        trace!("running pre-build hook (placeholder)");
        Ok(())
    }

    /// Placeholder for post-build hook  
    async fn run_post_build_hook(&self) -> anyhow::Result<()> {
        trace!("running post-build hook (placeholder)");
        Ok(())
    }
}
