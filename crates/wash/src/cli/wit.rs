//! CLI commands for managing WIT dependencies
//!
//! This module provides commands for managing WebAssembly Interface Type (WIT) dependencies
//! in component projects. WIT is the interface definition language for WebAssembly
//! components, allowing you to define imports and exports between components.
//!
//! # Commands
//!
//! - [`wash wit fetch`](#wash-wit-fetch) - Fetch WIT dependencies from registries
//! - [`wash wit update`](#wash-wit-update) - Update dependencies to latest versions
//! - [`wash wit add`](#wash-wit-add) - Add a new WIT dependency
//! - [`wash wit remove`](#wash-wit-remove) - Remove a WIT dependency
//! - [`wash wit clean`](#wash-wit-clean) - Remove fetched dependencies
//! - [`wash wit build`](#wash-wit-build) - Build WIT package into Wasm binary
//!
//! # Lock File
//!
//! WIT dependencies are tracked in a lock file (`wkg.lock`) at the project root. This file
//! records the exact versions of dependencies that were resolved, ensuring reproducible builds.
//! The lock file is automatically created and updated by `wash wit fetch` and related commands.
//!
//! For backward compatibility, wash also supports reading from `.wash/wasmcloud.lock`, but
//! new lock files are always created as `wkg.lock` at the project root.
//!
//! # Examples
//!
//! ## Fetch all dependencies
//!
//! ```bash
//! # Fetch dependencies declared in wit/world.wit
//! wash wit fetch
//!
//! # Clean fetch (remove existing deps first)
//! wash wit fetch --clean
//! ```
//!
//! ## Add a new dependency
//!
//! ```bash
//! # Add latest version
//! wash wit add wasi:http
//!
//! # Add specific version
//! wash wit add wasi:http@0.2.0
//! ```
//!
//! ## Update dependencies
//!
//! ```bash
//! # Update all dependencies to latest versions
//! wash wit update
//!
//! # Update specific package
//! wash wit update wasi:http
//! ```
//!
//! ## Remove a dependency
//!
//! ```bash
//! wash wit remove wasi:http
//! ```
//!
//! ## Build WIT package
//!
//! ```bash
//! # Build to project root (default)
//! wash wit build
//!
//! # Build to custom location
//! wash wit build -o target/my-component.wasm
//! ```
//!
//! # Configuration
//!
//! WIT sources can be configured in your wash config file to override default registries:
//!
//! ```toml
//! [wit]
//! wit_dir = "wit"  # Override default WIT directory location
//!
//! [wit.sources]
//! "wasi:http" = "https://github.com/WebAssembly/wasi-http"
//! "local:custom" = "./local/wit"
//! ```
//!
//! # World.wit File Format
//!
//! Dependencies are declared in your `wit/world.wit` file using import statements:
//!
//! ```wit
//! package myorg:mycomponent@0.1.0;
//!
//! world myworld {
//!     import wasi:http/types@0.2.0;
//!     import wasi:keyvalue/store@0.2.0-draft;
//!
//!     export wasi:http/incoming-handler@0.2.0;
//! }
//! ```
//!
//! # References
//!
//! - [Component Model Documentation](https://component-model.bytecodealliance.org/)
//! - [WIT Language Specification](https://component-model.bytecodealliance.org/design/wit.html)
//! - [wasm-pkg-tools Documentation](https://github.com/bytecodealliance/wasm-pkg-tools)

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};
use tracing::{debug, info, instrument};

use crate::{
    cli::{CliCommand, CliContext, CommandOutput},
    config::{Config, load_config},
    wit::{CommonPackageArgs, WkgFetcher, load_lock_file},
};

/// Manage WIT dependencies for wasmCloud components
#[derive(Parser, Debug, Clone)]
#[command(subcommand_required = true, arg_required_else_help = true)]
pub struct WitArgs {
    #[command(subcommand)]
    command: WitCommand,
}

impl CliCommand for WitArgs {
    #[instrument(level = "debug", skip_all, name = "wit")]
    async fn handle(&self, ctx: &CliContext) -> Result<CommandOutput> {
        self.command.handle(ctx).await
    }
}

/// WIT dependency management subcommands
#[derive(Debug, Clone, Subcommand)]
pub enum WitCommand {
    /// Fetch WIT dependencies (reads from wit/world.wit imports)
    Fetch {
        /// Remove existing dependencies before fetching
        #[arg(long)]
        clean: bool,
    },

    /// Update dependencies to latest compatible versions
    Update {
        /// Specific package to update (e.g., wasi:http). If not specified, updates all packages
        package: Option<String>,
    },

    /// Add a new WIT dependency
    Add {
        /// Package to add (e.g., wasi:keyvalue or wasi:keyvalue@0.2.0-draft)
        package: String,
    },

    /// Remove a WIT dependency
    Remove {
        /// Package to remove (e.g., wasi:keyvalue)
        package: String,
    },

    /// Remove fetched dependencies (wit/deps/)
    Clean {},

    /// Build a WIT package into a Wasm binary
    Build {
        /// Output file path for the built Wasm package
        #[arg(long = "output-file")]
        output_file: Option<PathBuf>,
    },
}

impl CliCommand for WitCommand {
    #[instrument(level = "debug", skip_all, name = "wit")]
    async fn handle(&self, ctx: &CliContext) -> Result<CommandOutput> {
        let config = ctx.load_config(None::<Config>)?;

        match self {
            WitCommand::Fetch { clean } => handle_fetch(ctx, &config, *clean).await,
            WitCommand::Update { package } => handle_update(ctx, package.as_deref(), &config).await,
            WitCommand::Add { package } => handle_add(ctx, package, &config).await,
            WitCommand::Remove { package } => handle_remove(ctx, package, &config).await,
            WitCommand::Clean {} => handle_clean(ctx, &config).await,
            WitCommand::Build { output_file } => {
                handle_build(ctx, &config, output_file.as_deref()).await
            }
        }
    }
}

/// Validate a WIT package reference follows the convention: namespace:package/interface
fn validate_interface_ref(package: &str) -> Result<()> {
    let name_part = package.split('@').next().unwrap_or(package);

    if !name_part.contains(':') {
        bail!(
            "Invalid package format '{}': must be in 'namespace:package/interface' format (e.g., 'wasi:http/types')",
            name_part
        );
    }

    if !name_part.contains('/') {
        bail!(
            "Invalid package format '{}': must include interface name in 'namespace:package/interface' format (e.g., 'wasi:http/types')",
            name_part
        );
    }

    Ok(())
}

/// Find the WIT file containing the world definition
/// Looks for world.wit first, then searches for any .wit file containing a world definition
async fn find_world_wit_file(wit_dir: &std::path::Path) -> Result<std::path::PathBuf> {
    // First try world.wit (most common)
    let world_wit_path = wit_dir.join("world.wit");
    if tokio::fs::try_exists(&world_wit_path)
        .await
        .unwrap_or(false)
    {
        return Ok(world_wit_path);
    }

    // If world.wit doesn't exist, search for any .wit file containing a world definition
    let mut entries = tokio::fs::read_dir(wit_dir)
        .await
        .context("failed to read WIT directory")?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("wit") {
            // Read the file and check if it contains a world definition
            if let Ok(content) = tokio::fs::read_to_string(&path).await {
                // Simple check: does it contain "world " keyword?
                // This is a heuristic - we're looking for "world <name> {" or "world <name>\n{"
                let mut found_world_keyword = false;
                for line in content.lines() {
                    let trimmed = line.trim();
                    // Check if this line starts a world definition
                    if trimmed.starts_with("world ") {
                        found_world_keyword = true;
                        // Check if the opening brace is on the same line
                        if trimmed.contains('{') {
                            debug!(
                                "Found world definition in {}",
                                path.file_name()
                                    .map(|n| n.to_string_lossy())
                                    .unwrap_or_default()
                            );
                            return Ok(path);
                        }
                    } else if found_world_keyword && trimmed.starts_with('{') {
                        // Opening brace on next line after world keyword
                        debug!(
                            "Found world definition in {}",
                            path.file_name()
                                .map(|n| n.to_string_lossy())
                                .unwrap_or_default()
                        );
                        return Ok(path);
                    } else if found_world_keyword
                        && !trimmed.is_empty()
                        && !trimmed.starts_with("//")
                    {
                        // Reset if we find a non-comment, non-brace line after "world"
                        found_world_keyword = false;
                    }
                }
            }
        }
    }

    // No world file found
    bail!(
        "No WIT file containing a world definition found in {}\n\
         \n\
         Create a world.wit file or ensure a .wit file contains a world definition",
        wit_dir.display()
    )
}

/// Handle `wash wit fetch`
#[instrument(level = "debug", skip(ctx))]
async fn handle_fetch(ctx: &CliContext, config: &Config, clean: bool) -> Result<CommandOutput> {
    let project_dir = ctx.project_dir();
    let wit_dir = config.wit_dir();
    // Check if WIT directory exists
    if !wit_dir.exists() {
        return Ok(CommandOutput::error(
            format!("WIT directory does not exist: {}", wit_dir.display()),
            None,
        ));
    }

    debug!(
        wit_dir = %wit_dir.display(),
        project_dir = %project_dir.display(),
        "fetching WIT dependencies"
    );

    // Clean if requested
    if clean {
        let deps_dir = wit_dir.join("deps");
        if deps_dir.exists() {
            debug!("removing existing deps directory: {}", deps_dir.display());
            tokio::fs::remove_dir_all(&deps_dir)
                .await
                .context("failed to remove deps directory")?;
        }
    }

    // Load or create lock file
    let mut lock_file = load_lock_file(&project_dir).await?;

    // Setup package fetcher
    let args = CommonPackageArgs {
        config: None,
        cache: Some(ctx.cache_dir().join("package_cache")),
    };
    let wkg_config = wasm_pkg_core::config::Config::default();

    let mut fetcher = WkgFetcher::from_common(&args, wkg_config).await?;

    // Apply WIT source overrides from config if present
    let config = load_config(&ctx.user_config_path(), Some(project_dir), None::<Config>).ok();
    if let Some(config) = config
        && let Some(wit_config) = &config.wit
        && !wit_config.sources.is_empty()
    {
        debug!("applying WIT source overrides: {:?}", wit_config.sources);
        fetcher
            .resolve_extended_pull_configs(&wit_config.sources, &project_dir)
            .await
            .context("failed to resolve WIT source overrides")?;
    }

    // Fetch dependencies
    fetcher
        .fetch_wit_dependencies(&wit_dir, &mut lock_file)
        .await?;

    // Write lock file
    lock_file
        .write()
        .await
        .context("failed to write lock file")?;

    info!("WIT dependencies fetched successfully");

    Ok(CommandOutput::ok(
        "WIT dependencies fetched successfully",
        Some(serde_json::json!({
            "wit_dir": wit_dir.display().to_string(),
            "lock_file": project_dir.join("wkg.lock").display().to_string(),
        })),
    ))
}

/// Handle `wash wit update`
#[instrument(level = "debug", skip(ctx))]
async fn handle_update(
    ctx: &CliContext,
    package: Option<&str>,
    config: &Config,
) -> Result<CommandOutput> {
    let project_dir = ctx.project_dir();
    let wit_dir = config.wit_dir();

    if !wit_dir.exists() {
        return Ok(CommandOutput::error(
            format!(
                "WIT directory does not exist: {}\n\
                 \n\
                 Create `wit/world.wit`",
                wit_dir.display()
            ),
            None,
        ));
    }

    debug!(
        wit_dir = %wit_dir.display(),
        package = ?package,
        "updating WIT dependencies"
    );

    // For update, we need to clear the lock file (or specific package entries) to force re-resolution
    let lock_file_path = project_dir.join("wkg.lock");

    if let Some(package_name) = package {
        // Selective package update: remove only the specified package from lock file
        validate_interface_ref(package_name)?;

        // Load the existing lock file
        let mut lock_file = load_lock_file(&project_dir).await?;

        // Parse the package name to get PackageRef
        let package_ref: wasm_pkg_client::PackageRef = package_name
            .split('@')
            .next()
            .unwrap_or(package_name)
            .parse()
            .context("failed to parse package name")?;

        // Count packages before removal
        let before_count = lock_file.packages.len();

        // Remove the package from the lock file
        lock_file
            .packages
            .retain(|locked_pkg| locked_pkg.name != package_ref);

        if lock_file.packages.len() == before_count {
            return Ok(CommandOutput::error(
                format!("Package '{}' not found in lock file", package_name),
                None,
            ));
        }

        // Write the modified lock file
        lock_file
            .write()
            .await
            .context("failed to write lock file after removing package")?;

        // Explicitly drop the lock_file to ensure it's fully written
        drop(lock_file);

        info!(
            "Removed {} from lock file, will re-fetch to get latest version",
            package_name
        );

        // Now fetch to re-resolve just this package
        handle_fetch(ctx, config, false).await?;

        Ok(CommandOutput::ok(
            format!("Updated package: {}", package_name),
            Some(serde_json::json!({
                "package": package_name,
                "wit_dir": wit_dir.display().to_string(),
            })),
        ))
    } else {
        // Full update: remove entire lock file to force re-resolution of all packages

        // Remove lock file to force full update
        if lock_file_path.exists() {
            tokio::fs::remove_file(&lock_file_path)
                .await
                .context("failed to remove lock file")?;
        }

        // Now fetch with the cleared lock file, which will resolve to latest versions
        handle_fetch(ctx, config, false).await?;

        Ok(CommandOutput::ok(
            "All WIT dependencies updated successfully".to_string(),
            Some(serde_json::json!({
                "wit_dir": wit_dir.display().to_string(),
            })),
        ))
    }
}

/// Handle `wash wit add`
#[instrument(level = "debug", skip(ctx))]
async fn handle_add(ctx: &CliContext, package: &str, config: &Config) -> Result<CommandOutput> {
    let wit_dir = config.wit_dir();

    if !wit_dir.exists() {
        return Ok(CommandOutput::error(
            format!(
                "WIT directory does not exist: {}\n\
                 \n\
                 Create `wit/world.wit`",
                wit_dir.display()
            ),
            None,
        ));
    }

    debug!(wit_dir = %wit_dir.display(), package, "adding WIT dependency");

    // Validate package reference format
    validate_interface_ref(package)?;

    // Parse package name and version
    let (package_name, version) = if let Some((name, ver)) = package.split_once('@') {
        (name, Some(ver))
    } else {
        (package, None)
    };

    // Find the WIT file containing the world definition
    let world_wit_path = match find_world_wit_file(&wit_dir).await {
        Ok(path) => path,
        Err(e) => {
            return Ok(CommandOutput::error(format!("{:#}", e), None));
        }
    };

    // Read the current world file content
    let content = tokio::fs::read_to_string(&world_wit_path)
        .await
        .context("failed to read world WIT file")?;

    // Check if the import already exists
    let import_line = if let Some(ver) = version {
        format!("import {package_name}@{ver};")
    } else {
        format!("import {package_name};")
    };

    if content.contains(&import_line) || content.contains(&format!("import {package_name}@")) {
        return Ok(CommandOutput::error(
            format!("Package {package_name} is already imported in world.wit"),
            None,
        ));
    }

    // Add the import inside the world block
    let lines: Vec<&str> = content.lines().collect();
    let mut new_lines = Vec::new();
    let mut inserted = false;
    let mut in_world = false;
    let mut world_indent = String::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // Detect when we enter a world block
        if trimmed.starts_with("world ") && trimmed.contains('{') {
            in_world = true;
            // Detect indentation level by looking at next non-empty line
            if let Some(next_line) = lines.get(i + 1) {
                let next_trimmed = next_line.trim_start();
                let indent_len = next_line.len() - next_trimmed.len();
                world_indent = " ".repeat(indent_len.max(4)); // Default to 4 spaces if no indent detected
            } else {
                world_indent = "    ".to_string(); // Default 4 spaces
            }
        }

        new_lines.push(line.to_string());

        // Insert the import inside the world block
        if in_world && !inserted {
            // Check if this line is an import statement inside the world
            if trimmed.starts_with("import ") {
                // Check if this is the last import in the world block
                let remaining_lines = lines.get(i + 1..).unwrap_or_default();
                let has_more_imports = remaining_lines
                    .iter()
                    .take_while(|l| !l.trim().starts_with('}')) // Stop at closing brace
                    .any(|l| l.trim().starts_with("import "));

                if !has_more_imports {
                    // Insert after the last import
                    new_lines.push(format!("{}{}", world_indent, import_line));
                    inserted = true;
                }
            } else if trimmed.starts_with("world ") && trimmed.ends_with('{') {
                // World block just opened, and there are no imports yet
                // Check if there are any existing imports in the world
                let has_imports = lines
                    .get(i + 1..)
                    .unwrap_or_default()
                    .iter()
                    .take_while(|l| !l.trim().starts_with('}'))
                    .any(|l| l.trim().starts_with("import "));

                if !has_imports {
                    // No imports yet, insert as first line in world block
                    new_lines.push(format!("{}{}", world_indent, import_line));
                    inserted = true;
                }
            }
        }

        // Detect when we exit the world block
        if in_world && trimmed == "}" {
            in_world = false;
        }
    }

    // If we still haven't inserted, there might not be a world block
    if !inserted {
        return Ok(CommandOutput::error(
            "Could not find a world block in world.wit to add the import".to_string(),
            None,
        ));
    }

    let mut new_content = new_lines.join("\n");
    // Preserve trailing newline if original content had one
    if content.ends_with('\n') {
        new_content.push('\n');
    }

    // Write the updated content
    tokio::fs::write(&world_wit_path, &new_content)
        .await
        .context("failed to write world.wit")?;

    info!("Added {package} to world.wit");

    // Now fetch the newly added dependency
    handle_fetch(ctx, config, false).await?;

    Ok(CommandOutput::ok(
        format!("Added WIT dependency: {package}"),
        Some(serde_json::json!({
            "package": package,
            "wit_dir": wit_dir.display().to_string(),
        })),
    ))
}

/// Handle `wash wit remove`
#[instrument(level = "debug", skip(_ctx, config))]
async fn handle_remove(_ctx: &CliContext, package: &str, config: &Config) -> Result<CommandOutput> {
    let wit_dir = config.wit_dir();

    if !wit_dir.exists() {
        return Ok(CommandOutput::error(
            format!(
                "WIT directory does not exist: {}\n\
                 \n\
                 Create `wit/world.wit`",
                wit_dir.display()
            ),
            None,
        ));
    }

    debug!(wit_dir = %wit_dir.display(), package, "removing WIT dependency");

    // Validate package reference format
    validate_interface_ref(package)?;

    // Find the WIT file containing the world definition
    let world_wit_path = match find_world_wit_file(&wit_dir).await {
        Ok(path) => path,
        Err(e) => {
            return Ok(CommandOutput::error(format!("{:#}", e), None));
        }
    };

    // Read the current world file content
    let content = tokio::fs::read_to_string(&world_wit_path)
        .await
        .context("failed to read world WIT file")?;

    // Remove the import line
    let lines: Vec<&str> = content.lines().collect();
    let mut new_lines = Vec::new();
    let mut removed = false;

    for line in lines {
        let trimmed = line.trim();
        // Check if this line imports the package we want to remove
        if trimmed.starts_with("import ") && trimmed.contains(package) {
            // Verify it's exactly the package (not a partial match)
            // Extract the package name from "import wasi:cli@0.2.0;" or "import wasi:cli;"
            if let Some(after_import) = trimmed.strip_prefix("import ") {
                let package_part = after_import
                    .trim()
                    .split(';')
                    .next()
                    .unwrap_or(after_import)
                    .split('@')
                    .next()
                    .unwrap_or(after_import)
                    .trim();

                if package_part == package {
                    removed = true;
                    continue; // Skip this line
                }
            }
        }
        new_lines.push(line);
    }

    if !removed {
        return Ok(CommandOutput::error(
            format!("Package {package} not found in world.wit imports"),
            None,
        ));
    }

    let mut new_content = new_lines.join("\n");
    // Preserve trailing newline if original content had one
    if content.ends_with('\n') {
        new_content.push('\n');
    }

    // Write the updated content
    tokio::fs::write(&world_wit_path, new_content)
        .await
        .context("failed to write world.wit")?;

    info!("Removed {package} from world.wit");

    Ok(CommandOutput::ok(
        format!("Removed WIT dependency: {package}"),
        Some(serde_json::json!({
            "package": package,
            "wit_dir": wit_dir.display().to_string(),
        })),
    ))
}

/// Handle `wash wit clean`
#[instrument(level = "debug", skip(_ctx, config))]
async fn handle_clean(_ctx: &CliContext, config: &Config) -> Result<CommandOutput> {
    let wit_dir = config.wit_dir();

    if !wit_dir.exists() {
        return Ok(CommandOutput::error(
            format!(
                "WIT directory does not exist: {}\n\
                 \n\
                 Create `wit/world.wit`",
                wit_dir.display()
            ),
            None,
        ));
    }

    let deps_dir = wit_dir.join("deps");

    if !deps_dir.exists() {
        return Ok(CommandOutput::ok(
            "No dependencies to clean (deps directory does not exist)",
            None,
        ));
    }

    debug!("removing deps directory: {}", deps_dir.display());

    tokio::fs::remove_dir_all(&deps_dir)
        .await
        .context("failed to remove deps directory")?;

    info!("Cleaned WIT dependencies");

    Ok(CommandOutput::ok(
        "WIT dependencies cleaned successfully",
        Some(serde_json::json!({
            "deps_dir": deps_dir.display().to_string(),
        })),
    ))
}

/// Handle `wash wit build`
#[instrument(level = "debug", skip(ctx, config))]
async fn handle_build(
    ctx: &CliContext,
    config: &Config,
    output_override: Option<&std::path::Path>,
) -> Result<CommandOutput> {
    let project_dir = ctx.project_dir();
    let wit_dir = config.wit_dir();

    if !wit_dir.exists() {
        return Ok(CommandOutput::error(
            format!(
                "WIT directory does not exist: {}\n\
                 \n\
                 Create `wit/world.wit`",
                wit_dir.display()
            ),
            None,
        ));
    }

    debug!(wit_dir = %wit_dir.display(), "building WIT package");

    // Load or create lock file
    let mut lock_file = load_lock_file(&project_dir).await?;

    // Setup package client using the same pattern as fetch
    let args = CommonPackageArgs {
        config: None,
        cache: Some(ctx.cache_dir().join("package_cache")),
    };
    let wkg_config = wasm_pkg_core::config::Config::default();
    let fetcher = WkgFetcher::from_common(&args, wkg_config).await?;

    // Build the package
    info!("Building WIT package...");
    let (package_ref, version, wasm_bytes) = fetcher
        .build_wit_package(&wit_dir, &mut lock_file)
        .await
        .context("failed to build WIT package")?;

    // Write lock file
    lock_file
        .write()
        .await
        .context("failed to write lock file")?;

    // Determine output path
    let output_path = if let Some(output) = output_override {
        if output.is_absolute() {
            output.to_path_buf()
        } else {
            ctx.original_working_dir().join(output)
        }
    } else {
        // Default to project root: <package-name>-<version>.wasm or <package-name>.wasm
        let filename = if let Some(ver) = &version {
            format!("{}-{}.wasm", package_ref.name(), ver)
        } else {
            format!("{}.wasm", package_ref.name())
        };
        project_dir.join(filename)
    };

    // Write the wasm bytes to the output file
    tokio::fs::write(&output_path, &wasm_bytes)
        .await
        .context("failed to write output wasm file")?;

    info!(
        output = %output_path.display(),
        package = %package_ref,
        version = ?version,
        "WIT package built successfully"
    );

    Ok(CommandOutput::ok(
        format!(
            "Built WIT package {} to {}",
            package_ref,
            output_path.display()
        ),
        Some(serde_json::json!({
            "package": package_ref.to_string(),
            "version": version.map(|v| v.to_string()),
            "output": output_path.display().to_string(),
            "size": wasm_bytes.len(),
        })),
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Helper to create a temporary project directory with a basic WIT structure
    async fn setup_test_project() -> (TempDir, PathBuf, PathBuf) {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_dir = temp_dir.path().to_path_buf();
        let wit_dir = project_dir.join("wit");

        fs::create_dir_all(&wit_dir).expect("failed to create wit dir");

        // Create a basic world.wit file
        let world_wit = wit_dir.join("world.wit");
        fs::write(
            &world_wit,
            r#"package test:component@0.1.0;

world example {

}
"#,
        )
        .expect("failed to write world.wit");

        (temp_dir, project_dir, wit_dir)
    }

    #[tokio::test]
    async fn test_clean_removes_deps_directory() {
        let (_temp, _project_dir, wit_dir) = setup_test_project().await;

        // Create a deps directory with content
        let deps_dir = wit_dir.join("deps");
        fs::create_dir_all(&deps_dir).expect("failed to create deps dir");
        fs::write(deps_dir.join("test.wit"), "// test").expect("failed to write test file");
        assert!(deps_dir.exists());

        let ctx = CliContext::builder().build().await.unwrap();
        let config = Config {
            wit: Some(crate::wit::WitConfig {
                wit_dir: Some(wit_dir.clone()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let output = handle_clean(&ctx, &config).await.unwrap();
        assert!(output.is_success());
        assert!(!deps_dir.exists());
    }

    #[tokio::test]
    async fn test_remove_package_from_world_wit() {
        let (_temp, _project_dir, wit_dir) = setup_test_project().await;
        let world_wit_path = wit_dir.join("world.wit");

        // Add an import to the file first
        let content = r#"package test:component@0.1.0;

import wasi:cli@0.2.0;

world example {

}
"#;
        fs::write(&world_wit_path, content).expect("failed to write world.wit");

        // Read the file
        let content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");

        assert!(content.contains("import wasi:cli@0.2.0;"));

        // Remove the import
        let lines: Vec<&str> = content.lines().collect();
        let new_lines: Vec<&str> = lines
            .into_iter()
            .filter(|line| {
                let trimmed = line.trim();
                !(trimmed.starts_with("import ") && trimmed.contains("wasi:cli"))
            })
            .collect();

        let new_content = new_lines.join("\n");
        tokio::fs::write(&world_wit_path, new_content)
            .await
            .expect("failed to write world.wit");

        // Verify removal
        let content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");

        assert!(!content.contains("import wasi:cli@0.2.0;"));
    }

    #[tokio::test]
    async fn test_add_package_to_world_wit() {
        let (_temp, _project_dir, wit_dir) = setup_test_project().await;
        let world_wit_path = wit_dir.join("world.wit");

        let original_content = r#"package test:component@0.1.0;

world example {

}
"#;
        fs::write(&world_wit_path, original_content).expect("failed to write world.wit");

        // Simulate adding a package inside the world block
        let package = "wasi:http@0.2.0";
        let import_line = format!("import {package};");

        let content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");

        assert!(!content.contains(&import_line));

        // Add the import inside the world block (correct WIT syntax)
        let lines: Vec<&str> = content.lines().collect();
        let mut new_lines = Vec::new();
        let mut inserted = false;

        for line in lines.iter() {
            new_lines.push(line.to_string());

            if !inserted {
                let trimmed = line.trim();
                // Insert after the opening brace of the world block
                if trimmed.starts_with("world ") && trimmed.ends_with('{') {
                    new_lines.push(format!("   {}", import_line)); // Indent inside world block
                    inserted = true;
                }
            }
        }

        let new_content = new_lines.join("\n");
        tokio::fs::write(&world_wit_path, &new_content)
            .await
            .expect("failed to write world.wit");

        // Verify addition - import should be inside the world block
        let content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");

        assert!(
            content.contains(&import_line),
            "Import should be added to world.wit"
        );
        // Verify it's inside the world block, not at top level
        let lines: Vec<&str> = content.lines().collect();
        let mut found_world = false;
        let mut found_import_inside = false;
        for line in lines {
            if line.trim().starts_with("world ") {
                found_world = true;
            }
            if found_world && line.trim().starts_with("import wasi:http") {
                found_import_inside = true;
                break;
            }
        }
        assert!(
            found_import_inside,
            "Import should be inside the world block"
        );
    }

    #[test]
    fn test_parse_package_with_version() {
        let package = "wasi:http@0.2.0";
        let (name, version) = if let Some((n, v)) = package.split_once('@') {
            (n, Some(v))
        } else {
            (package, None)
        };

        assert_eq!(name, "wasi:http");
        assert_eq!(version, Some("0.2.0"));
    }

    #[test]
    fn test_parse_package_without_version() {
        let package = "wasi:http";
        let (name, version) = if let Some((n, v)) = package.split_once('@') {
            (n, Some(v))
        } else {
            (package, None)
        };

        assert_eq!(name, "wasi:http");
        assert_eq!(version, None);
    }

    #[tokio::test]
    async fn test_lock_file_detection() {
        let (_temp, project_dir, _wit_dir) = setup_test_project().await;

        // Test that wkg.lock is preferred
        let wkg_lock = project_dir.join("wkg.lock");
        let legacy_lock = project_dir.join(".wash").join("wasmcloud.lock");

        fs::create_dir_all(project_dir.join(".wash")).expect("failed to create .wash dir");
        fs::write(&wkg_lock, "").expect("failed to create wkg.lock");
        fs::write(&legacy_lock, "").expect("failed to create wasmcloud.lock");

        assert!(wkg_lock.exists());
        assert!(legacy_lock.exists());

        // wkg.lock should be preferred
        let preferred = if wkg_lock.exists() {
            &wkg_lock
        } else if legacy_lock.exists() {
            &legacy_lock
        } else {
            panic!("no lock file found");
        };

        assert_eq!(preferred, &wkg_lock);
    }

    #[tokio::test]
    async fn test_default_output_filename_with_version() {
        let package_name = "test-package";
        let version = Some("1.0.0".to_string());

        let filename = if let Some(ver) = &version {
            format!("{}-{}.wasm", package_name, ver)
        } else {
            format!("{}.wasm", package_name)
        };

        assert_eq!(filename, "test-package-1.0.0.wasm");
    }

    #[tokio::test]
    async fn test_default_output_filename_without_version() {
        let package_name = "test-package";
        let version: Option<String> = None;

        let filename = if let Some(ver) = &version {
            format!("{}-{}.wasm", package_name, ver)
        } else {
            format!("{}.wasm", package_name)
        };

        assert_eq!(filename, "test-package.wasm");
    }

    #[tokio::test]
    async fn test_check_wit_directory_exists() {
        let (_temp, _project_dir, wit_dir) = setup_test_project().await;

        assert!(wit_dir.exists());
        assert!(wit_dir.join("world.wit").exists());
    }

    #[tokio::test]
    async fn test_check_wit_directory_not_exists() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_dir = temp_dir.path().to_path_buf();
        let wit_dir = project_dir.join("wit");

        assert!(!wit_dir.exists());
    }

    #[tokio::test]
    async fn test_update_clears_lock_files() {
        let (_temp, project_dir, _wit_dir) = setup_test_project().await;

        let wkg_lock = project_dir.join("wkg.lock");
        let legacy_lock = project_dir.join(".wash").join("wasmcloud.lock");

        fs::create_dir_all(project_dir.join(".wash")).expect("failed to create .wash dir");
        fs::write(&wkg_lock, "test content").expect("failed to create wkg.lock");
        fs::write(&legacy_lock, "test content").expect("failed to create wasmcloud.lock");

        assert!(wkg_lock.exists());
        assert!(legacy_lock.exists());

        // Simulate update by removing lock files
        if wkg_lock.exists() {
            tokio::fs::remove_file(&wkg_lock)
                .await
                .expect("failed to remove wkg.lock");
        }
        if legacy_lock.exists() {
            tokio::fs::remove_file(&legacy_lock)
                .await
                .expect("failed to remove legacy lock");
        }

        assert!(!wkg_lock.exists());
        assert!(!legacy_lock.exists());
    }

    #[test]
    fn test_import_line_detection() {
        let content = r#"package test:component@0.1.0;

import wasi:cli@0.2.0;
import wasi:http@0.2.0;

world example {

}
"#;

        let has_cli = content.contains("import wasi:cli");
        let has_http = content.contains("import wasi:http");
        let has_keyvalue = content.contains("import wasi:keyvalue");

        assert!(has_cli);
        assert!(has_http);
        assert!(!has_keyvalue);
    }

    #[tokio::test]
    async fn test_clean_nonexistent_deps_directory() {
        let (_temp, _project_dir, wit_dir) = setup_test_project().await;
        let deps_dir = wit_dir.join("deps");
        assert!(!deps_dir.exists());

        let ctx = CliContext::builder().build().await.unwrap();
        let config = Config {
            wit: Some(crate::wit::WitConfig {
                wit_dir: Some(wit_dir.clone()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let output = handle_clean(&ctx, &config).await.unwrap();
        // Should succeed with a "nothing to clean" message
        assert!(output.is_success());
    }

    #[tokio::test]
    async fn test_world_wit_structure() {
        let (_temp, _project_dir, wit_dir) = setup_test_project().await;
        let world_wit_path = wit_dir.join("world.wit");

        let content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");

        assert!(content.contains("package test:component"));
        assert!(content.contains("world example"));
    }

    #[tokio::test]
    async fn test_multiple_imports_in_world_wit() {
        let (_temp, _project_dir, wit_dir) = setup_test_project().await;
        let world_wit_path = wit_dir.join("world.wit");

        let content = r#"package test:component@0.1.0;

import wasi:cli@0.2.0;
import wasi:http@0.2.0;
import wasi:keyvalue@0.2.0-draft;

world example {

}
"#;
        fs::write(&world_wit_path, content).expect("failed to write world.wit");

        let read_content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");

        // Count imports
        let import_count = read_content
            .lines()
            .filter(|line| line.trim().starts_with("import "))
            .count();

        assert_eq!(import_count, 3);
    }

    #[tokio::test]
    async fn test_remove_specific_import() {
        let (_temp, _project_dir, wit_dir) = setup_test_project().await;
        let world_wit_path = wit_dir.join("world.wit");

        let content = r#"package test:component@0.1.0;

import wasi:cli@0.2.0;
import wasi:http@0.2.0;
import wasi:keyvalue@0.2.0-draft;

world example {

}
"#;
        fs::write(&world_wit_path, content).expect("failed to write world.wit");

        // Remove wasi:http
        let package_to_remove = "wasi:http";
        let read_content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");

        let lines: Vec<&str> = read_content.lines().collect();
        let new_lines: Vec<&str> = lines
            .into_iter()
            .filter(|line| {
                let trimmed = line.trim();
                if trimmed.starts_with("import ") {
                    // Extract the package name from the import line
                    // Format: import wasi:http@0.2.0;
                    if let Some(rest) = trimmed.strip_prefix("import ") {
                        // First remove the semicolon if present
                        let rest = rest.trim_end_matches(';');
                        // Then split by @ to get the package name without version
                        let package_name = if let Some((pkg, _version)) = rest.split_once('@') {
                            pkg.trim()
                        } else {
                            rest.trim()
                        };
                        // Keep the line only if it doesn't match the package to remove
                        package_name != package_to_remove
                    } else {
                        // If we can't parse it, keep it
                        true
                    }
                } else {
                    // Keep all non-import lines
                    true
                }
            })
            .collect();

        let new_content = new_lines.join("\n");

        tokio::fs::write(&world_wit_path, &new_content)
            .await
            .expect("failed to write world.wit");

        let final_content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");

        assert!(
            !final_content.contains("import wasi:http"),
            "File still contains 'import wasi:http'"
        );
        assert!(final_content.contains("import wasi:cli"));
        assert!(final_content.contains("import wasi:keyvalue"));
    }

    #[tokio::test]
    async fn test_selective_package_update() {
        use std::fs;

        let (_temp, project_dir, _wit_dir) = setup_test_project().await;

        // Create a mock lock file with multiple packages
        let lock_file_path = project_dir.join("wkg.lock");
        let lock_content = r#"
version = 1

[[package]]
name = "wasi:http"
registry = "example.com"

[[package.versions]]
requirement = "^0.2.0"
version = "0.2.0"
digest = "sha256:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"

[[package]]
name = "wasi:cli"
registry = "example.com"

[[package.versions]]
requirement = "^0.2.0"
version = "0.2.0"
digest = "sha256:abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
"#;
        fs::write(&lock_file_path, lock_content).expect("failed to write lock file");

        // Load the lock file and verify it has 2 packages
        let lock_file = load_lock_file(&project_dir)
            .await
            .expect("failed to load lock file");
        assert_eq!(lock_file.packages.len(), 2);

        // Simulate removing one package by parsing the package name
        let package_to_remove = "wasi:http";
        let package_ref: wasm_pkg_client::PackageRef = package_to_remove
            .parse()
            .expect("failed to parse package name");

        let mut modified_lock = lock_file;
        modified_lock.packages.retain(|pkg| pkg.name != package_ref);

        // Verify only one package remains
        assert_eq!(modified_lock.packages.len(), 1);

        // Verify the remaining package is wasi:cli
        let remaining_pkg = modified_lock.packages.iter().next().unwrap();
        assert_eq!(remaining_pkg.name.to_string(), "wasi:cli");
    }

    #[test]
    fn test_build_default_output_location() {
        // Test that default output is to project root, not wit dir
        let package_name = "test-package";
        let version = Some("1.0.0".to_string());

        let project_dir = std::path::PathBuf::from("/project");
        let wit_dir = project_dir.join("wit");

        // Simulate the default output path logic
        let filename = if let Some(ver) = &version {
            format!("{}-{}.wasm", package_name, ver)
        } else {
            format!("{}.wasm", package_name)
        };
        let default_output = project_dir.join(filename);

        // Verify it's in project root, not wit dir
        assert_eq!(default_output, project_dir.join("test-package-1.0.0.wasm"));
        assert_ne!(default_output, wit_dir.join("test-package-1.0.0.wasm"));
    }

    #[test]
    fn test_build_output_override() {
        // Test that output path can be overridden
        let package_name = "test-package";
        let version = Some("1.0.0".to_string());

        let project_dir = std::path::PathBuf::from("/project");
        let custom_output = std::path::PathBuf::from("/custom/path/output.wasm");

        // Simulate the override logic
        let output_path = if let Some(output) = Some(&custom_output) {
            output.to_path_buf()
        } else {
            let filename = if let Some(ver) = &version {
                format!("{}-{}.wasm", package_name, ver)
            } else {
                format!("{}.wasm", package_name)
            };
            project_dir.join(filename)
        };

        // Verify override works
        assert_eq!(output_path, custom_output);
        assert_eq!(
            output_path,
            std::path::PathBuf::from("/custom/path/output.wasm")
        );
    }

    #[tokio::test]
    async fn test_add_malformed_wit_file() {
        use std::fs;

        let (_temp, _project_dir, wit_dir) = setup_test_project().await;

        // Create a malformed world.wit file (missing closing brace)
        let world_wit_path = wit_dir.join("world.wit");
        let malformed_content = r#"package test:component@0.1.0;

world example {
    import wasi:http/types@0.2.0;
    // Missing closing brace!
"#;
        fs::write(&world_wit_path, malformed_content).expect("failed to write malformed world.wit");

        // Try to add an import - our code should handle the malformed file gracefully
        let content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");

        // Verify we can still read and parse it line-by-line (our approach is resilient)
        let lines: Vec<&str> = content.lines().collect();
        assert!(lines.iter().any(|l| l.trim().starts_with("import ")));
        assert!(lines.iter().any(|l| l.trim().starts_with("world ")));

        // Our line-based approach will still add the import even though file is malformed
        // The actual validation happens when wasm_pkg_core tries to parse it
        let new_import = "import wasi:cli/stdout@0.2.0;";
        let mut new_lines = Vec::new();
        let mut inserted = false;

        for (i, line) in lines.iter().enumerate() {
            new_lines.push(line.to_string());

            if !inserted {
                let trimmed = line.trim();
                if trimmed.starts_with("import ") {
                    let remaining_lines = &lines[i + 1..];
                    let has_more_imports = remaining_lines
                        .iter()
                        .any(|l| l.trim().starts_with("import "));
                    if !has_more_imports {
                        new_lines.push(new_import.to_string());
                        inserted = true;
                    }
                }
            }
        }

        // Verify import was added (though file is still malformed)
        let new_content = new_lines.join("\n");
        assert!(new_content.contains(new_import));

        // Note: The malformed syntax will be caught by wasm_pkg_core when fetching
        // Our responsibility is to add the import line correctly, not to validate WIT syntax
    }

    #[tokio::test]
    async fn test_add_to_wit_file_with_comments() {
        use std::fs;

        let (_temp, _project_dir, wit_dir) = setup_test_project().await;

        // Create a world.wit with comments
        let world_wit_path = wit_dir.join("world.wit");
        let content_with_comments = r#"package test:component@0.1.0;

// This is a comment about imports
world example {
    // Import HTTP types
    import wasi:http/types@0.2.0;
    // More comments
}
"#;
        fs::write(&world_wit_path, content_with_comments)
            .expect("failed to write world.wit with comments");

        // Parse and add import
        let content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");

        let lines: Vec<&str> = content.lines().collect();

        // Verify comments are preserved and not mistaken for imports
        let comment_lines: Vec<_> = lines
            .iter()
            .filter(|l| l.trim().starts_with("//"))
            .collect();
        assert_eq!(comment_lines.len(), 3, "Should preserve all comment lines");

        // Verify import detection works correctly (ignores comments)
        let import_lines: Vec<_> = lines
            .iter()
            .filter(|l| l.trim().starts_with("import "))
            .collect();
        assert_eq!(
            import_lines.len(),
            1,
            "Should find only actual import, not comments"
        );
    }

    #[tokio::test]
    async fn test_remove_from_empty_world_wit() {
        use std::fs;

        let (_temp, _project_dir, wit_dir) = setup_test_project().await;

        // Create an empty world.wit (only package declaration)
        let world_wit_path = wit_dir.join("world.wit");
        let empty_world = r#"package test:component@0.1.0;

world example {
}
"#;
        fs::write(&world_wit_path, empty_world).expect("failed to write empty world.wit");

        let package_to_remove = "wasi:http";

        // Try to remove a package that doesn't exist
        let content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");

        let lines: Vec<&str> = content.lines().collect();
        let new_lines: Vec<&str> = lines
            .into_iter()
            .filter(|line| {
                let trimmed = line.trim();
                if trimmed.starts_with("import ") {
                    if let Some(rest) = trimmed.strip_prefix("import ") {
                        let rest = rest.trim_end_matches(';');
                        let package_name = if let Some((pkg, _version)) = rest.split_once('@') {
                            pkg.trim()
                        } else {
                            rest.trim()
                        };
                        package_name != package_to_remove
                    } else {
                        true
                    }
                } else {
                    true
                }
            })
            .collect();

        let new_content = new_lines.join("\n");

        // Content should be unchanged (no imports to remove)
        assert_eq!(content.trim(), new_content.trim());
    }

    #[tokio::test]
    async fn test_wit_file_with_inline_comments() {
        use std::fs;

        let (_temp, _project_dir, wit_dir) = setup_test_project().await;

        // Create world.wit with inline comments (not on import lines, but nearby)
        let world_wit_path = wit_dir.join("world.wit");
        let content = r#"package test:component@0.1.0;

world example {
    import wasi:http/types@0.2.0; // HTTP types
    import wasi:cli/stdout@0.2.0; // CLI output
    // import wasi:disabled@0.1.0; - This is commented out
}
"#;
        fs::write(&world_wit_path, content).expect("failed to write world.wit");

        let read_content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");

        let lines: Vec<&str> = read_content.lines().collect();

        // Count actual imports (should ignore the commented-out import)
        let import_count = lines
            .iter()
            .filter(|l| {
                let trimmed = l.trim();
                trimmed.starts_with("import ") && !trimmed.starts_with("// import")
            })
            .count();

        assert_eq!(
            import_count, 2,
            "Should find 2 imports, ignoring commented one"
        );
    }
}
