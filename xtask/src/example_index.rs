//! `example-index`: the repo's local wrapper over `wash_topology::derive`.
//! Renders a listing to eyeball, and `--check` gates CI on every shipped
//! project still deriving cleanly.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use wash_topology::catalog::{CATALOG_NAME, to_catalog_json};
use wash_topology::style::{BOLD, BRIGHT_CYAN, CYAN, DIM, MAGENTA, RESET, YELLOW};
use wash_topology::{Topology, derive, diagram};

/// Starting points for the walk; `discover` finds projects by convention from here.
const ROOTS: [&str; 2] = ["templates", "examples"];

/// What the command does with the topologies it derives.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Render the wizard listing; touch nothing.
    Print,
    /// Fail on any underivable project or a stale committed catalog.
    Check,
    /// Open the interactive picker over the derived topologies.
    Wizard,
    /// Write the repo's `catalog.json`, the published document the wizard fetches.
    WriteCatalog,
}

/// Build a project so its components exist to be decoded. Uses `wash build`, not
/// `build.command`: only wash fetches the WIT dependency closure some templates need.
fn build_project(wash: &Path, project: &Path) -> Result<()> {
    println!("  building {}", project.display());
    let status = std::process::Command::new(wash)
        .arg("-C")
        .arg(project)
        .arg("build")
        .status()
        .with_context(|| format!("failed to spawn `wash build` for {}", project.display()))?;
    if !status.success() {
        bail!("`wash build` failed in {}", project.display());
    }
    Ok(())
}

/// Collect every project under [`ROOTS`], derive its topology, and act per `mode`.
/// `only` restricts the scan so a generated workload is indexed by the same rules.
pub fn run(workspace: &Path, mode: Mode, build: bool, only: Option<&Path>) -> Result<()> {
    let wash = if build {
        Some(crate::ensure_wash(workspace).context("failed to locate wash for --build")?)
    } else {
        None
    };

    let mut topologies = Vec::new();
    let roots: Vec<PathBuf> = match only {
        Some(path) => vec![path.to_path_buf()],
        None => ROOTS.iter().map(|r| workspace.join(r)).collect(),
    };
    for dir in roots {
        if !dir.is_dir() {
            continue;
        }
        // An explicit --path may name the project itself rather than a directory of them.
        let projects = if only.is_some() && wash_topology::derive::find_config(&dir).is_some() {
            vec![dir.clone()]
        } else {
            wash_topology::derive::discover(&dir)
        };
        for project in projects {
            if let Some(wash) = &wash
                && let Err(err) = build_project(wash, &project)
            {
                eprintln!("  {YELLOW}skipped:{RESET} {err:#}");
            }
            let rel = project
                .strip_prefix(workspace)
                .unwrap_or(&project)
                .to_string_lossy()
                .replace('\\', "/");
            topologies.push(
                derive(&project, &rel)
                    .with_context(|| format!("failed to derive topology for {rel}"))?,
            );
        }
    }
    topologies.sort_by(|a, b| a.id.cmp(&b.id));

    match mode {
        Mode::Print => print_listing(&topologies),
        Mode::Wizard => crate::example_wizard::run(&topologies)?,
        // Derivations already ran with `?`; the count guards a walk that finds nothing.
        Mode::Check => {
            if topologies.is_empty() {
                bail!("no projects found under templates/ or examples/");
            }
            // Full scans also gate the published catalog; a --path scan is
            // partial and would compare against a fragment.
            if only.is_none() {
                let path = workspace.join(CATALOG_NAME);
                let committed = std::fs::read_to_string(&path).unwrap_or_default();
                if committed != to_catalog_json(&topologies)? {
                    bail!(
                        "{CATALOG_NAME} is out of date; run \
                         `cargo xtask example-index --build --write-catalog`"
                    );
                }
            }
            println!(
                "all {} project(s) derive cleanly; {CATALOG_NAME} up to date",
                topologies.len()
            );
        }
        Mode::WriteCatalog => {
            if topologies.is_empty() {
                bail!("no projects found under templates/ or examples/");
            }
            let path = workspace.join(CATALOG_NAME);
            std::fs::write(&path, to_catalog_json(&topologies)?)
                .with_context(|| format!("failed to write {}", path.display()))?;
            println!("wrote {} entries to {}", topologies.len(), path.display());
        }
    }
    Ok(())
}

/// Render what the picker would show — a preview of the selection screen.
fn print_listing(topologies: &[Topology]) {
    let mut by_shape: BTreeMap<String, Vec<&Topology>> = BTreeMap::new();
    for topology in topologies {
        by_shape
            .entry(format!("{:?}", topology.shape))
            .or_default()
            .push(topology);
    }

    println!();
    println!(
        "  {BOLD}{BRIGHT_CYAN}wash new{RESET} {DIM}—{RESET} {CYAN}choose an architecture{RESET}"
    );
    println!(
        "  {DIM}{} projects indexed from templates/ and examples/{RESET}",
        topologies.len()
    );

    for (shape, group) in &by_shape {
        println!();
        println!("  {BOLD}{MAGENTA}{}{RESET}", shape.to_uppercase());
        for topology in group {
            let title = topology.title.as_deref().unwrap_or(&topology.id);
            println!(
                "    {BRIGHT_CYAN}●{RESET} {BOLD}{}{RESET} {DIM}{}{RESET}",
                topology.id, title
            );
            println!();
            for line in diagram(topology) {
                println!("      {line}");
            }
            println!();
            if !topology.capabilities.is_empty() {
                println!(
                    "      {CYAN}caps{RESET} {DIM}{}{RESET}",
                    topology.capabilities.join("  ")
                );
            }
            if !topology.unresolved.is_empty() {
                println!(
                    "      {YELLOW}⚠ {} unresolved{RESET} {DIM}{}{RESET}",
                    topology.unresolved.len(),
                    topology
                        .unresolved
                        .iter()
                        .map(|u| u.reason.as_str())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    }
    println!();
}
