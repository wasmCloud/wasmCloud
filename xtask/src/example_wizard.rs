//! The interactive half of `example-index`: a full-screen picker over the
//! derived manifests. Every architecture comes from [`crate::example_index`],
//! so adding a template to `templates/` is the whole of adding it here.

use std::io::Write as _;

use anyhow::{Context, Result};
use console::{Key, Term};

use wash_topology::render::{display_width, truncate};
use wash_topology::style::{BOLD, BRIGHT_CYAN, CYAN, DIM, MAGENTA, RESET};
use wash_topology::{Topology, diagram};

/// The upstream every in-repo template is cloned from.
const TEMPLATE_REPO: &str = "https://github.com/wasmCloud/wasmCloud.git";

/// One rendered row of the left pane. Headers are drawn but never selected.
enum Row<'a> {
    Header(String),
    Project(&'a Topology),
}

/// Run the picker. Returns once the user selects a project or quits.
pub fn run(topologies: &[Topology]) -> Result<()> {
    if topologies.is_empty() {
        println!("no projects indexed; nothing to pick from");
        return Ok(());
    }

    // No TTY means no sensible default; refuse rather than block on a read.
    let term = Term::stdout();
    if !term.is_term() {
        anyhow::bail!(
            "the wizard needs an interactive terminal; run `cargo xtask example-index` \
             for the non-interactive listing"
        );
    }

    let rows = build_rows(topologies);
    let selectable: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| matches!(r, Row::Project(_)))
        .map(|(i, _)| i)
        .collect();

    term.hide_cursor().ok();
    let chosen = event_loop(&term, &rows, &selectable);
    term.show_cursor().ok();
    term.clear_screen().ok();

    match chosen? {
        Some(topology) => confirm(&term, topology),
        None => {
            println!("nothing selected");
            Ok(())
        }
    }
}

fn event_loop<'a>(
    term: &Term,
    rows: &'a [Row<'a>],
    selectable: &[usize],
) -> Result<Option<&'a Topology>> {
    let mut cursor = 0usize;
    loop {
        draw(term, rows, selectable[cursor]).context("failed to draw wizard")?;
        match term.read_key().context("failed to read key")? {
            Key::ArrowUp | Key::Char('k') => cursor = cursor.saturating_sub(1),
            Key::ArrowDown | Key::Char('j') => cursor = (cursor + 1).min(selectable.len() - 1),
            Key::Home => cursor = 0,
            Key::End => cursor = selectable.len() - 1,
            Key::Enter => {
                let Row::Project(topology) = &rows[selectable[cursor]] else {
                    continue;
                };
                return Ok(Some(topology));
            }
            Key::Escape | Key::Char('q') => return Ok(None),
            _ => {}
        }
    }
}

/// Group projects under a heading per architecture.
fn build_rows(topologies: &[Topology]) -> Vec<Row<'_>> {
    let mut shapes: Vec<String> = topologies
        .iter()
        .map(|t| format!("{:?}", t.shape))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    // Architectures with structure first; single-component ones are the long tail.
    shapes.sort_by_key(|s| (s == "Single", s.clone()));

    let mut rows = Vec::new();
    for shape in shapes {
        rows.push(Row::Header(shape.to_uppercase()));
        for topology in topologies
            .iter()
            .filter(|t| format!("{:?}", t.shape) == shape)
        {
            rows.push(Row::Project(topology));
        }
    }
    rows
}

fn draw(term: &Term, rows: &[Row<'_>], active: usize) -> Result<()> {
    let (_, width) = term.size();
    let left_width = rows
        .iter()
        .map(|r| match r {
            Row::Header(h) => h.chars().count(),
            Row::Project(t) => t.id.chars().count() + 2,
        })
        .max()
        .unwrap_or(20)
        .max(24);

    let mut out = String::new();
    out.push_str(&format!(
        "\n  {BOLD}{BRIGHT_CYAN}wash new{RESET} {DIM}·{RESET} {CYAN}choose an architecture{RESET}\n"
    ));
    out.push_str(&format!(
        "  {DIM}↑↓ move · enter select · q quit{RESET}\n\n"
    ));

    // Right pane belongs to whichever project is under the cursor.
    let Row::Project(selected) = &rows[active] else {
        return Ok(());
    };
    let right = detail_lines(selected);

    let height = rows.len().max(right.len());
    for i in 0..height {
        let left = match rows.get(i) {
            Some(Row::Header(h)) => format!("  {BOLD}{MAGENTA}{h}{RESET}"),
            Some(Row::Project(t)) if i == active => {
                format!("  {BRIGHT_CYAN}❯{RESET} {BOLD}{BRIGHT_CYAN}{}{RESET}", t.id)
            }
            Some(Row::Project(t)) => format!("    {}", t.id),
            None => String::new(),
        };
        let pad = (left_width + 6).saturating_sub(display_width(&left));
        out.push_str(&left);
        out.push_str(&" ".repeat(pad));
        out.push_str(&format!("{DIM}│{RESET}  "));
        if let Some(line) = right.get(i) {
            // Trim anything that would wrap and break the two-pane alignment.
            out.push_str(&truncate(line, width as usize - left_width - 12));
        }
        out.push('\n');
    }

    term.clear_screen()?;
    let mut stdout = std::io::stdout();
    stdout.write_all(out.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

/// The right pane: title, diagram, capabilities, and anything unresolved.
fn detail_lines(topology: &Topology) -> Vec<String> {
    let mut lines = vec![
        format!(
            "{BOLD}{}{RESET}",
            topology.title.as_deref().unwrap_or(&topology.id)
        ),
        format!("{DIM}{}{RESET}", topology.source),
        String::new(),
    ];
    lines.extend(diagram(topology));
    lines.push(String::new());

    if !topology.capabilities.is_empty() {
        lines.push(format!("{CYAN}capabilities{RESET}"));
        for capability in &topology.capabilities {
            lines.push(format!("  {DIM}{capability}{RESET}"));
        }
        lines.push(String::new());
    }
    lines.extend(wash_topology::render::not_in_the_diagram(topology, 52));
    lines
}

/// Show the `wash new` invocation the selection maps to, and offer to run it; defaults to no.
fn confirm(term: &Term, topology: &Topology) -> Result<()> {
    let name = &topology.id;
    let command = format!(
        "wash new {TEMPLATE_REPO} --name {name} --subfolder {}",
        topology.source
    );

    println!();
    println!("  {BOLD}{BRIGHT_CYAN}{}{RESET}", topology.id);
    for line in diagram(topology) {
        println!("  {line}");
    }
    println!();
    println!("  {DIM}scaffolds with{RESET}");
    println!("  {CYAN}{command}{RESET}");
    println!();
    print!("  run it now? {DIM}[y/N]{RESET} ");
    std::io::stdout().flush()?;

    let answer = term.read_key().context("failed to read confirmation")?;
    println!();
    if !matches!(answer, Key::Char('y') | Key::Char('Y')) {
        println!("  {DIM}not run; copy the command above to scaffold it yourself{RESET}");
        return Ok(());
    }

    let status = std::process::Command::new("sh")
        .args(["-c", &command])
        .status()
        .context("failed to run wash new (is wash on PATH?)")?;
    if !status.success() {
        anyhow::bail!("`{command}` failed");
    }
    Ok(())
}
