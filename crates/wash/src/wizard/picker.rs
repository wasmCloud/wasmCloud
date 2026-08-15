//! Full-screen architecture picker.
//!
//! Built on `dialoguer::console::Term`, sharing the non-interactive listing's renderer.

use std::io::Write as _;
use std::time::Duration;

use anyhow::{Context as _, Result};
use dialoguer::console::{Key, Term};
use wash_topology::{Shape, Topology};

use super::index::{EXPERIMENTAL_REPO, Index, Source};
use wash_topology::render::{diagram, display_width, not_in_the_diagram, truncate};
#[cfg(not(feature = "host-component-plugins"))]
use wash_topology::style::YELLOW;
use wash_topology::style::{BOLD, BRIGHT_CYAN, CYAN, DIM, MAGENTA, RESET};

/// One row of the left pane. Headers never land under the cursor; projects are
/// indices, not references, so the row list borrows nothing.
enum Row {
    Header(&'static str),
    Project(usize),
    /// Compose a new architecture rather than starting from a template.
    Custom,
    /// Provide a capability rather than consume one.
    CustomCapability,
    /// Switch the community source on or off.
    Experimental,
}

/// What the user picked.
pub enum Choice<'a> {
    /// Scaffold this existing template.
    Template(&'a Topology),
    /// Design a new architecture from scratch.
    Custom,
    /// Generate a host component plugin plus a consumer of it.
    CustomCapability,
    /// Add or drop the community source. The caller re-loads and comes back:
    /// `choose` is synchronous and `index::load` is not.
    ToggleExperimental,
    /// Fetch both sources again, via the same caller round-trip as the toggle.
    Refresh,
}

/// Present the picker and return the chosen architecture, or `None` if the
/// user quit. The caller must refuse to reach here without a TTY.
pub fn choose(index: &Index) -> Result<Option<Choice<'_>>> {
    let topologies = index.topologies.as_slice();
    let rows = build_rows(topologies, &index.origins);
    // Everything except the headings.
    let selectable: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| !matches!(r, Row::Header(_)))
        .map(|(i, _)| i)
        .collect();

    // Alternate screen: `clear_screen` alone leaves every redraw in scrollback.
    let term = Term::stdout();
    term.write_str(ENTER_ALT_SCREEN).ok();
    term.hide_cursor().ok();
    let chosen = event_loop(&term, index, &rows, &selectable);
    term.show_cursor().ok();
    term.write_str(LEAVE_ALT_SCREEN).ok();
    chosen
}

fn event_loop<'a>(
    term: &Term,
    index: &'a Index,
    rows: &[Row],
    selectable: &[usize],
) -> Result<Option<Choice<'a>>> {
    let mut cursor = 0usize;
    loop {
        draw(term, index, rows, selectable[cursor]).context("failed to draw the picker")?;
        match term.read_key().context("failed to read a key")? {
            Key::ArrowUp | Key::Char('k') => cursor = cursor.saturating_sub(1),
            Key::ArrowDown | Key::Char('j') => cursor = (cursor + 1).min(selectable.len() - 1),
            Key::Home => cursor = 0,
            Key::End => cursor = selectable.len() - 1,
            Key::Enter => {
                return Ok(match rows[selectable[cursor]] {
                    Row::Custom => Some(Choice::Custom),
                    Row::CustomCapability => Some(Choice::CustomCapability),
                    Row::Project(i) => index.topologies.get(i).map(Choice::Template),
                    Row::Experimental => Some(Choice::ToggleExperimental),
                    Row::Header(_) => continue,
                });
            }
            Key::Char('r') => return Ok(Some(Choice::Refresh)),
            Key::Escape | Key::Char('q') => return Ok(None),
            _ => {}
        }
    }
}

/// Group projects under one heading per shape; community entries form a single
/// trailing section because `index::load` appends them without re-sorting.
fn build_rows(topologies: &[Topology], origins: &[Source]) -> Vec<Row> {
    let community = |index: usize| origins.get(index) == Some(&Source::Experimental);

    let mut rows = vec![
        Row::Header("DESIGN YOUR OWN"),
        Row::Custom,
        Row::CustomCapability,
    ];
    let mut current: Option<Shape> = None;
    for (index, topology) in topologies.iter().enumerate() {
        if community(index) {
            continue;
        }
        if current != Some(topology.shape) {
            rows.push(Row::Header(topology.shape.label()));
            current = Some(topology.shape);
        }
        rows.push(Row::Project(index));
    }

    // Always drawn, on or off, so the toggle stays findable.
    rows.push(Row::Header("COMMUNITY"));
    rows.push(Row::Experimental);
    rows.extend(
        (0..topologies.len())
            .filter(|index| community(*index))
            .map(Row::Project),
    );
    rows
}

fn draw(term: &Term, index: &Index, rows: &[Row], active: usize) -> Result<()> {
    let topologies = index.topologies.as_slice();
    let community = community_count(index);
    let (_, cols) = term.size();
    let left_width = rows
        .iter()
        .map(|r| match r {
            Row::Header(h) => h.chars().count(),
            Row::Custom => CUSTOM_LABEL.chars().count() + 2,
            Row::CustomCapability => CAPABILITY_LABEL.chars().count() + 2,
            Row::Experimental => EXPERIMENTAL_LABEL.chars().count() + 6,
            Row::Project(i) => topologies
                .get(*i)
                .map(|t| t.id.chars().count() + 2)
                .unwrap_or(0),
        })
        .max()
        .unwrap_or(24)
        .max(24);

    let right = match rows[active] {
        Row::Project(selected) => match topologies.get(selected) {
            Some(topology) => detail_lines(topology),
            None => return Ok(()),
        },
        Row::Custom => custom_detail_lines(),
        Row::CustomCapability => capability_detail_lines(),
        Row::Experimental => {
            experimental_detail_lines(index.experimental, community, index.community_age)
        }
        Row::Header(_) => return Ok(()),
    };
    // Keeps the two panes from wrapping into each other on a narrow terminal.
    let right_budget = (cols as usize).saturating_sub(left_width + 12).max(20);

    let mut out = String::new();
    out.push_str(&format!(
        "\n  {BOLD}{BRIGHT_CYAN}wash wizard{RESET} {DIM}·{RESET} {CYAN}choose an architecture{RESET}\n"
    ));
    out.push_str(&format!(
        "  {DIM}↑↓ move · enter select · r refresh · q quit{RESET}\n\n"
    ));

    for row in 0..rows.len().max(right.len()) {
        let left = match rows.get(row) {
            Some(Row::Header(heading)) => format!("  {BOLD}{MAGENTA}{heading}{RESET}"),
            Some(Row::Custom) if row == active => {
                format!("  {BRIGHT_CYAN}❯{RESET} {BOLD}{BRIGHT_CYAN}{CUSTOM_LABEL}{RESET}")
            }
            Some(Row::Custom) => format!("    {CUSTOM_LABEL}"),
            Some(Row::CustomCapability) if row == active => {
                format!("  {BRIGHT_CYAN}❯{RESET} {BOLD}{BRIGHT_CYAN}{CAPABILITY_LABEL}{RESET}")
            }
            Some(Row::CustomCapability) => format!("    {CAPABILITY_LABEL}"),
            Some(Row::Experimental) => {
                let marker = if row == active {
                    format!("  {BRIGHT_CYAN}❯{RESET} ")
                } else {
                    "    ".to_string()
                };
                let box_ = if index.experimental {
                    format!("{BRIGHT_CYAN}[x]{RESET}")
                } else {
                    format!("{DIM}[ ]{RESET}")
                };
                let tally = if index.experimental {
                    format!("{DIM}{community} available{RESET}")
                } else {
                    format!("{DIM}not loaded{RESET}")
                };
                format!("{marker}{box_} {EXPERIMENTAL_LABEL}  {tally}")
            }
            Some(Row::Project(index)) if row == active => format!(
                "  {BRIGHT_CYAN}❯{RESET} {BOLD}{BRIGHT_CYAN}{}{RESET}",
                topologies.get(*index).map(|t| t.id.as_str()).unwrap_or("")
            ),
            Some(Row::Project(index)) => format!(
                "    {}",
                topologies.get(*index).map(|t| t.id.as_str()).unwrap_or("")
            ),
            None => String::new(),
        };
        out.push_str(&left);
        out.push_str(&" ".repeat((left_width + 6).saturating_sub(display_width(&left))));
        out.push_str(&format!("{DIM}│{RESET}  "));
        if let Some(line) = right.get(row) {
            out.push_str(&truncate(line, right_budget));
        }
        out.push('\n');
    }

    term.clear_screen()?;
    let mut stdout = std::io::stdout();
    stdout.write_all(out.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

/// Label for the compose-your-own row.
const CUSTOM_LABEL: &str = "+ custom architecture";

/// Label for the capability generator.
const CAPABILITY_LABEL: &str = "+ custom capability";

/// Label for the community-source toggle.
const EXPERIMENTAL_LABEL: &str = "include awesome-wasmcloud";

/// A rough age in the largest useful unit, rounded down.
fn describe_age(age: Duration) -> String {
    let seconds = age.as_secs();
    match seconds {
        0..=90 => "just now".to_string(),
        91..=5400 => format!("{} minutes ago", seconds / 60),
        5401..=172_800 => format!("{} hours ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

/// How many of the indexed architectures came from the community source.
fn community_count(index: &Index) -> usize {
    index
        .origins
        .iter()
        .filter(|origin| **origin == Source::Experimental)
        .count()
}

/// The right pane for the community toggle.
fn experimental_detail_lines(on: bool, count: usize, age: Option<Duration>) -> Vec<String> {
    let mut lines = vec![
        format!("{BOLD}Community contributions{RESET}"),
        format!("{DIM}{}{RESET}", EXPERIMENTAL_REPO),
        String::new(),
    ];
    lines.push(if on {
        format!("{CYAN}on{RESET} {DIM}· {count} architecture(s) added below{RESET}")
    } else {
        format!("{DIM}off · enter to fetch and include them{RESET}")
    });
    // The list is served from disk; a cached list must not look live.
    if let Some(age) = age {
        lines.push(format!(
            "{DIM}fetched {} · r to fetch again{RESET}",
            describe_age(age)
        ));
    }
    lines.push(String::new());
    lines.extend(
        [
            "Not curated — community contributions, for inspiration",
            "rather than a supported starting point. Picking one",
            "clones it just like any other entry.",
        ]
        .map(str::to_string),
    );

    if on && count == 0 {
        lines.push(String::new());
        lines.extend(
            [
                "Nothing there yet. A project hosted in the repository",
                "appears by itself; one living elsewhere is added with:",
            ]
            .map(str::to_string),
        );
        lines.push(format!(
            "  {DIM}wash wizard index --link <url> --into <dir>{RESET}"
        ));
    }
    lines
}

/// Switch to the alternate screen buffer, and back.
const ENTER_ALT_SCREEN: &str = "\x1b[?1049h";
const LEAVE_ALT_SCREEN: &str = "\x1b[?1049l";

/// The right pane when the custom row is highlighted.
fn custom_detail_lines() -> Vec<String> {
    vec![
        format!("{BOLD}Design your own workload{RESET}"),
        format!("{DIM}generated, not cloned{RESET}"),
        String::new(),
        "Pick an architecture and get a project that is stubbed".to_string(),
        "but completely wired: it builds and serves a request".to_string(),
        "across every component boundary before you edit it.".to_string(),
        String::new(),
        format!("{CYAN}architectures{RESET}"),
        format!("  {DIM}single    one component behind HTTP{RESET}"),
        format!("  {DIM}chain     ingress calling through N components{RESET}"),
        format!("  {DIM}fan-out   ingress calling N components directly{RESET}"),
        String::new(),
        format!("{CYAN}egress{RESET}"),
        format!("  {DIM}outbound HTTP, with the host allowlisted{RESET}"),
    ]
}

/// The right pane for the capability generator.
fn capability_detail_lines() -> Vec<String> {
    // Only mutated when the feature is off, below.
    #[cfg_attr(feature = "host-component-plugins", allow(unused_mut))]
    let mut lines = vec![
        format!("{BOLD}Provide your own capability{RESET}"),
        format!("{DIM}a host plugin, generated{RESET}"),
        String::new(),
        "Components import capabilities the host provides —".to_string(),
        "keyvalue, blobstore, messaging. This generates one of".to_string(),
        "your own: a component exporting an interface, which".to_string(),
        "other workloads then import as if it were built in.".to_string(),
        String::new(),
        format!("{CYAN}you get{RESET}"),
        format!("  {DIM}plugin     exports the interface, owns its state{RESET}"),
        format!("  {DIM}consumer   imports it, serves HTTP{RESET}"),
        String::new(),
        "Both wired, so it answers a request before you edit it.".to_string(),
    ];

    // Warned here, before generating: the row is where someone decides.
    #[cfg(not(feature = "host-component-plugins"))]
    {
        lines.push(String::new());
        lines.push(format!(
            "{YELLOW}needs a wash built with host-component-plugins{RESET}"
        ));
        lines.push(format!("{DIM}this one was not; the README says how{RESET}"));
    }

    lines
}

/// How wide the detail pane's prose gets before it wraps.
const DETAIL_WIDTH: usize = 52;

/// A clone URL with scheme and trailing `.git` trimmed — never the host, the
/// part worth reading before cloning from a stranger.
fn short_repo(repo: &str) -> &str {
    let trimmed = repo
        .strip_prefix("https://")
        .or_else(|| repo.strip_prefix("http://"))
        .unwrap_or(repo);
    trimmed.strip_suffix(".git").unwrap_or(trimmed)
}

/// The right pane for a template: title, diagram, capabilities, caveats.
pub fn detail_lines(topology: &Topology) -> Vec<String> {
    let mut lines = vec![format!(
        "{BOLD}{}{RESET}",
        topology.title.as_deref().unwrap_or(&topology.id)
    )];
    // A linked entry clones a third-party repository; say whose before enter is pressed.
    match topology.repo.as_deref() {
        Some(repo) => {
            lines.push(format!("{MAGENTA}{}{RESET}", short_repo(repo)));
            lines.push(format!(
                "{DIM}linked · cloned from its own repository{RESET}"
            ));
        }
        None => lines.push(format!("{DIM}{}{RESET}", topology.source)),
    }
    lines.push(String::new());
    // A linked origin stub carries no graph; say so instead of drawing nothing.
    if topology.repo.is_some() && topology.nodes.is_empty() {
        lines.push(format!("{DIM}shape derived after cloning{RESET}"));
    } else {
        lines.extend(diagram(topology));
    }
    lines.push(String::new());

    if !topology.capabilities.is_empty() {
        lines.push(format!("{CYAN}capabilities{RESET}"));
        lines.extend(
            topology
                .capabilities
                .iter()
                .map(|c| format!("  {DIM}{c}{RESET}")),
        );
        lines.push(String::new());
    }
    lines.extend(not_in_the_diagram(topology, DETAIL_WIDTH));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use wash_topology::style::YELLOW;
    use wash_topology::{FactsSource, Node, Role, Unresolved};

    #[test]
    fn hidden_wiring_is_explained_once_rather_than_classified_per_node() {
        let mut t = topology("distributed", Shape::FanOut);
        let hint = "publishes to a subject carried by the incoming message";
        t.unresolved = vec![
            Unresolved {
                node: Some("task-leet".into()),
                reason: "dynamic-publish-subject".into(),
                hint: hint.into(),
            },
            Unresolved {
                node: Some("task-reverse".into()),
                reason: "dynamic-publish-subject".into(),
                hint: hint.into(),
            },
        ];

        let rendered = not_in_the_diagram(&t, 60).join("\n");

        assert!(rendered.contains("task-leet, task-reverse"), "{rendered}");
        assert_eq!(
            rendered.matches("carried by the incoming message").count(),
            1,
            "the shared hint should not repeat per node: {rendered}"
        );
        assert!(
            !rendered.contains("dynamic-publish-subject"),
            "the classification is machine-facing: {rendered}"
        );
        assert!(!rendered.contains(YELLOW), "{rendered}");
    }

    #[test]
    fn a_linked_entry_names_the_repository_it_will_clone_from() {
        let mut t = topology("wasi-ai-app", Shape::Chain);
        t.source = "workload-examples/wasi-ai-app".into();
        t.repo = Some("https://github.com/bharattech/wasi-ai-app.git".into());

        let rendered = detail_lines(&t).join("\n");

        // Pressing enter fetches someone else's repository; the pane says whose.
        assert!(
            rendered.contains("github.com/bharattech/wasi-ai-app"),
            "{rendered}"
        );
        assert!(rendered.contains("linked"), "{rendered}");
        assert!(
            !rendered.contains("workload-examples/wasi-ai-app"),
            "{rendered}"
        );
    }

    #[test]
    fn an_origin_stub_says_the_shape_arrives_with_the_clone() {
        // A linked entry carries no graph; the pane says so instead of drawing nothing.
        let mut t = topology("wasi-ai-app", Shape::Chain);
        t.repo = Some("https://github.com/bharattech/wasi-ai-app.git".into());
        t.nodes.clear();

        let rendered = detail_lines(&t).join("\n");
        assert!(rendered.contains("shape derived after cloning"), "{rendered}");
    }

    #[test]
    fn a_hosted_entry_still_shows_its_path_and_claims_no_origin() {
        let t = topology("http-handler", Shape::Single);
        let rendered = detail_lines(&t).join("\n");
        assert!(rendered.contains("templates/http-handler"), "{rendered}");
        assert!(!rendered.contains("linked"), "{rendered}");
    }

    #[test]
    fn shortening_a_clone_url_keeps_the_host() {
        // The host is the part worth reading before cloning from a stranger.
        assert_eq!(
            short_repo("https://github.com/someone/thing.git"),
            "github.com/someone/thing"
        );
        assert_eq!(short_repo("http://git.example.com/x"), "git.example.com/x");
        assert_eq!(
            short_repo("git@github.com:someone/thing.git"),
            "git@github.com:someone/thing"
        );
    }

    #[test]
    fn a_topology_with_nothing_hidden_says_nothing() {
        let t = topology("plain", Shape::Single);
        assert!(not_in_the_diagram(&t, 60).is_empty());
    }

    fn topology(id: &str, shape: Shape) -> Topology {
        Topology {
            schema: 1,
            id: id.into(),
            source: format!("templates/{id}"),
            repo: None,
            subfolder: None,
            title: None,
            shape,
            capabilities: Vec::new(),
            nodes: vec![Node {
                id: "a".into(),
                role: Role::Ingress,
                world: None,
                world_match: None,
                facts_from: FactsSource::Wasm,
                file: None,
                subscriptions: Vec::new(),
                imports: Vec::new(),
                exports: Vec::new(),
            }],
            edges: Vec::new(),
            unresolved: Vec::new(),
        }
    }

    /// Origins for a set of curated topologies.
    fn curated(count: usize) -> Vec<Source> {
        vec![Source::Cache; count]
    }

    #[test]
    fn one_header_per_architecture_not_one_per_project() {
        let topologies = vec![
            topology("a", Shape::Chain),
            topology("b", Shape::Chain),
            topology("c", Shape::Single),
        ];
        let rows = build_rows(&topologies, &curated(3));

        let headers = rows.iter().filter(|r| matches!(r, Row::Header(_))).count();
        assert_eq!(headers, 4, "design-your-own, two shapes, and community");
        assert_eq!(
            rows.len(),
            10,
            "4 headings + 2 generator rows + 3 projects + the community toggle"
        );
    }

    /// Mirrors what `choose` computes; kept in sync by the tests below.
    fn selectable_of(rows: &[Row]) -> Vec<usize> {
        rows.iter()
            .enumerate()
            .filter(|(_, r)| !matches!(r, Row::Header(_)))
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn the_custom_row_is_reachable_by_the_cursor() {
        // Regression: selectable rows once filtered to Row::Project only.
        let rows = build_rows(&[topology("a", Shape::Chain)], &curated(1));
        let selectable = selectable_of(&rows);

        let custom_index = rows
            .iter()
            .position(|r| matches!(r, Row::Custom))
            .expect("custom row should exist");
        assert!(
            selectable.contains(&custom_index),
            "the custom row must be selectable, got {selectable:?}"
        );
        assert_eq!(
            selectable.first(),
            Some(&custom_index),
            "and it should be where the cursor starts"
        );
    }

    #[test]
    fn headings_are_never_selectable() {
        let rows = build_rows(
            &[topology("a", Shape::Chain), topology("b", Shape::Single)],
            &curated(2),
        );
        for index in selectable_of(&rows) {
            assert!(
                !matches!(rows[index], Row::Header(_)),
                "heading at {index} must not be selectable"
            );
        }
    }

    #[test]
    fn the_custom_row_leads_and_survives_an_empty_index() {
        let rows = build_rows(&[], &[]);
        assert!(matches!(rows.first(), Some(Row::Header(_))));
        assert!(
            matches!(rows.get(1), Some(Row::Custom)),
            "composing must stay reachable with no templates available"
        );
    }

    #[test]
    fn the_community_toggle_is_always_drawn_and_reachable() {
        // Regression: the toggle once existed only as a flag, invisible on screen.
        let rows = build_rows(&[topology("a", Shape::Chain)], &curated(1));
        let toggle = rows
            .iter()
            .position(|r| matches!(r, Row::Experimental))
            .expect("the toggle should be drawn even with the source off");
        assert!(
            selectable_of(&rows).contains(&toggle),
            "and the cursor must be able to reach it"
        );
    }

    #[test]
    fn community_templates_group_under_their_own_heading() {
        // index::load appends community entries without re-sorting.
        let topologies = vec![
            topology("curated-chain", Shape::Chain),
            topology("community-chain", Shape::Chain),
        ];
        let origins = vec![Source::Cache, Source::Experimental];
        let rows = build_rows(&topologies, &origins);

        let chain_headings = rows
            .iter()
            .filter(|r| matches!(r, Row::Header(h) if *h == Shape::Chain.label()))
            .count();
        assert_eq!(chain_headings, 1, "one shape heading, not one per origin");

        let toggle = rows
            .iter()
            .position(|r| matches!(r, Row::Experimental))
            .expect("toggle");
        let community = rows
            .iter()
            .position(|r| matches!(r, Row::Project(1)))
            .expect("the community template is drawn");
        assert!(community > toggle, "community entries follow the toggle");

        let curated_row = rows
            .iter()
            .position(|r| matches!(r, Row::Project(0)))
            .expect("the curated template is drawn");
        assert!(curated_row < toggle, "and curated ones lead");
    }

    #[test]
    fn the_toggle_pane_explains_an_empty_community_source() {
        // An empty community source must explain itself.
        let fresh = Some(Duration::from_secs(30));
        let empty = experimental_detail_lines(true, 0, fresh).join("\n");
        assert!(empty.contains("wash wizard index --link"), "{empty}");

        let populated = experimental_detail_lines(true, 3, fresh).join("\n");
        assert!(!populated.contains("wash wizard index"), "{populated}");
        assert!(populated.contains('3'), "{populated}");

        let off = experimental_detail_lines(false, 0, None).join("\n");
        assert!(off.contains("enter"), "{off}");
        assert!(!off.contains("Nothing there yet"), "{off}");
    }

    #[test]
    fn the_pane_says_how_old_the_list_is() {
        // The list is served from disk; it must say its age.
        let stale = experimental_detail_lines(true, 4, Some(Duration::from_secs(9 * 3600)));
        let stale = stale.join("\n");
        assert!(stale.contains("9 hours ago"), "{stale}");
        assert!(stale.contains("r to fetch again"), "{stale}");

        let off = experimental_detail_lines(false, 0, None).join("\n");
        assert!(!off.contains("fetched"), "{off}");
    }

    #[test]
    fn an_age_is_described_in_the_largest_useful_unit() {
        assert_eq!(describe_age(Duration::from_secs(5)), "just now");
        assert_eq!(describe_age(Duration::from_secs(600)), "10 minutes ago");
        assert_eq!(
            describe_age(Duration::from_secs(3 * 3600 + 3599)),
            "3 hours ago"
        );
        assert_eq!(describe_age(Duration::from_secs(3 * 86_400)), "3 days ago");
    }

    #[test]
    fn truncate_keeps_colour_from_leaking() {
        let styled = format!("{BOLD}abcdefghij{RESET}");
        let cut = truncate(&styled, 4);
        assert!(cut.ends_with(RESET), "must re-reset after cutting: {cut:?}");
        assert!(display_width(&cut) <= 4);
    }

    #[test]
    fn truncate_leaves_short_lines_alone() {
        let styled = format!("{BOLD}abc{RESET}");
        assert_eq!(truncate(&styled, 40), styled);
    }
}
