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
//! A world imports interfaces, so an interface has to be named. Passing a package on its own
//! lists the interfaces it defines instead of writing anything.
//!
//! ```bash
//! # Add latest version
//! wash wit add wasi:http/types
//!
//! # Add specific version
//! wash wit add wasi:http/types@0.2.0
//!
//! # List the interfaces of a package
//! wash wit add wasi:http
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

use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};
use tracing::{debug, info, instrument, warn};

use crate::{
    cli::{CliCommand, CliContext, CommandOutput},
    config::Config,
    wit::{PackageContents, PackageLookup, WKG_LOCK_FILE_NAME, WkgFetcher, load_lock_file},
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

    /// Add a WIT interface to the world's imports
    Add {
        /// Interface to import (e.g., wasi:keyvalue/store or wasi:keyvalue/store@0.2.0-draft).
        /// Passing a package on its own (e.g., wasi:keyvalue) lists its interfaces
        package: String,
    },

    /// Remove a WIT dependency
    Remove {
        /// Interface (e.g., wasi:keyvalue/store) or package (e.g., wasi:keyvalue) to remove.
        /// A package removes every import and include it contributes to the world
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

/// A WIT reference as accepted on the command line: `namespace:package`, optionally with an
/// `/interface` and an `@version`
#[derive(Debug, PartialEq, Eq)]
struct WitRef {
    /// The `namespace:package` part of the reference
    package: String,
    /// The interface named after the package, if any
    interface: Option<String>,
    /// The version, if any
    version: Option<String>,
}

impl WitRef {
    /// Parse a reference of the form `namespace:package[/interface][@version]`
    fn parse(reference: &str) -> Result<Self> {
        let (name_part, version) = match reference.split_once('@') {
            Some((name, version)) => (name, Some(version.to_string())),
            None => (reference, None),
        };

        let Some((namespace, rest)) = name_part.split_once(':') else {
            bail!(
                "Invalid package format '{name_part}': must be in 'namespace:package' or 'namespace:package/interface' format (e.g., 'wasi:http' or 'wasi:http/types')"
            );
        };

        let (pkg_name, interface) = match rest.split_once('/') {
            Some((pkg_name, interface)) => (pkg_name, Some(interface.to_string())),
            None => (rest, None),
        };

        if namespace.is_empty() || pkg_name.is_empty() {
            bail!(
                "Invalid package format '{name_part}': namespace and package name must be non-empty (e.g., 'wasi:http' or 'wasi:http/types')"
            );
        }

        // A trailing slash names no interface, and `import wasi:http/@0.2.0;` is not WIT
        if interface.as_deref().is_some_and(str::is_empty) {
            bail!(
                "Invalid package format '{name_part}': the interface after '/' is empty (e.g., 'wasi:http/types')"
            );
        }

        Ok(Self {
            package: format!("{namespace}:{pkg_name}"),
            interface,
            version,
        })
    }

    /// The path a world statement names, e.g. `wasi:http/types@0.2.0`, carrying `version`. A
    /// world can only name an interface (or a world, via `include`), so this is `None` for a
    /// package-only reference.
    fn world_path(&self, version: Option<&str>) -> Option<String> {
        let interface = self.interface.as_deref()?;
        Some(match version {
            Some(version) => format!("{}/{interface}@{version}", self.package),
            None => format!("{}/{interface}", self.package),
        })
    }

    /// Whether a path named by a world statement refers to this reference, ignoring versions. A
    /// package-only reference matches everything the package contributes to the world.
    fn matches_path(&self, path: &str) -> bool {
        match self.interface.as_deref() {
            Some(interface) => statement_package(path) == format!("{}/{interface}", self.package),
            None => self.is_same_package(path),
        }
    }

    /// Whether a path named by a world statement comes from this reference's package, whichever
    /// interface or world of it the statement names
    fn is_same_package(&self, path: &str) -> bool {
        let path = statement_package(path);
        path == self.package
            || path
                .strip_prefix(&self.package)
                .is_some_and(|rest| rest.starts_with('/'))
    }
}

/// A statement a world is built out of: `import wasi:http/types@0.2.0;`, the labelled form
/// `import store-a: wasi:keyvalue/store@0.2.0-draft;` that the component model's `(implements ..)`
/// mechanism uses, `include`, `export`, and the `use` an interface is written with.
struct WorldStatement<'a> {
    /// `import`, `include`, `export` or `use`
    keyword: &'a str,
    /// The label a multiplexed import is bound to, if any
    label: Option<&'a str>,
    /// The path the statement names, e.g. `wasi:http/types@0.2.0`
    path: &'a str,
}

impl<'a> WorldStatement<'a> {
    /// Whether this is the kind of statement `wash wit add` writes and `wash wit remove` takes
    /// away. An `export` is something a component implements and a `use` belongs to an interface,
    /// so neither is one of those, whatever package it names.
    fn is_dependency(&self) -> bool {
        matches!(self.keyword, "import" | "include")
    }
}

/// Read a world statement, if the line is one
fn world_statement(line: &str) -> Option<WorldStatement<'_>> {
    let trimmed = line.trim();
    let (keyword, rest) = ["import", "include", "export", "use"]
        .into_iter()
        .find_map(|keyword| Some((keyword, trimmed.strip_prefix(&format!("{keyword} "))?)))?;

    // A trailing line comment sits outside the statement
    let rest = rest.split("//").next().unwrap_or(rest).trim();
    let rest = rest.strip_suffix(';').unwrap_or(rest).trim();

    // `import <label>: <interface>` binds one interface to a label, and the path is what follows
    // the label. A package path has no `:` outside its `namespace:package`, which comes after the
    // label's, so the first `:` is the label's only when a second one follows it.
    let (label, path) = match rest.split_once(':') {
        Some((label, after)) if after.contains(':') => (Some(label.trim()), after.trim()),
        _ => (None, rest),
    };

    Some(WorldStatement {
        keyword,
        label,
        path,
    })
}

/// The path of any statement that uses a package, which is what decides the version in use. An
/// export uses its package just as an import does, and so does a labelled import.
fn any_statement_path(line: &str) -> Option<&str> {
    world_statement(line).map(|statement| statement.path)
}

/// The `namespace:package/name` part of a statement path, without its version. An
/// `include ... with { ... }` carries more than the path, so the path ends at the first space.
fn statement_package(path: &str) -> &str {
    let path = path.split('@').next().unwrap_or(path).trim();
    path.split_whitespace().next().unwrap_or(path)
}

/// The version a statement path carries, e.g. `0.2.0` from `wasi:http/types@0.2.0`. A `use` names
/// what it takes after the version — `use wasi:clocks/wall-clock@0.2.0.{datetime};` — so what
/// follows the `@` is only a version when it reads as one.
fn statement_version(path: &str) -> Option<&str> {
    let (_, after) = path.split_once('@')?;
    let version = after.split_whitespace().next().unwrap_or(after);
    // A `use` names what it takes from the interface after the version
    let version = version
        .split_once(".{")
        .map_or(version, |(version, _)| version);
    semver::Version::parse(version).ok()?;
    Some(version)
}

/// A copy of the source with its comments blanked out, so that scanning never takes a
/// commented-out statement for a real one or writes into the middle of a comment. Every comment
/// character becomes a space, so lines and columns still line up with the original.
///
/// WIT block comments nest, which is what the depth counts.
fn without_comments(content: &str) -> String {
    let mut blanked = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();
    let mut line_comment = false;
    let mut depth = 0usize;

    while let Some(c) = chars.next() {
        if c == '\n' {
            line_comment = false;
            blanked.push(c);
            continue;
        }
        if line_comment || depth > 0 {
            // Nested block comments open and close inside one another
            if depth > 0 && c == '/' && chars.peek() == Some(&'*') {
                chars.next();
                depth += 1;
                blanked.push_str("  ");
                continue;
            }
            if depth > 0 && c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                depth -= 1;
                blanked.push_str("  ");
                continue;
            }
            blanked.push(' ');
            continue;
        }
        match (c, chars.peek()) {
            ('/', Some('/')) => {
                chars.next();
                line_comment = true;
                blanked.push_str("  ");
            }
            ('/', Some('*')) => {
                chars.next();
                depth = 1;
                blanked.push_str("  ");
            }
            _ => blanked.push(c),
        }
    }

    blanked
}

/// The last line a statement occupies, given the line it starts on. A statement runs to its `;`,
/// except for `include ... with { ... }`, which ends with the closing brace and takes no
/// semicolon. A statement that ends neither way is treated as the one line, so that a file this
/// does not understand loses nothing.
fn statement_end(lines: &[&str], start: usize) -> usize {
    let mut depth = 0usize;
    let mut opened = false;

    for (offset, line) in lines.iter().skip(start).enumerate() {
        for c in line.chars() {
            match c {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => {
                    depth = depth.saturating_sub(1);
                    if opened && depth == 0 {
                        return start + offset;
                    }
                }
                ';' if depth == 0 => return start + offset,
                _ => {}
            }
        }
    }

    start
}

/// Whether a line is a doc comment, which belongs to the statement below it
fn is_doc_comment(line: &str) -> bool {
    line.trim_start().starts_with("///")
}

/// The version a new world statement should carry.
///
/// What was asked for wins: a whole version is written as typed, even when the world uses a
/// different one for that package. A partial version like `0.3` asks for anything compatible
/// with it, which is the version the world already uses when that one fits, and the newest
/// `0.3.x` otherwise. Asking for nothing takes the world's version, then the newest.
///
/// The result carries a whole version whenever one can be worked out. WIT has no version ranges,
/// and an import with no version at all does not resolve against a versioned package — `import
/// wasi:clocks/monotonic-clock;` fails the fetch with "package 'wasi:clocks' not found. known
/// packages: wasi:clocks@0.3.1" — so an unversioned statement is only ever written when nothing
/// could be resolved to pin it to.
fn version_to_write(
    world_version: Option<&str>,
    requested: Option<&str>,
    resolved: Option<&semver::Version>,
) -> Option<String> {
    match (requested, world_version) {
        // The version the world already uses satisfies what was asked for, so nothing moves
        (Some(requested), Some(existing)) if version_matches(requested, existing) => {
            Some(existing.to_string())
        }
        // A whole version is written as it was asked for
        (Some(requested), _) if semver::Version::parse(requested).is_ok() => {
            Some(requested.to_string())
        }
        // A partial version is not something a world statement can express, so it becomes the
        // newest version compatible with it
        (Some(_), _) => resolved.map(ToString::to_string),
        (None, Some(existing)) => Some(existing.to_string()),
        (None, None) => resolved.map(ToString::to_string),
    }
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

/// The indentation to write a world's statements at, read from the first line inside it that has
/// any. This reads the original rather than the blanked copy, where a comment is all spaces.
fn indent_inside_world(original: &[&str], world_line: usize) -> String {
    let indent = original
        .iter()
        .skip(world_line + 1)
        .find(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .unwrap_or(4);
    " ".repeat(indent.max(4))
}

fn insert_import_into_world(content: &str, import_line: &str) -> Result<String> {
    // The world block is located in a copy with the comments blanked out, so that a `world` or
    // `import` inside a comment is not taken for the real thing and written into. Blanking keeps
    // lines and columns, so an index into it is an index into the original.
    let blanked = without_comments(content);
    let original: Vec<&str> = content.lines().collect();
    let lines: Vec<&str> = blanked.lines().collect();
    let mut new_lines = Vec::new();
    let mut inserted = false;
    let mut pending_world = false;
    let mut world_indent = String::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        if trimmed.starts_with("world ") {
            pending_world = true;
            if trimmed.contains('{') {
                pending_world = false;
                world_indent = indent_inside_world(&original, i);
            }
        } else if pending_world && trimmed.starts_with('{') {
            pending_world = false;
            world_indent = indent_inside_world(&original, i);
        } else if pending_world && !trimmed.is_empty() && !trimmed.starts_with("//") {
            pending_world = false;
        }

        new_lines.push(original.get(i).copied().unwrap_or(line).to_string());

        if inserted || world_indent.is_empty() {
            continue;
        }

        if trimmed.starts_with("import ") {
            let remaining_lines = lines.get(i + 1..).unwrap_or_default();
            let has_more_imports = remaining_lines
                .iter()
                .take_while(|l| !l.trim().starts_with('}'))
                .any(|l| l.trim().starts_with("import "));

            if !has_more_imports {
                new_lines.push(format!("{world_indent}{import_line}"));
                inserted = true;
            }
        } else if trimmed.starts_with('{')
            || (trimmed.starts_with("world ") && trimmed.ends_with('{'))
        {
            let has_imports = lines
                .get(i + 1..)
                .unwrap_or_default()
                .iter()
                .take_while(|l| !l.trim().starts_with('}'))
                .any(|l| l.trim().starts_with("import "));

            if !has_imports {
                new_lines.push(format!("{world_indent}{import_line}"));
                inserted = true;
            }
        }
    }

    if !inserted {
        bail!("Could not find a world block in world.wit to add the import");
    }

    let mut new_content = new_lines.join("\n");
    if content.ends_with('\n') {
        new_content.push('\n');
    }

    Ok(new_content)
}

/// Handle `wash wit fetch`
#[instrument(level = "debug", skip(ctx))]
async fn handle_fetch(ctx: &CliContext, config: &Config, clean: bool) -> Result<CommandOutput> {
    // Before building a fetcher, which applies the project's source overrides and so downloads
    // and clones the sources they name
    let wit_dir = config.wit_dir();
    if !wit_dir.exists() {
        return Ok(CommandOutput::error(
            format!("WIT directory does not exist: {}", wit_dir.display()),
            None,
        ));
    }

    let fetcher = build_fetcher(ctx, config).await?;
    fetch_with(&fetcher, ctx, config, clean).await
}

/// Fetch a project's WIT dependencies with a fetcher that is already built. Building one applies
/// the project's source overrides, which downloads and clones the sources they name, so a command
/// that already has one hands it on rather than paying for another.
async fn fetch_with(
    fetcher: &WkgFetcher,
    ctx: &CliContext,
    config: &Config,
    clean: bool,
) -> Result<CommandOutput> {
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

    // Fetch dependencies
    if let Err(e) = fetcher
        .fetch_wit_dependencies(&wit_dir, &mut lock_file)
        .await
    {
        // Resolution reports only that it failed, so ask the sources what they have for each
        // package the WIT names and report anything they do not. The underlying error goes with
        // it, since the report is a reading of the WIT rather than of the failure itself.
        if let Some(report) = fetch_failure_report(fetcher, &wit_dir).await {
            return Ok(CommandOutput::error(
                format!("{report}\n\nFetching failed with: {e:#}"),
                None,
            ));
        }
        return Err(e);
    }

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

/// Put the lock file back when an update's fetch does not land. `wash wit update` clears the lock
/// to force re-resolution, so a fetch that fails would otherwise leave the project with nothing
/// pinned and the next successful fetch free to take versions nobody chose.
async fn restore_lock_on_failure(
    lock_file_path: &std::path::Path,
    lock_before: Option<Vec<u8>>,
    fetched: Result<CommandOutput>,
) -> Result<CommandOutput> {
    let landed = matches!(&fetched, Ok(output) if output.is_success());
    if landed {
        return fetched;
    }

    let Some(lock_before) = lock_before else {
        return fetched;
    };
    match tokio::fs::write(lock_file_path, lock_before).await {
        Ok(()) => debug!("update did not land, put {} back", lock_file_path.display()),
        Err(e) => warn!(
            "update did not land and {} could not be put back: {e}",
            lock_file_path.display()
        ),
    }

    fetched
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

    // For update, we need to clear the lock file (or specific package entries) to force
    // re-resolution — and to put back what was there if the fetch that follows does not land, so
    // that a failed update does not leave the project unpinned
    let lock_file_path = project_dir.join(WKG_LOCK_FILE_NAME);
    let versions_before = locked_versions(project_dir).await;
    let lock_before = tokio::fs::read(&lock_file_path).await.ok();

    if let Some(package_name) = package {
        // Selective package update: remove only the specified package from lock file. The lock
        // file records packages, so an interface reference updates the package it belongs to.
        let wit_ref = WitRef::parse(package_name)?;

        // Load the existing lock file
        let mut lock_file = load_lock_file(&project_dir).await?;

        let package_ref: wasm_pkg_client::PackageRef = wit_ref
            .package
            .parse()
            .with_context(|| format!("[{}] is not a valid package name", wit_ref.package))?;

        // Count packages before removal
        let before_count = lock_file.packages.len();

        // Remove the package from the lock file
        lock_file
            .packages
            .retain(|locked_pkg| locked_pkg.name != package_ref);

        if lock_file.packages.len() == before_count {
            return Ok(CommandOutput::error(
                format!("Package '{package_name}' not found in lock file"),
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
        let fetched = handle_fetch(ctx, config, false).await;
        let fetched = restore_lock_on_failure(&lock_file_path, lock_before, fetched).await?;
        if !fetched.is_success() {
            return Ok(fetched);
        }

        let changes = version_changes(&versions_before, &locked_versions(project_dir).await);
        Ok(CommandOutput::ok(
            match changes.is_empty() {
                true => format!("{package_name} is already at the version it resolves to"),
                false => format!("Updated {package_name}\n{}", render_changes(&changes)),
            },
            Some(serde_json::json!({
                "package": package_name,
                "changes": changes,
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
        let fetched = handle_fetch(ctx, config, false).await;
        let fetched = restore_lock_on_failure(&lock_file_path, lock_before, fetched).await?;
        if !fetched.is_success() {
            return Ok(fetched);
        }

        let changes = version_changes(&versions_before, &locked_versions(project_dir).await);
        Ok(CommandOutput::ok(
            match changes.is_empty() {
                true => {
                    "All WIT dependencies are already at the versions they resolve to".to_string()
                }
                false => format!(
                    "Updated WIT dependencies\n{changes}",
                    changes = render_changes(&changes)
                ),
            },
            Some(serde_json::json!({
                "changes": changes,
                "wit_dir": wit_dir.display().to_string(),
            })),
        ))
    }
}

/// The versions a project's lock file pins, keyed by package. A package can be locked to more
/// than one version when the WIT names more than one requirement for it.
async fn locked_versions(project_dir: &std::path::Path) -> BTreeMap<String, Vec<String>> {
    let lock_file = match load_lock_file(project_dir).await {
        Ok(lock_file) => lock_file,
        Err(e) => {
            debug!("no lock file to compare against: {e:#}");
            return BTreeMap::new();
        }
    };

    let mut versions: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for package in &lock_file.packages {
        versions
            .entry(package.name.to_string())
            .or_default()
            .extend(
                package
                    .versions
                    .iter()
                    .map(|locked| locked.version.to_string()),
            );
    }
    for locked in versions.values_mut() {
        locked.sort();
    }
    versions
}

/// What moved between two sets of locked versions, in the shape `cargo update` reports
fn version_changes(
    before: &BTreeMap<String, Vec<String>>,
    after: &BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    let mut changes = Vec::new();
    for (package, versions) in after {
        match before.get(package) {
            Some(previous) if previous == versions => {}
            Some(previous) => changes.push(format!(
                "Updating {package} {} -> {}",
                previous.join(", "),
                versions.join(", ")
            )),
            None => changes.push(format!("Adding {package} {}", versions.join(", "))),
        }
    }
    for (package, versions) in before {
        if !after.contains_key(package) {
            changes.push(format!("Removing {package} {}", versions.join(", ")));
        }
    }
    changes
}

/// Indent a list of changes under the line that introduces them
fn render_changes(changes: &[String]) -> String {
    changes
        .iter()
        .map(|change| format!("  {change}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render the message for a package-only reference, listing what the package defines when it
/// could be loaded
fn package_reference_message(
    wit_ref: &WitRef,
    contents: Result<&PackageContents, &anyhow::Error>,
) -> String {
    let package = &wit_ref.package;
    let mut message = format!(
        "{package} is a package, and a world imports interfaces from a package rather than the package itself.\n"
    );

    let contents = match contents {
        Ok(contents) => contents,
        Err(e) => {
            let version = wit_ref
                .version
                .as_deref()
                .map(|v| format!("@{v}"))
                .unwrap_or_default();
            message.push_str(&format!(
                "\nIts interfaces could not be listed: {e:#}\n\
                 \n\
                 Add one of them instead, for example:\n  wash wit add {package}/<interface>{version}\n"
            ));
            return message;
        }
    };

    if contents.interfaces.is_empty() {
        let version = package_version_suffix(contents);
        message.push_str(&format!("\n{package}{version} defines no interfaces.\n"));
    } else {
        message.push_str(&interface_list(package, contents));
        message.push_str(
            "\nAn interface your component implements belongs in an `export` instead, which is written directly in wit/world.wit.\n",
        );
    }

    if !contents.worlds.is_empty() {
        let version = package_version_suffix(contents);
        message.push_str(
            "\nTo pull in every interface of a world at once, add an `include` to your own world in wit/world.wit:\n",
        );
        for world in &contents.worlds {
            message.push_str(&format!("  include {package}/{world}{version};\n"));
        }
    }

    message
}

/// Render the message for an interface the loaded package does not define, or `None` when it
/// defines it
fn undefined_interface_message(wit_ref: &WitRef, contents: &PackageContents) -> Option<String> {
    let interface = wit_ref.interface.as_deref()?;
    if contents
        .interfaces
        .iter()
        .any(|defined| defined == interface)
    {
        return None;
    }

    let package = &wit_ref.package;
    let version = package_version_suffix(contents);
    let mut message =
        format!("{package}{version} does not define an interface named `{interface}`.\n");
    if contents.interfaces.is_empty() {
        message.push_str(&format!("\n{package}{version} defines no interfaces.\n"));
    } else {
        message.push_str(&interface_list(package, contents));
    }
    Some(message)
}

/// The `@version` suffix to print for a loaded package, empty when it is unversioned
fn package_version_suffix(contents: &PackageContents) -> String {
    contents
        .version
        .as_ref()
        .map(|version| format!("@{version}"))
        .unwrap_or_default()
}

/// The `wash wit add` line for each interface a package defines
fn interface_list(package: &str, contents: &PackageContents) -> String {
    let version = package_version_suffix(contents);
    let mut list = format!("\nInterfaces in {package}{version}:\n");
    for interface in &contents.interfaces {
        list.push_str(&format!("  wash wit add {package}/{interface}{version}\n"));
    }
    list
}

/// Load a package from the project's configured sources and report what it defines
async fn load_package(
    fetcher: &WkgFetcher,
    wit_ref: &WitRef,
    version: Option<&str>,
    lock: Option<&wasm_pkg_core::lock::LockFile>,
) -> PackageLookup {
    match wit_ref.package.parse::<wasm_pkg_client::PackageRef>() {
        Ok(package) => fetcher.load_package(&package, version, lock).await,
        Err(e) => PackageLookup::Missing(format!(
            "{} is not a valid package name: {e}",
            wit_ref.package
        )),
    }
}

/// Whether two versions sit in the same major.minor line, which is the line WIT treats as one
/// package: `0.2.3` and `0.2.6` are the same package at two versions, while `0.2.3` and `0.3.0`
/// are separate packages that can both appear in a world
fn same_version_line(one: &str, other: &str) -> bool {
    match (semver::Version::parse(one), semver::Version::parse(other)) {
        (Ok(one), Ok(other)) => (one.major, one.minor) == (other.major, other.minor),
        _ => false,
    }
}

/// Whether a requested version, which may be partial like `0.2`, is the version already in use
fn version_matches(requested: &str, existing: &str) -> bool {
    match (
        semver::VersionReq::parse(&format!("={requested}")),
        semver::Version::parse(existing),
    ) {
        (Ok(requirement), Ok(version)) => requirement.matches(&version),
        // Whatever semver cannot read is compared as written
        _ => requested == existing,
    }
}

/// The report to show for a fetch that failed, when the WIT turns out to name something its
/// sources do not have or to name one package twice. `None` when nothing is found, in which case
/// the fetch failed for a reason its own error describes.
async fn fetch_failure_report(fetcher: &WkgFetcher, wit_dir: &std::path::Path) -> Option<String> {
    let problems = fetcher
        .diagnose_wit(wit_dir)
        .await
        .inspect_err(|e| debug!("could not check the WIT directory's packages: {e:#}"))
        .ok()?;

    match problems.is_empty() {
        true => None,
        false => Some(problems.join("\n\n")),
    }
}

/// Build a package fetcher for the project, with the project's `[wit]` configuration applied
async fn build_fetcher(ctx: &CliContext, config: &Config) -> Result<WkgFetcher> {
    let project_dir = ctx.project_dir();
    let mut fetcher =
        WkgFetcher::for_project(ctx.cache_dir().join("package_cache"), &project_dir).await?;
    if let Some(wit_config) = &config.wit {
        fetcher.apply_wit_config(wit_config, &project_dir).await?;
    }
    Ok(fetcher)
}

/// Put `content` in the world file, and take it back out again if that is what stops the WIT
/// parsing. Returns the message to show when the edit was refused.
///
/// The check runs against the file in place rather than the new text on its own, because a world
/// is parsed together with the rest of its package — an `interface` in a sibling file, a `use` of
/// it — so the text alone says little. A WIT directory that did not parse before the edit is left
/// to the edit: refusing there would stand in the way of the change that repairs it.
async fn write_world_wit(
    world_wit_path: &std::path::Path,
    wit_dir: &std::path::Path,
    previous: &str,
    content: &str,
) -> Result<Option<String>> {
    let parsed_before = wit_parses(wit_dir).await;

    tokio::fs::write(world_wit_path, content)
        .await
        .context("failed to write world.wit")?;

    let Err(e) = parse_wit(wit_dir).await else {
        return Ok(None);
    };

    if !parsed_before {
        warn!(
            "{} still does not parse, which it did not before this either: {e:#}",
            wit_dir.display()
        );
        return Ok(None);
    }

    tokio::fs::write(world_wit_path, previous)
        .await
        .context("failed to put world.wit back")?;

    Ok(Some(format!(
        "{} was left as it was: the edit would have stopped it parsing.\n\
         \n\
         {e:#}",
        world_wit_path.display()
    )))
}

/// Parse a WIT directory, which is what every later `wash wit fetch` and `wash build` starts with
async fn parse_wit(wit_dir: &std::path::Path) -> Result<()> {
    let wit_dir = wit_dir.to_path_buf();
    tokio::task::spawn_blocking(move || wasm_pkg_core::wit::get_packages(&wit_dir))
        .await
        .context("failed to parse the WIT directory")??;
    Ok(())
}

/// Whether a WIT directory parses
async fn wit_parses(wit_dir: &std::path::Path) -> bool {
    parse_wit(wit_dir).await.is_ok()
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

    let wit_ref = WitRef::parse(package)?;

    // Find the WIT file containing the world definition
    let world_wit_path = match find_world_wit_file(&wit_dir).await {
        Ok(path) => path,
        Err(e) => {
            return Ok(CommandOutput::error(format!("{e:#}"), None));
        }
    };

    // Read the current world file content
    let content = tokio::fs::read_to_string(&world_wit_path)
        .await
        .context("failed to read world WIT file")?;

    // What the world says is read from a copy with the comments blanked out, so a commented-out
    // import counts for nothing
    let blanked = without_comments(&content);

    // An interface already imported is answered from the file alone, before the package is looked
    // up: the interface counts as imported whatever version it is pinned to
    if wit_ref.interface.is_some()
        && let Some(existing) = blanked
            .lines()
            .filter_map(world_statement)
            // A labelled import binds one more copy of an interface, so it is not the plain
            // import of it that is being asked for here
            .filter(|statement| statement.is_dependency() && statement.label.is_none())
            .map(|statement| statement.path)
            .find(|path| wit_ref.matches_path(path))
    {
        return Ok(CommandOutput::error(
            format!("{existing} is already imported in world.wit"),
            None,
        ));
    }

    // The version the world already uses for this package, which is what an unversioned or
    // compatible reference resolves to. An export counts: a component that exports
    // `wasi:http/incoming-handler@0.2.0` is using wasi:http at 0.2.0. Statements that name the
    // package without a version say nothing about which one, so they are passed over rather than
    // taken as an answer.
    let world_version = blanked
        .lines()
        .filter_map(any_statement_path)
        .filter(|path| wit_ref.is_same_package(path))
        .find_map(statement_version)
        .map(ToString::to_string);

    // The version to look the package up at, which is the one that will be written when it can be
    // pinned down before the lookup
    let requested_version = match (wit_ref.version.as_deref(), world_version.as_deref()) {
        (Some(requested), Some(existing)) if version_matches(requested, existing) => {
            Some(existing.to_string())
        }
        (Some(requested), _) => Some(requested.to_string()),
        (None, existing) => existing.map(ToString::to_string),
    };

    // The world statement is only written once the package is known to exist and to define the
    // named interface: a world imports an interface rather than a whole package, and an import
    // that names something the source does not have fails every later fetch and build.
    //
    // One fetcher for the lookup and the fetch that follows it: building one applies the project's
    // source overrides, which downloads and clones the sources they name. The lock file goes into
    // the lookup so it selects the version the fetch would, and is dropped before that fetch takes
    // it for writing.
    let fetcher = build_fetcher(ctx, config).await?;
    let lookup = {
        let lock = load_lock_file(&ctx.project_dir()).await?;
        load_package(
            &fetcher,
            &wit_ref,
            requested_version.as_deref(),
            Some(&lock),
        )
        .await
    };

    let world_path = match lookup {
        PackageLookup::Missing(message) => return Ok(CommandOutput::error(message, None)),
        // Whatever went wrong, the source could not say whether this interface exists, and an
        // import of something that does not is what this command is here to avoid writing
        PackageLookup::Failed(e) => {
            return Ok(CommandOutput::error(
                match wit_ref.interface.is_some() {
                    true => format!(
                        "{package} could not be loaded, so it was not added to world.wit: {e:#}"
                    ),
                    false => package_reference_message(&wit_ref, Err(&e)),
                },
                None,
            ));
        }
        PackageLookup::Found(contents) => {
            let version = version_to_write(
                world_version.as_deref(),
                wit_ref.version.as_deref(),
                contents.version.as_ref(),
            );

            // A version that was asked for is written as asked for, so a source that does not
            // have it is said so rather than written down. On the registry path the version was
            // already checked, so this is what catches a local or overridden source.
            if let Some(requested) = wit_ref.version.as_deref()
                && let Some(loaded) = contents.version.as_ref()
                && !version_matches(requested, &loaded.to_string())
            {
                return Ok(CommandOutput::error(
                    format!(
                        "{} provides {loaded}, not the requested version [{requested}].",
                        wit_ref.package
                    ),
                    None,
                ));
            }

            match wit_ref.world_path(version.as_deref()) {
                None => {
                    return Ok(CommandOutput::error(
                        package_reference_message(&wit_ref, Ok(&contents)),
                        None,
                    ));
                }
                Some(world_path) => {
                    if let Some(message) = undefined_interface_message(&wit_ref, &contents) {
                        return Ok(CommandOutput::error(message, None));
                    }
                    world_path
                }
            }
        }
    };

    // A version that was asked for wins over the one the world uses, which leaves the world
    // naming that package twice
    if let Some(existing) = world_version.as_deref()
        && let Some(written) = statement_version(&world_path)
        && written != existing
    {
        let package = &wit_ref.package;
        match same_version_line(written, existing) {
            // Two versions in one major.minor line are the same package to WIT, and it cannot
            // hold both
            true => warn!(
                "world.wit imports {package}@{existing} elsewhere, which cannot resolve alongside \
                 {written}: move the other imports to {written} too, or add this one at {existing}"
            ),
            // Different major.minor versions are separate packages, which WIT resolves side by
            // side — but a fetch only resolves one version per package name
            false => warn!(
                "world.wit imports {package}@{existing} elsewhere: a fetch resolves one version \
                 per package name, so only one of {existing} and {written} will reach wit/deps"
            ),
        }
    }

    let import_line = format!("import {world_path};");
    let new_content = match insert_import_into_world(&content, &import_line) {
        Ok(content) => content,
        Err(e) => return Ok(CommandOutput::error(e.to_string(), None)),
    };

    // Write the updated content, and only keep it if the WIT still parses with it in place
    if let Some(refused) =
        write_world_wit(&world_wit_path, &wit_dir, &content, &new_content).await?
    {
        return Ok(CommandOutput::error(refused, None));
    }

    info!("Added {world_path} to world.wit");

    // Now fetch the newly added dependency. The world has already been edited, so a fetch that
    // fails is reported against that rather than hidden behind a success.
    let fetch_context = || {
        format!(
            "`{import_line}` was added to {}, but fetching the WIT dependencies of the world it \
             now describes failed",
            world_wit_path.display()
        )
    };
    let fetched = fetch_with(&fetcher, ctx, config, false)
        .await
        .with_context(fetch_context)?;
    if !fetched.is_success() {
        let (message, _) = fetched.render();
        return Ok(CommandOutput::error(
            format!("{}\n\n{message}", fetch_context()),
            None,
        ));
    }

    Ok(CommandOutput::ok(
        format!("Added WIT dependency: {world_path}"),
        Some(serde_json::json!({
            "package": package,
            "import": world_path,
            "wit_dir": wit_dir.display().to_string(),
        })),
    ))
}

/// Handle `wash wit remove`
#[instrument(level = "debug", skip(ctx, config))]
async fn handle_remove(ctx: &CliContext, package: &str, config: &Config) -> Result<CommandOutput> {
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

    let wit_ref = WitRef::parse(package)?;

    // Find the WIT file containing the world definition
    let world_wit_path = match find_world_wit_file(&wit_dir).await {
        Ok(path) => path,
        Err(e) => {
            return Ok(CommandOutput::error(format!("{e:#}"), None));
        }
    };

    // Read the current world file content
    let content = tokio::fs::read_to_string(&world_wit_path)
        .await
        .context("failed to read world WIT file")?;

    // Statements are found in a copy with the comments blanked out, so a commented-out import is
    // not mistaken for a real one, and taken out of the original by line
    let blanked = without_comments(&content);
    let blanked: Vec<&str> = blanked.lines().collect();
    let lines: Vec<&str> = content.lines().collect();

    let mut new_lines: Vec<&str> = Vec::new();
    let mut removed = false;
    let mut labelled = Vec::new();
    let mut line_number = 0;

    while line_number < lines.len() {
        let statement = blanked
            .get(line_number)
            .and_then(|line| world_statement(line))
            .filter(|statement| statement.is_dependency() && wit_ref.matches_path(statement.path));

        let Some(statement) = statement else {
            new_lines.extend(lines.get(line_number));
            line_number += 1;
            continue;
        };

        // A labelled import binds one more copy of an interface under a name the component's
        // bindings use. It is not what `wash wit add` writes, so it is not what this takes
        // away — it is reported instead.
        if let Some(label) = statement.label {
            labelled.push(format!("{label}: {}", statement.path));
            new_lines.extend(lines.get(line_number));
            line_number += 1;
            continue;
        }

        removed = true;
        // A doc comment belongs to the statement below it, so it goes with the statement rather
        // than staying behind to document whatever follows
        while new_lines.last().is_some_and(|line| is_doc_comment(line)) {
            new_lines.pop();
        }
        line_number = statement_end(&blanked, line_number) + 1;
    }

    if !removed {
        return Ok(CommandOutput::error(
            match labelled.is_empty() {
                true => format!("Package {package} not found in world.wit imports"),
                false => format!(
                    "{package} is only imported under a label in world.wit, which \
                     `wash wit remove` leaves alone:\n{}\n\
                     \n\
                     Those bind interfaces to names the component's bindings use; take them out \
                     of world.wit by hand.",
                    labelled
                        .iter()
                        .map(|statement| format!("  import {statement};"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            },
            None,
        ));
    }

    let mut new_content = new_lines.join("\n");
    // Preserve trailing newline if original content had one
    if content.ends_with('\n') {
        new_content.push('\n');
    }

    // Write the updated content, and only keep it if the WIT still parses with it in place
    if let Some(refused) =
        write_world_wit(&world_wit_path, &wit_dir, &content, &new_content).await?
    {
        return Ok(CommandOutput::error(refused, None));
    }

    info!("Removed {package} from world.wit");

    // Take the removed package out of `wit/deps` directly, so the removal holds even when the
    // fetch below cannot run. The fetch rewrites `wit/deps` from the resolution and prunes it
    // too, but only when it gets that far.
    let deps_dir = wit_dir.join("deps");
    let dropped = remove_package_deps(&deps_dir, &wit_ref.package).await;
    if dropped > 0 {
        debug!(
            "removed {dropped} director(ies) from {}",
            deps_dir.display()
        );
    }

    // Re-fetch so the lock file stops pinning what the world no longer names, and so anything
    // still named is back in place. A project that has never fetched has nothing to bring back.
    let nothing_fetched =
        !deps_dir.exists() && !ctx.project_dir().join(WKG_LOCK_FILE_NAME).exists();
    let refetched = match nothing_fetched {
        true => true,
        false => match handle_fetch(ctx, config, false).await {
            Ok(output) if output.is_success() => true,
            // The world has already been edited, so the removal stands and the fetch failure is
            // reported rather than hidden
            Ok(output) => {
                warn!(
                    "removed {package}, but the re-fetch that follows it failed: {}",
                    output.render().0
                );
                false
            }
            Err(e) => {
                warn!("removed {package}, but the re-fetch that follows it failed: {e:#}");
                false
            }
        },
    };

    let mut message = match refetched {
        true => format!("Removed WIT dependency: {package}"),
        false => format!(
            "Removed WIT dependency: {package}\n\n\
             The lock file still pins it; run `wash wit fetch` once world.wit resolves again."
        ),
    };
    if !labelled.is_empty() {
        message.push_str(&format!(
            "\n\n{} labelled import(s) of it are still in world.wit, which this leaves alone.",
            labelled.len()
        ));
    }

    let data = serde_json::json!({
        "package": package,
        "refetched": refetched,
        "labelled_kept": labelled,
        "wit_dir": wit_dir.display().to_string(),
    });

    match refetched {
        true => Ok(CommandOutput::ok(message, Some(data))),
        // world.wit no longer agrees with wit/deps and the lock file, which a `&&` in a script
        // needs to hear about
        false => Ok(CommandOutput::error(message, Some(data))),
    }
}

/// Remove a package's directories from `wit/deps`, which wkg names `<namespace>-<name>` with the
/// version appended. Returns how many were removed.
async fn remove_package_deps(deps_dir: &std::path::Path, package: &str) -> usize {
    let prefix = package.replace(':', "-");
    let Ok(mut entries) = tokio::fs::read_dir(deps_dir).await else {
        return 0;
    };

    let mut removed = 0;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        // `wasi-http-0.2.0` belongs to wasi:http; `wasi-http-extra-0.2.0` does not, so whatever
        // follows the name has to be a version
        let belongs = name == prefix
            || name
                .strip_prefix(&format!("{prefix}-"))
                .is_some_and(|rest| semver::Version::parse(rest).is_ok());
        if !belongs {
            continue;
        }
        if let Err(e) = tokio::fs::remove_dir_all(entry.path()).await {
            warn!("failed to remove {}: {e}", entry.path().display());
            continue;
        }
        removed += 1;
    }
    removed
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

    // Setup package client and apply the project's `[wit]` config, matching `wash wit fetch`
    let mut fetcher =
        WkgFetcher::for_project(ctx.cache_dir().join("package_cache"), project_dir).await?;
    if let Some(wit_config) = &config.wit {
        fetcher.apply_wit_config(wit_config, project_dir).await?;
    }

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
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;
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

    /// The path of a statement `wash wit add` writes or `wash wit remove` takes away
    fn dependency_path(line: &str) -> Option<&str> {
        world_statement(line)
            .filter(WorldStatement::is_dependency)
            .map(|statement| statement.path)
    }

    /// A [`CliContext`] rooted in a test project, so that nothing a command does — a lock file,
    /// a `deps` directory — lands outside its temporary directory. The process working directory
    /// stays where it is, since every other test in this binary is using it.
    async fn test_ctx(project_dir: &Path) -> CliContext {
        CliContext::builder()
            .non_interactive(true)
            .project_dir(project_dir.to_path_buf())
            .keep_working_dir()
            .build()
            .await
            .expect("failed to build CLI context")
    }

    /// A [`Config`] pointing at a test project's WIT directory
    fn test_config(wit_dir: &Path) -> Config {
        Config {
            wit: Some(crate::wit::WitConfig {
                wit_dir: Some(wit_dir.to_path_buf()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// Assert that a WIT file parses, which is what `wash wit fetch` and `wash wit build` do
    /// before anything else
    fn assert_parses(world_wit_path: &Path) {
        if let Err(e) = wasm_pkg_core::wit::get_packages(world_wit_path) {
            let content = fs::read_to_string(world_wit_path).unwrap_or_default();
            panic!("WIT should parse but did not: {e:#}\n{content}");
        }
    }

    #[tokio::test]
    async fn test_clean_removes_deps_directory() {
        let (_temp, project_dir, wit_dir) = setup_test_project().await;

        // Create a deps directory with content
        let deps_dir = wit_dir.join("deps");
        fs::create_dir_all(&deps_dir).expect("failed to create deps dir");
        fs::write(deps_dir.join("test.wit"), "// test").expect("failed to write test file");
        assert!(deps_dir.exists());

        let ctx = test_ctx(&project_dir).await;
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
        let (_temp, project_dir, wit_dir) = setup_test_project().await;
        let world_wit_path = wit_dir.join("world.wit");

        let content = r#"package test:component@0.1.0;

world example {
    import wasi:cli/stdout@0.2.0;
    import wasi:http/types@0.2.0;
}
"#;
        fs::write(&world_wit_path, content).expect("failed to write world.wit");

        let config = test_config(&wit_dir);
        let ctx = test_ctx(&project_dir).await;

        let output = handle_remove(&ctx, "wasi:cli/stdout", &config)
            .await
            .expect("handle_remove should not return Err");
        assert!(output.is_success());

        let content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");

        assert!(!content.contains("import wasi:cli/stdout@0.2.0;"));
        assert!(content.contains("import wasi:http/types@0.2.0;"));
        assert_parses(&world_wit_path);
    }

    #[tokio::test]
    async fn test_remove_package_removes_every_interface_of_that_package() {
        let (_temp, project_dir, wit_dir) = setup_test_project().await;
        let world_wit_path = wit_dir.join("world.wit");

        let content = r#"package test:component@0.1.0;

world example {
    import wasi:keyvalue/store@0.2.0-draft;
    import wasi:keyvalue/atomics@0.2.0-draft;
    import wasi:http/types@0.2.0;
}
"#;
        fs::write(&world_wit_path, content).expect("failed to write world.wit");

        let config = test_config(&wit_dir);
        let ctx = test_ctx(&project_dir).await;

        let output = handle_remove(&ctx, "wasi:keyvalue", &config)
            .await
            .expect("handle_remove should not return Err");
        assert!(output.is_success());

        let content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");

        assert!(
            !content.contains("wasi:keyvalue"),
            "both wasi:keyvalue interfaces should be gone: {content}"
        );
        assert!(content.contains("import wasi:http/types@0.2.0;"));
        assert_parses(&world_wit_path);
    }

    #[tokio::test]
    async fn test_an_edit_that_would_not_parse_is_refused() {
        let (_temp, _project_dir, wit_dir) = setup_test_project().await;
        let world_wit_path = wit_dir.join("world.wit");

        let content = r#"package test:component@0.1.0;

world example {
    import wasi:cli/stdout@0.2.0;
}
"#;
        fs::write(&world_wit_path, content).expect("failed to write world.wit");

        // The shape `wash wit add` used to write: a world imports interfaces, not packages
        let broken = r#"package test:component@0.1.0;

world example {
    import wasi:cli/stdout@0.2.0;
    import wasmcloud:messaging;
}
"#;
        let refused = write_world_wit(&world_wit_path, &wit_dir, content, broken)
            .await
            .expect("the guard should not return Err")
            .expect("WIT that does not parse should be refused");

        assert!(refused.contains("was left as it was"), "{refused}");
        assert!(
            refused.contains("messaging"),
            "the reason it does not parse comes with it: {refused}"
        );

        let after = fs::read_to_string(&world_wit_path).expect("failed to read world.wit");
        assert_eq!(after, content, "world.wit should be back as it was");

        // What does parse is kept
        let valid = r#"package test:component@0.1.0;

world example {
    import wasi:cli/stdout@0.2.0;
    import wasmcloud:messaging/consumer@0.2.0;
}
"#;
        assert_eq!(
            write_world_wit(&world_wit_path, &wit_dir, content, valid)
                .await
                .expect("the guard should not return Err"),
            None
        );
        let after = fs::read_to_string(&world_wit_path).expect("failed to read world.wit");
        assert_eq!(after, valid);
    }

    #[tokio::test]
    async fn test_an_edit_to_wit_that_already_does_not_parse_is_kept() {
        let (_temp, project_dir, wit_dir) = setup_test_project().await;
        let world_wit_path = wit_dir.join("world.wit");

        // The shape `wash wit add` used to write: a world cannot import a package, so this file
        // does not parse before the edit either
        let content = r#"package test:component@0.1.0;

world example {
    import wasmcloud:messaging;
    import wasi:cli/stdout@0.2.0;
}
"#;
        fs::write(&world_wit_path, content).expect("failed to write world.wit");

        let config = test_config(&wit_dir);
        let ctx = test_ctx(&project_dir).await;

        let output = handle_remove(&ctx, "wasmcloud:messaging", &config)
            .await
            .expect("handle_remove should not return Err");
        assert!(output.is_success());

        // Refusing here would stand in the way of the edit that repairs the file
        let after = fs::read_to_string(&world_wit_path).expect("failed to read world.wit");
        assert!(!after.contains("wasmcloud:messaging"), "{after}");
        assert!(after.contains("import wasi:cli/stdout@0.2.0;"));
    }

    #[tokio::test]
    async fn test_remove_prunes_stale_deps() {
        let (_temp, project_dir, wit_dir) = setup_test_project().await;

        // A local source keeps this off the network
        let source_dir = project_dir.join("local-wit");
        fs::create_dir_all(&source_dir).expect("failed to create local source dir");
        fs::write(
            source_dir.join("greet.wit"),
            "package test:dep@0.1.0;\n\ninterface greet {\n  hello: func() -> string;\n}\n",
        )
        .expect("failed to write the local package");

        fs::write(
            wit_dir.join("world.wit"),
            "package test:component@0.1.0;\n\nworld example {\n    import test:dep/greet@0.1.0;\n}\n",
        )
        .expect("failed to write world.wit");

        let config = Config {
            wit: Some(crate::wit::WitConfig {
                wit_dir: Some(wit_dir.clone()),
                sources: HashMap::from([("test:dep".to_string(), "local-wit".to_string())]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let ctx = test_ctx(&project_dir).await;

        let output = handle_fetch(&ctx, &config, false)
            .await
            .expect("fetch should not return Err");
        assert!(output.is_success(), "fetch from a local source should work");

        // Something the world does not name, of the kind left behind by an earlier edit
        let deps_dir = wit_dir.join("deps");
        let stale_dir = deps_dir.join("stale-pkg-0.1.0");
        fs::create_dir_all(&stale_dir).expect("failed to create the stale dep");
        fs::write(
            stale_dir.join("package.wit"),
            "package stale:pkg@0.1.0;\n\ninterface gone {\n  ping: func();\n}\n",
        )
        .expect("failed to write the stale dep");

        let output = handle_remove(&ctx, "test:dep", &config)
            .await
            .expect("handle_remove should not return Err");
        assert!(output.is_success());

        let content = fs::read_to_string(wit_dir.join("world.wit")).expect("failed to read world");
        assert!(!content.contains("test:dep"), "the import should be gone");
        assert!(
            !stale_dir.exists(),
            "wit/deps should no longer hold what the world does not name"
        );
    }

    #[test]
    fn test_version_changes_between_lock_files() {
        let before = BTreeMap::from([
            ("wasi:http".to_string(), vec!["0.2.0".to_string()]),
            ("wasi:config".to_string(), vec!["0.2.0-draft".to_string()]),
        ]);
        let after = BTreeMap::from([
            ("wasi:http".to_string(), vec!["0.2.6".to_string()]),
            ("wasi:clocks".to_string(), vec!["0.2.12".to_string()]),
        ]);

        assert_eq!(
            version_changes(&before, &after),
            [
                "Adding wasi:clocks 0.2.12",
                "Updating wasi:http 0.2.0 -> 0.2.6",
                "Removing wasi:config 0.2.0-draft",
            ]
        );

        // A lock file that did not move reports nothing
        assert!(version_changes(&after, &after).is_empty());
    }

    #[tokio::test]
    async fn test_remove_takes_the_whole_include_statement() {
        let (_temp, project_dir, wit_dir) = setup_test_project().await;
        let world_wit_path = wit_dir.join("world.wit");

        // `include ... with { ... }` runs past the line its path is on, and takes no semicolon
        let content = r#"package test:component@0.1.0;

world example {
    include wasi:http/proxy@0.2.0 with {
        handler as my-handler
    }
    import wasi:cli/stdout@0.2.0;
}
"#;
        fs::write(&world_wit_path, content).expect("failed to write world.wit");

        let config = test_config(&wit_dir);
        let ctx = test_ctx(&project_dir).await;

        let output = handle_remove(&ctx, "wasi:http", &config)
            .await
            .expect("handle_remove should not return Err");
        assert!(output.is_success());

        let content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");

        assert!(!content.contains("wasi:http/proxy"));
        assert!(
            !content.contains("my-handler"),
            "the rest of the statement should go with it: {content}"
        );
        assert!(
            content.contains("import wasi:cli/stdout@0.2.0;"),
            "the statement after it stays: {content}"
        );
        assert!(content.contains('}'), "the world still closes: {content}");
        assert_parses(&world_wit_path);
    }

    #[test]
    fn test_statements_inside_comments_are_not_statements() {
        let content = r#"package test:component@0.1.0;

world example {
    /*
    import wasi:http/types@0.2.0;
    /* a nested comment, which WIT allows */
    */
    import wasi:cli/stdout@0.2.0; // and a line comment
}
"#;
        let blanked = without_comments(content);

        // Only the one real statement survives blanking
        let statements: Vec<&str> = blanked.lines().filter_map(any_statement_path).collect();
        assert_eq!(statements, ["wasi:cli/stdout@0.2.0"], "{blanked}");

        // Blanking keeps the shape of the file, so a line index into it is one into the original
        assert_eq!(blanked.lines().count(), content.lines().count());

        // ... and an import is not written into the middle of the comment
        let updated = insert_import_into_world(content, "import wasi:clocks/wall-clock@0.2.0;")
            .expect("should insert the import");
        let position = updated
            .lines()
            .position(|line| line.contains("wasi:clocks/wall-clock"))
            .expect("the import was written");
        let last_comment = updated
            .lines()
            .position(|line| line.trim() == "*/")
            .expect("the comment is still there");
        assert!(position > last_comment, "{updated}");
    }

    #[test]
    fn test_version_is_only_read_when_it_reads_as_one() {
        // A `use` names what it takes after the version
        assert_eq!(
            statement_version("wasi:clocks/wall-clock@0.2.0.{datetime}"),
            Some("0.2.0")
        );
        assert_eq!(statement_version("wasi:http/types@0.2.0"), Some("0.2.0"));
        assert_eq!(
            statement_version("wasmcloud:messaging/consumer@0.2.0-draft"),
            Some("0.2.0-draft")
        );
        // An `include ... with` carries more than the path
        assert_eq!(
            statement_version("wasi:http/proxy@0.2.0 with {"),
            Some("0.2.0")
        );
        assert_eq!(statement_version("wasi:http/types"), None);
        assert_eq!(statement_version("wasi:http/types@nonsense"), None);

        // The package is what comes before the version, and before anything else on the line
        assert_eq!(
            statement_package("wasi:http/proxy with { handler as my-handler }"),
            "wasi:http/proxy"
        );
        assert_eq!(
            statement_package("wasi:http/types@0.2.0"),
            "wasi:http/types"
        );
    }

    #[tokio::test]
    async fn test_remove_leaves_other_packages_with_the_same_prefix() {
        let (_temp, project_dir, wit_dir) = setup_test_project().await;
        let world_wit_path = wit_dir.join("world.wit");

        let content = r#"package test:component@0.1.0;

world example {
    import wasi:http/types@0.2.0;
    import wasi:http-extra/types@0.2.0;
}
"#;
        fs::write(&world_wit_path, content).expect("failed to write world.wit");

        let config = test_config(&wit_dir);
        let ctx = test_ctx(&project_dir).await;

        handle_remove(&ctx, "wasi:http", &config)
            .await
            .expect("handle_remove should not return Err");

        let content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");

        assert!(!content.contains("import wasi:http/types@0.2.0;"));
        assert!(content.contains("import wasi:http-extra/types@0.2.0;"));
    }

    #[test]
    fn test_add_writes_a_parseable_import() {
        let content = r#"package test:component@0.1.0;

world example {
}
"#;
        // The line `wash wit add` writes for a fully qualified interface reference
        let wit_ref = WitRef::parse("wasi:http/types@0.2.0").expect("reference should parse");
        let world_path = wit_ref
            .world_path(wit_ref.version.as_deref())
            .expect("interface reference has a path");
        let updated = insert_import_into_world(content, &format!("import {world_path};"))
            .expect("should insert the import");

        assert!(updated.contains("    import wasi:http/types@0.2.0;"));

        let temp = TempDir::new().expect("failed to create temp dir");
        let world_wit_path = temp.path().join("world.wit");
        fs::write(&world_wit_path, &updated).expect("failed to write world.wit");
        assert_parses(&world_wit_path);
    }

    #[test]
    fn test_add_rejects_a_package_without_an_interface() {
        // A world cannot import a package, so there is no line to write for one
        let wit_ref = WitRef::parse("wasmcloud:messaging@0.3.0").expect("reference should parse");
        assert_eq!(wit_ref.world_path(None), None);

        let error = anyhow::anyhow!("no release matching version requirement `=0.3.0`");
        let message = package_reference_message(&wit_ref, Err(&error));
        assert!(
            message.contains("no release matching version requirement"),
            "the reason the lookup failed is kept: {message}"
        );
        assert!(
            message.contains("wash wit add wasmcloud:messaging/<interface>@0.3.0"),
            "unresolvable packages still get a usable example: {message}"
        );

        let contents = PackageContents {
            version: Some(semver::Version::new(0, 3, 0)),
            interfaces: vec!["types".to_string(), "consumer".to_string()],
            worlds: vec!["imports".to_string()],
        };
        let message = package_reference_message(&wit_ref, Ok(&contents));
        assert!(message.contains("wash wit add wasmcloud:messaging/consumer@0.3.0"));
        assert!(message.contains("include wasmcloud:messaging/imports@0.3.0;"));
    }

    #[test]
    fn test_version_written_into_the_world() {
        let resolved = semver::Version::new(0, 3, 7);

        // A whole version is written as it was asked for
        assert_eq!(
            version_to_write(None, Some("0.3.4"), Some(&resolved)).as_deref(),
            Some("0.3.4")
        );

        // A partial version is not something a world statement can carry, so what gets written is
        // the newest version compatible with it
        assert_eq!(
            version_to_write(None, Some("0.3"), Some(&resolved)).as_deref(),
            Some("0.3.7")
        );

        // No version becomes the version that was selected: an unversioned statement does not
        // resolve against a versioned package
        assert_eq!(
            version_to_write(None, None, Some(&resolved)).as_deref(),
            Some("0.3.7")
        );

        // Only a package that could not be resolved at all is left unversioned
        assert_eq!(version_to_write(None, None, None), None);

        // The version the world already uses is what an unversioned reference takes
        assert_eq!(
            version_to_write(Some("0.3.1"), None, Some(&resolved)).as_deref(),
            Some("0.3.1")
        );

        // ... and what a compatible partial version takes, rather than moving the world
        assert_eq!(
            version_to_write(Some("0.3.1"), Some("0.3"), Some(&resolved)).as_deref(),
            Some("0.3.1")
        );

        // A whole version that was asked for wins over the world's
        assert_eq!(
            version_to_write(Some("0.3.1"), Some("0.3.4"), Some(&resolved)).as_deref(),
            Some("0.3.4")
        );
        assert_eq!(
            version_to_write(Some("0.2.0"), Some("0.3.4"), Some(&resolved)).as_deref(),
            Some("0.3.4")
        );

        // A partial version the world cannot satisfy takes the newest compatible with it
        assert_eq!(
            version_to_write(Some("0.2.0"), Some("0.3"), Some(&resolved)).as_deref(),
            Some("0.3.7")
        );

        let wit_ref = WitRef::parse("wasi:http/types@0.2").expect("reference should parse");
        assert_eq!(
            wit_ref.world_path(Some("0.2.3")).as_deref(),
            Some("wasi:http/types@0.2.3")
        );
        assert_eq!(wit_ref.world_path(None).as_deref(), Some("wasi:http/types"));
    }

    #[test]
    fn test_labelled_imports_are_read() {
        // The `(implements ..)` form binds one interface to a label
        let statement =
            world_statement("    import team-a: wasi:keyvalue/store@0.2.0-draft;").unwrap();
        assert_eq!(statement.keyword, "import");
        assert_eq!(statement.label, Some("team-a"));
        assert_eq!(statement.path, "wasi:keyvalue/store@0.2.0-draft");
        assert_eq!(statement_version(statement.path), Some("0.2.0-draft"));

        // An unlabelled import has a `:` of its own, which is the package's
        let statement = world_statement("    import wasi:keyvalue/store@0.2.0-draft;").unwrap();
        assert_eq!(statement.label, None);
        assert_eq!(statement.path, "wasi:keyvalue/store@0.2.0-draft");

        // A labelled import is one the world depends on, so it decides the version in use
        let wit_ref = WitRef::parse("wasi:keyvalue").unwrap();
        assert!(wit_ref.is_same_package(statement.path));
    }

    #[tokio::test]
    async fn test_remove_leaves_labelled_imports_alone() {
        let (_temp, project_dir, wit_dir) = setup_test_project().await;
        let world_wit_path = wit_dir.join("world.wit");

        let content = r#"package test:component@0.1.0;

world example {
    import team-a: wasi:keyvalue/store@0.2.0-draft;
    import wasi:keyvalue/store@0.2.0-draft;
}
"#;
        fs::write(&world_wit_path, content).expect("failed to write world.wit");

        let config = test_config(&wit_dir);
        let ctx = test_ctx(&project_dir).await;

        let output = handle_remove(&ctx, "wasi:keyvalue/store", &config)
            .await
            .expect("handle_remove should not return Err");
        assert!(output.is_success());
        let (message, _) = output.render();

        let content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");

        // The plain import goes; the labelled binding the component's bindings are written
        // against stays, and is reported
        assert!(
            !content.contains("    import wasi:keyvalue/store@0.2.0-draft;"),
            "{content}"
        );
        assert!(content.contains("import team-a: wasi:keyvalue/store@0.2.0-draft;"));
        assert!(message.contains("labelled import"), "{message}");
        assert_parses(&world_wit_path);
    }

    #[tokio::test]
    async fn test_remove_reports_when_only_labelled_imports_match() {
        let (_temp, project_dir, wit_dir) = setup_test_project().await;
        let world_wit_path = wit_dir.join("world.wit");

        let content = r#"package test:component@0.1.0;

world example {
    import team-a: wasi:keyvalue/store@0.2.0-draft;
}
"#;
        fs::write(&world_wit_path, content).expect("failed to write world.wit");

        let config = test_config(&wit_dir);
        let ctx = test_ctx(&project_dir).await;

        let output = handle_remove(&ctx, "wasi:keyvalue", &config)
            .await
            .expect("handle_remove should not return Err");
        assert!(!output.is_success());
        let (message, _) = output.render();
        assert!(message.contains("only imported under a label"), "{message}");
        assert!(
            message.contains("import team-a: wasi:keyvalue/store@0.2.0-draft;"),
            "{message}"
        );

        let after = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");
        assert_eq!(after, content, "world.wit should be untouched");
    }

    #[tokio::test]
    async fn test_remove_takes_the_doc_comment_with_it() {
        let (_temp, project_dir, wit_dir) = setup_test_project().await;
        let world_wit_path = wit_dir.join("world.wit");

        let content = r#"package test:component@0.1.0;

world example {
    /// Wall-clock time, used for the cache expiry.
    /// Two lines of it, even.
    import wasi:clocks/wall-clock@0.2.0;
    /// Monotonic time, used for the retry backoff.
    import wasi:clocks/monotonic-clock@0.2.0;
}
"#;
        fs::write(&world_wit_path, content).expect("failed to write world.wit");

        let config = test_config(&wit_dir);
        let ctx = test_ctx(&project_dir).await;

        handle_remove(&ctx, "wasi:clocks/wall-clock", &config)
            .await
            .expect("handle_remove should not return Err");

        let content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");

        // The comment that documented the removed import goes with it, rather than staying to
        // document the one below
        assert!(!content.contains("cache expiry"), "{content}");
        assert!(!content.contains("Two lines of it"), "{content}");
        assert!(content.contains("/// Monotonic time, used for the retry backoff."));
        assert!(content.contains("import wasi:clocks/monotonic-clock@0.2.0;"));
        assert_parses(&world_wit_path);
    }

    #[test]
    fn test_version_line_compatibility() {
        // The same major.minor line is one package to WIT, which cannot hold two versions of it
        assert!(same_version_line("0.2.3", "0.2.6"));
        assert!(same_version_line("0.2.0", "0.2.0"));
        // Different major.minor versions are separate packages, which can sit side by side
        assert!(!same_version_line("0.2.3", "0.3.0"));
        assert!(!same_version_line("1.0.0", "2.0.0"));
        assert!(!same_version_line("0.2", "0.2.0"));
    }

    #[test]
    fn test_export_pins_the_version_in_use() {
        let world = "package test:component@0.1.0;\n\n\
                     world example {\n    export wasi:http/incoming-handler@0.2.0;\n}\n";

        // An export uses the package just as an import does, so it decides the version
        let exported = world
            .lines()
            .filter_map(any_statement_path)
            .next()
            .expect("the export names a path");
        assert_eq!(statement_version(exported), Some("0.2.0"));

        // ... but it is not something `add` treats as already imported, or `remove` takes away
        assert_eq!(world.lines().filter_map(dependency_path).count(), 0);
    }

    #[test]
    fn test_requested_version_against_the_one_in_use() {
        assert!(version_matches("0.2.0", "0.2.0"));
        assert!(version_matches("0.2", "0.2.0"));
        assert!(version_matches("0.2.0-draft", "0.2.0-draft"));
        assert!(!version_matches("0.3.0", "0.2.0"));
        assert!(!version_matches("0.2", "0.3.1"));
    }

    #[test]
    fn test_add_rejects_an_interface_the_package_does_not_define() {
        let contents = PackageContents {
            version: Some(semver::Version::new(0, 2, 0)),
            interfaces: vec!["types".to_string(), "consumer".to_string()],
            worlds: Vec::new(),
        };

        let wit_ref = WitRef::parse("wasmcloud:messaging/consumr@0.2.0").expect("should parse");
        let message = undefined_interface_message(&wit_ref, &contents)
            .expect("a misspelled interface is rejected");
        assert!(
            message.contains("does not define an interface named `consumr`"),
            "the message names the interface: {message}"
        );
        assert!(
            message.contains("wash wit add wasmcloud:messaging/consumer@0.2.0"),
            "the message lists what the package does define: {message}"
        );

        let wit_ref = WitRef::parse("wasmcloud:messaging/consumer@0.2.0").expect("should parse");
        assert_eq!(undefined_interface_message(&wit_ref, &contents), None);
    }

    #[test]
    fn test_insert_import_into_world_with_same_line_open_brace() {
        let content = r#"package test:component@0.1.0;

world example {
}
"#;

        let updated = insert_import_into_world(content, "import wasi:http/types@0.2.0;")
            .expect("insert should succeed");

        assert!(updated.contains("    import wasi:http/types@0.2.0;"));
    }

    #[test]
    fn test_insert_import_into_world_with_next_line_open_brace() {
        let content = r#"package test:component@0.1.0;

world example
{
}
"#;

        let updated = insert_import_into_world(content, "import wasi:http/types@0.2.0;")
            .expect("insert should succeed");

        assert!(updated.contains("{\n    import wasi:http/types@0.2.0;\n}"));
    }

    #[test]
    fn test_insert_import_into_world_with_next_line_open_brace_and_existing_imports() {
        let content = r#"package test:component@0.1.0;

world example
{
    import wasi:config/store@0.2.0-rc.1;
}
"#;

        let updated = insert_import_into_world(content, "import wasi:http/types@0.2.0;")
            .expect("insert should succeed");

        assert!(updated.contains(
            "    import wasi:config/store@0.2.0-rc.1;\n    import wasi:http/types@0.2.0;\n}"
        ));
    }

    #[test]
    fn test_parse_package_with_version() {
        assert_eq!(
            WitRef::parse("wasi:http@0.2.0").unwrap(),
            WitRef {
                package: "wasi:http".to_string(),
                interface: None,
                version: Some("0.2.0".to_string()),
            }
        );
        assert_eq!(
            WitRef::parse("wasi:keyvalue/store@0.2.0-draft").unwrap(),
            WitRef {
                package: "wasi:keyvalue".to_string(),
                interface: Some("store".to_string()),
                version: Some("0.2.0-draft".to_string()),
            }
        );
    }

    #[test]
    fn test_parse_package_without_version() {
        assert_eq!(
            WitRef::parse("wasi:http").unwrap(),
            WitRef {
                package: "wasi:http".to_string(),
                interface: None,
                version: None,
            }
        );
        let wit_ref = WitRef::parse("wasi:http/types").unwrap();
        assert_eq!(wit_ref.world_path(None).as_deref(), Some("wasi:http/types"));
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
            format!("{package_name}-{ver}.wasm")
        } else {
            format!("{package_name}.wasm")
        };

        assert_eq!(filename, "test-package-1.0.0.wasm");
    }

    #[tokio::test]
    async fn test_default_output_filename_without_version() {
        let package_name = "test-package";
        let version: Option<String> = None;

        let filename = if let Some(ver) = &version {
            format!("{package_name}-{ver}.wasm")
        } else {
            format!("{package_name}.wasm")
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
        assert_eq!(
            dependency_path("    import wasi:http/types@0.2.0;"),
            Some("wasi:http/types@0.2.0")
        );
        assert_eq!(
            dependency_path("  include wasi:http/proxy@0.2.0;"),
            Some("wasi:http/proxy@0.2.0")
        );
        assert_eq!(dependency_path("export wasi:cli/run@0.2.0;"), None);
        assert_eq!(dependency_path("world example {"), None);

        let wit_ref = WitRef::parse("wasi:http").unwrap();
        assert!(wit_ref.matches_path("wasi:http/types@0.2.0"));
        assert!(wit_ref.matches_path("wasi:http/proxy"));
        assert!(!wit_ref.matches_path("wasi:http-extra/types@0.2.0"));
        assert!(!wit_ref.matches_path("wasi:keyvalue/store"));

        let wit_ref = WitRef::parse("wasi:http/types").unwrap();
        assert!(wit_ref.matches_path("wasi:http/types@0.2.0"));
        assert!(!wit_ref.matches_path("wasi:http/proxy@0.2.0"));
    }

    #[tokio::test]
    async fn test_clean_nonexistent_deps_directory() {
        let (_temp, project_dir, wit_dir) = setup_test_project().await;
        let deps_dir = wit_dir.join("deps");
        assert!(!deps_dir.exists());

        let ctx = test_ctx(&project_dir).await;
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

    #[test]
    fn test_multiple_imports_in_world_wit() {
        let mut content = "package test:component@0.1.0;\n\nworld example {\n}\n".to_string();

        for reference in [
            "wasi:cli/stdout@0.2.0",
            "wasi:http/types@0.2.0",
            "wasi:keyvalue/store@0.2.0-draft",
        ] {
            let wit_ref = WitRef::parse(reference).unwrap();
            let world_path = wit_ref
                .world_path(wit_ref.version.as_deref())
                .expect("interface reference has a path");
            content = insert_import_into_world(&content, &format!("import {world_path};"))
                .expect("should insert the import");
        }

        let imports: Vec<&str> = content.lines().filter_map(dependency_path).collect();
        assert_eq!(
            imports,
            [
                "wasi:cli/stdout@0.2.0",
                "wasi:http/types@0.2.0",
                "wasi:keyvalue/store@0.2.0-draft"
            ]
        );

        let temp = TempDir::new().expect("failed to create temp dir");
        let world_wit_path = temp.path().join("world.wit");
        fs::write(&world_wit_path, &content).expect("failed to write world.wit");
        assert_parses(&world_wit_path);
    }

    #[tokio::test]
    async fn test_remove_specific_import() {
        let (_temp, project_dir, wit_dir) = setup_test_project().await;
        let world_wit_path = wit_dir.join("world.wit");

        let content = r#"package test:component@0.1.0;

world example {
    import wasi:cli/stdout@0.2.0;
    import wasi:http/types@0.2.0;
    import wasi:keyvalue/store@0.2.0-draft;
}
"#;
        fs::write(&world_wit_path, content).expect("failed to write world.wit");

        let config = test_config(&wit_dir);
        let ctx = test_ctx(&project_dir).await;

        let output = handle_remove(&ctx, "wasi:http", &config)
            .await
            .expect("handle_remove should not return Err");
        assert!(output.is_success());

        let final_content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");

        assert!(
            !final_content.contains("import wasi:http"),
            "File still contains 'import wasi:http'"
        );
        assert!(final_content.contains("import wasi:cli"));
        assert!(final_content.contains("import wasi:keyvalue"));
        assert_parses(&world_wit_path);
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
            format!("{package_name}-{ver}.wasm")
        } else {
            format!("{package_name}.wasm")
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
                format!("{package_name}-{ver}.wasm")
            } else {
                format!("{package_name}.wasm")
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

    #[test]
    fn test_add_malformed_wit_file() {
        // A file that does not parse is still edited: the WIT syntax error is reported by the
        // fetch that follows, and the import line itself lands where it belongs
        let malformed_content = r#"package test:component@0.1.0;

world example {
    import wasi:http/types@0.2.0;
    // Missing closing brace!
"#;

        let updated = insert_import_into_world(malformed_content, "import wasi:cli/stdout@0.2.0;")
            .expect("should insert the import");

        assert!(
            updated
                .contains("    import wasi:http/types@0.2.0;\n    import wasi:cli/stdout@0.2.0;")
        );
    }

    #[test]
    fn test_add_to_wit_file_with_comments() {
        let content_with_comments = r#"package test:component@0.1.0;

// This is a comment about imports
world example {
    // Import HTTP types
    import wasi:http/types@0.2.0;
    // More comments
}
"#;

        let updated =
            insert_import_into_world(content_with_comments, "import wasi:cli/stdout@0.2.0;")
                .expect("should insert the import");

        assert_eq!(
            updated
                .lines()
                .filter(|l| l.trim().starts_with("//"))
                .count(),
            3,
            "comments should be preserved: {updated}"
        );
        assert!(
            updated
                .contains("    import wasi:http/types@0.2.0;\n    import wasi:cli/stdout@0.2.0;"),
            "{updated}"
        );
    }

    #[tokio::test]
    async fn test_remove_from_empty_world_wit() {
        let (_temp, project_dir, wit_dir) = setup_test_project().await;
        let world_wit_path = wit_dir.join("world.wit");

        let empty_world = r#"package test:component@0.1.0;

world example {
}
"#;
        fs::write(&world_wit_path, empty_world).expect("failed to write empty world.wit");

        let config = test_config(&wit_dir);
        let ctx = test_ctx(&project_dir).await;

        let output = handle_remove(&ctx, "wasi:http", &config)
            .await
            .expect("handle_remove should not return Err");
        assert!(!output.is_success(), "there is nothing to remove");

        let content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");
        assert_eq!(content, empty_world, "world.wit should be untouched");
    }

    #[tokio::test]
    async fn test_wit_file_with_inline_comments() {
        let (_temp, project_dir, wit_dir) = setup_test_project().await;
        let world_wit_path = wit_dir.join("world.wit");

        let content = r#"package test:component@0.1.0;

world example {
    import wasi:http/types@0.2.0; // HTTP types
    import wasi:cli/stdout; // CLI output
    // import wasi:keyvalue/store@0.2.0-draft; - This is commented out
}
"#;
        fs::write(&world_wit_path, content).expect("failed to write world.wit");

        let config = test_config(&wit_dir);
        let ctx = test_ctx(&project_dir).await;

        // A trailing comment does not hide the interface a line imports, versioned or not
        for package in ["wasi:http", "wasi:cli/stdout"] {
            let output = handle_remove(&ctx, package, &config)
                .await
                .expect("handle_remove should not return Err");
            assert!(output.is_success(), "{package} should have been removed");
        }

        let content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");
        assert!(!content.contains("import wasi:http"));
        assert!(!content.contains("import wasi:cli"));
        assert!(
            content.contains("// import wasi:keyvalue/store@0.2.0-draft;"),
            "a commented out import is left alone: {content}"
        );
    }

    #[test]
    fn test_wit_ref_package_only() {
        // Package-level references (no interface) must be accepted
        assert!(WitRef::parse("wasi:http").is_ok());
        assert!(WitRef::parse("wasi:http@0.2.0").is_ok());
        assert!(WitRef::parse("wasi:keyvalue").is_ok());
    }

    #[test]
    fn test_wit_ref_with_interface() {
        // Interface-level references must still be accepted
        assert!(WitRef::parse("wasi:http/types").is_ok());
        assert!(WitRef::parse("wasi:http/types@0.2.0").is_ok());
        assert!(WitRef::parse("wasi:http/incoming-handler@0.2.0").is_ok());
    }

    #[test]
    fn test_wit_ref_invalid() {
        // Missing namespace separator
        assert!(WitRef::parse("http").is_err());
        assert!(WitRef::parse("http/types").is_err());
        // Empty namespace or package name
        assert!(WitRef::parse(":http").is_err());
        assert!(WitRef::parse("wasi:").is_err());
        assert!(WitRef::parse(":/").is_err());
        // A trailing slash names no interface
        assert!(WitRef::parse("wasi:http/").is_err());
        assert!(WitRef::parse("wasi:http/@0.2.0").is_err());
    }

    #[tokio::test]
    async fn test_remove_with_version_in_argument() {
        let (_temp, project_dir, wit_dir) = setup_test_project().await;
        let world_wit_path = wit_dir.join("world.wit");

        let content = "package test:component@0.1.0;\n\nworld example {\n    import wasi:http/types@0.2.0;\n}\n";
        fs::write(&world_wit_path, content).expect("failed to write world.wit");

        let config = Config {
            wit: Some(crate::wit::WitConfig {
                wit_dir: Some(wit_dir.clone()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let ctx = test_ctx(&project_dir).await;

        // Removing with an explicit version should work: "wasi:http@0.2.0" matches
        // "import wasi:http/types@0.2.0;" by stripping the version before comparing.
        let output = handle_remove(&ctx, "wasi:http@0.2.0", &config)
            .await
            .expect("handle_remove should not return Err");

        assert!(
            output.is_success(),
            "removing wasi:http@0.2.0 should succeed when 'import wasi:http/types@0.2.0;' is present"
        );

        let new_content = tokio::fs::read_to_string(&world_wit_path)
            .await
            .expect("failed to read world.wit");
        assert!(
            !new_content.contains("import wasi:http"),
            "import should have been removed"
        );
    }
}
