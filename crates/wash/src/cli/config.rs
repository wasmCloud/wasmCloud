use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use clap::{Parser, Subcommand};
use humansize::{BINARY, format_size};
use tracing::{info, instrument, warn};

use crate::{
    cli::{CONFIG_DIR_NAME, CliCommand, CliContext, CommandOutput},
    config::{
        Config, generate_default_config, generate_example_config, load_config,
        load_config_from_file,
    },
};

/// Output format for a generated config file.
#[derive(Debug, Clone, Default, clap::ValueEnum)]
pub enum ConfigFormat {
    /// YAML (default)
    #[default]
    #[value(alias = "yml")]
    Yaml,
    /// JSON
    Json,
    /// TOML
    Toml,
}

impl ConfigFormat {
    fn extension(&self) -> &'static str {
        match self {
            ConfigFormat::Yaml => "yaml",
            ConfigFormat::Json => "json",
            ConfigFormat::Toml => "toml",
        }
    }
}

/// View and manage wash configuration
#[derive(Parser, Debug, Clone)]
#[command(subcommand_required = true, arg_required_else_help = true)]
pub struct ConfigArgs {
    #[command(subcommand)]
    command: ConfigCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ConfigCommand {
    /// Initialize a new configuration file for wash
    Init {
        /// Overwrite existing configuration
        #[arg(long)]
        force: bool,

        /// Populate the config with example values for every field
        #[arg(long)]
        example: bool,

        /// Output format for the generated config file
        #[arg(long, default_value = "yaml", value_enum)]
        format: ConfigFormat,

        /// Deprecated
        #[arg(long)]
        global: bool,
    },
    /// Print the current version and local directories used by wash
    Info,
    /// Print the current configuration file for wash
    Show,
    /// Remove wash cache and/or data directories
    Cleanup {
        /// Remove cache directory
        #[arg(long)]
        cache: bool,
        /// Remove data directory
        #[arg(long)]
        data: bool,
        /// Remove all wash directories (cache + data)
        #[arg(long)]
        all: bool,
        /// Show what would be removed without actually deleting
        #[arg(long = "dry-run")]
        dry_run: bool,
    },
    /// Validate the wash configuration (syntax, schema, values, and conflicts)
    Validate {
        /// Path to specific config file to validate (optional)
        #[arg(long)]
        file: Option<PathBuf>,
    },
}

impl CliCommand for ConfigArgs {
    #[instrument(level = "debug", skip_all, name = "config")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        self.command.handle(ctx).await
    }
}

impl CliCommand for ConfigCommand {
    #[instrument(level = "debug", skip_all, name = "config")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        match self {
            ConfigCommand::Init {
                force,
                example,
                format,
                global,
            } => {
                if *global {
                    warn!(
                        "--global is deprecated and has no effect; `wash config init` always writes to the project-local .wash/ directory"
                    );
                }

                cmd_init(ctx, *force, *example, format).await
            }
            ConfigCommand::Info => cmd_info(ctx),
            ConfigCommand::Show => cmd_show(ctx),
            ConfigCommand::Cleanup {
                cache,
                data,
                all,
                dry_run,
            } => cmd_cleanup(ctx, *cache, *data, *all, *dry_run).await,
            ConfigCommand::Validate { file } => cmd_validate(ctx, file.as_deref()).await,
        }
    }
}

async fn cmd_init(
    ctx: &CliContext,
    force: bool,
    example: bool,
    format: &ConfigFormat,
) -> anyhow::Result<CommandOutput> {
    let config_path = ctx
        .project_dir()
        .join(CONFIG_DIR_NAME)
        .join(format!("config.{}", format.extension()));

    if example {
        generate_example_config(&config_path, force)
            .await
            .context("failed to initialize config")?;
    } else {
        generate_default_config(&config_path, force)
            .await
            .context("failed to initialize config")?;
    }

    Ok(CommandOutput::ok(
        "Configuration initialized successfully.".to_string(),
        Some(serde_json::json!({
            "message": "Configuration initialized successfully.",
            "config_path": config_path.display().to_string(),
            "success": true,
        })),
    ))
}

fn cmd_info(ctx: &CliContext) -> anyhow::Result<CommandOutput> {
    let version = env!("CARGO_PKG_VERSION");
    let data_dir = ctx.data_dir().display().to_string();
    let cache_dir = ctx.cache_dir().display().to_string();
    let user_config_dir = ctx.config_dir().display().to_string();
    let user_config_path = ctx.user_config_path().display().to_string();
    let project_path = ctx.project_dir();
    let project_config_path = ctx.project_config_path().display().to_string();

    Ok(CommandOutput::ok(
        format!(
            r"
wash version: {version}
Data directory: {data_dir}
Cache directory: {cache_dir}
User Config directory: {user_config_dir}
User Config path: {user_config_path}
Project Config path: {project_config_path}
                        "
        ),
        Some(serde_json::json!({
            "version": version,
            "data_dir": data_dir,
            "cache_dir": cache_dir,
            "user_config_dir": user_config_dir,
            "user_config_path": user_config_path,
            "project_path": project_path,
            "project_config_path": project_config_path,
        })),
    ))
}

fn cmd_show(ctx: &CliContext) -> anyhow::Result<CommandOutput> {
    let config = load_config(
        &ctx.user_config_path(),
        Some(ctx.project_dir()),
        None::<Config>,
    )?;
    Ok(CommandOutput::ok(
        serde_yaml_ng::to_string(&config).context("failed to serialize config")?,
        Some(serde_json::to_value(&config).context("failed to serialize config")?),
    ))
}

async fn cmd_cleanup(
    ctx: &CliContext,
    cache: bool,
    data: bool,
    all: bool,
    dry_run: bool,
) -> anyhow::Result<CommandOutput> {
    let remove_cache = cache || all;
    let remove_data = data || all;
    if !remove_cache && !remove_data {
        bail!("specify at least one of --cache, --data, or --all");
    }

    let mut targets = Vec::new();
    if remove_cache {
        targets.push(("cache", ctx.cache_dir()));
    }
    if remove_data {
        targets.push(("data", ctx.data_dir()));
    }

    let mut lines = Vec::new();
    let mut entries = Vec::new();
    let mut total_bytes: u64 = 0;

    for (kind, path) in &targets {
        let exists = path.exists();
        let size_bytes = if exists {
            match dir_size(path).await {
                Ok(b) => Some(b),
                Err(e) => {
                    warn!(kind, path = %path.display(), error = ?e, "failed to compute directory size");
                    None
                }
            }
        } else {
            None
        };
        if let Some(b) = size_bytes {
            total_bytes = total_bytes.saturating_add(b);
        }

        let action = match (dry_run, exists) {
            (true, true) => "would remove",
            (true, false) => "would skip (missing)",
            (false, true) => {
                tokio::fs::remove_dir_all(path).await.with_context(|| {
                    format!("failed to remove {kind} directory at {}", path.display())
                })?;
                info!(kind, path = %path.display(), bytes = size_bytes.unwrap_or(0), "removed directory");
                "removed"
            }
            (false, false) => "skipped (missing)",
        };

        let size_label = match size_bytes {
            Some(b) => format!(" ({})", format_size(b, BINARY)),
            None if exists => " (size unknown)".to_string(),
            None => String::new(),
        };
        lines.push(format!(
            "{action} {kind} directory: {}{size_label}",
            path.display()
        ));
        entries.push(serde_json::json!({
            "kind": kind,
            "path": path.display().to_string(),
            "existed": exists,
            "action": action,
            "size_bytes": size_bytes,
            "size_human": size_bytes.map(|b| format_size(b, BINARY)),
        }));
    }

    lines.push(format!(
        "total{}: {}",
        if dry_run { " (would free)" } else { " freed" },
        format_size(total_bytes, BINARY)
    ));

    Ok(CommandOutput::ok(
        lines.join("\n"),
        Some(serde_json::json!({
            "dry_run": dry_run,
            "entries": entries,
            "total_bytes": total_bytes,
            "total_human": format_size(total_bytes, BINARY),
        })),
    ))
}

async fn cmd_validate(ctx: &CliContext, file: Option<&Path>) -> anyhow::Result<CommandOutput> {
    let (config, source_label, source_existed) = match file {
        Some(path) => {
            if !path.exists() {
                bail!("config file does not exist: {}", path.display());
            }
            let cfg = load_config_from_file(path)
                .with_context(|| format!("failed to parse {}", path.display()))?;
            (cfg, path.display().to_string(), true)
        }
        None => {
            let user_path = ctx.user_config_path();
            let project_path = ctx.project_config_path();
            let any_exists = user_path.exists() || project_path.exists();
            let cfg = load_config(&user_path, Some(ctx.project_dir()), None::<Config>)
                .context("failed to load merged configuration")?;
            let label = format!(
                "merged configuration (user: {}, project: {})",
                user_path.display(),
                project_path.display()
            );
            (cfg, label, any_exists)
        }
    };

    let mut lines = Vec::new();
    lines.push(format!("Validating: {source_label}"));
    if !source_existed {
        lines.push("note: no config file found on disk; validating built-in defaults".to_string());
    }

    match config.validate(ctx.project_dir()).await {
        Ok(()) => {
            lines.push("Configuration is valid.".to_string());
            Ok(CommandOutput::ok(
                lines.join("\n"),
                Some(serde_json::json!({
                    "source": source_label,
                    "valid": true,
                })),
            ))
        }
        Err(e) => {
            for line in e.to_string().lines() {
                lines.push(format!("ERROR: {line}"));
            }
            Ok(CommandOutput::error(
                lines.join("\n"),
                Some(serde_json::json!({
                    "source": source_label,
                    "errors": e.to_string().lines().collect::<Vec<_>>(),
                    "valid": false,
                })),
            ))
        }
    }
}

/// Sum the byte size of all files under `path`.
///
/// Runs the synchronous walk on a blocking task so the async runtime is not stalled.
async fn dir_size(path: &Path) -> anyhow::Result<u64> {
    let owned = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut total = 0u64;
        let mut stack = vec![owned];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir)
                .with_context(|| format!("failed to read directory {}", dir.display()))?
            {
                let entry =
                    entry.with_context(|| format!("failed to read entry in {}", dir.display()))?;
                let meta = entry
                    .metadata()
                    .with_context(|| format!("failed to stat {}", entry.path().display()))?;

                if meta.is_dir() && !meta.is_symlink() {
                    stack.push(entry.path());
                } else {
                    total = total.saturating_add(meta.len());
                }
            }
        }
        Ok(total)
    })
    .await
    .context("directory size task panicked")?
}
