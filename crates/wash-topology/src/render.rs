//! Drawing a [`Topology`] as boxes and connectors. Box-drawing characters
//! rather than a terminal image protocol, so output renders identically over
//! SSH, in tmux, and in CI logs; shared by every caller.

use std::collections::{BTreeMap, BTreeSet};

use crate::schema::{Edge, EdgeKind, Role, Topology};
use crate::style::{BOLD, BRIGHT_CYAN, CYAN, DIM, RESET};

/// Draw the topology as boxes and connectors, one rank per hop.
pub fn diagram(topology: &Topology) -> Vec<String> {
    diagram_with(topology, &Decor::default())
}

/// Optional overlays drawn onto a topology; [`Decor::default`] must render
/// byte-for-byte what [`diagram`] renders.
#[derive(Default)]
pub struct Decor {
    /// Node drawn as the one being worked on: a brighter border, same size.
    pub focus: Option<String>,
    /// An extra dim line inside a node's box, by node id.
    pub notes: BTreeMap<String, String>,
}

/// [`diagram`], with [`Decor`] applied.
pub fn diagram_with(topology: &Topology, decor: &Decor) -> Vec<String> {
    let ranks = rank_nodes(topology);
    let mut columns: Vec<Vec<Block>> = Vec::new();

    for (depth, rank) in ranks.iter().enumerate() {
        // One connector block per target so the arrow lines up with its box.
        if depth > 0 {
            let incoming: Vec<&Edge> = topology
                .edges
                .iter()
                .filter(|e| rank.contains(&e.to))
                .collect();
            columns.push(
                rank.iter()
                    .map(|id| connector_block(&incoming, id))
                    .collect(),
            );
        }
        columns.push(
            rank.iter()
                .map(|id| {
                    let node = topology.nodes.iter().find(|n| &n.id == id);
                    let role = node.map(|n| n.role).unwrap_or(Role::Component);
                    let subs = node.map(|n| n.subscriptions.as_slice()).unwrap_or(&[]);
                    node_box(
                        id,
                        role,
                        subs,
                        decor.notes.get(id).map(String::as_str),
                        decor.focus.as_deref() == Some(id.as_str()),
                    )
                })
                .collect(),
        );
    }
    render_columns(&columns)
}

/// A rectangular run of pre-styled lines.
type Block = Vec<String>;

/// Lay blocks out in a grid: block `i` of every column shares a vertical band.
fn render_columns(columns: &[Vec<Block>]) -> Vec<String> {
    let bands = columns.iter().map(Vec::len).max().unwrap_or(0);
    let band_heights: Vec<usize> = (0..bands)
        .map(|i| {
            columns
                .iter()
                .filter_map(|col| col.get(i))
                .map(Vec::len)
                .max()
                .unwrap_or(0)
        })
        .collect();

    let column_lines: Vec<Vec<String>> = columns
        .iter()
        .map(|col| {
            let mut lines = Vec::new();
            for (i, height) in band_heights.iter().enumerate() {
                if i > 0 {
                    lines.push(String::new());
                }
                let block = col.get(i).cloned().unwrap_or_default();
                let slack = height.saturating_sub(block.len());
                let above = slack / 2;
                lines.extend(std::iter::repeat_n(String::new(), above));
                lines.extend(block);
                lines.extend(std::iter::repeat_n(String::new(), slack - above));
            }
            lines
        })
        .collect();

    let widths: Vec<usize> = column_lines
        .iter()
        .map(|col| col.iter().map(|l| display_width(l)).max().unwrap_or(0))
        .collect();
    let height = column_lines.iter().map(Vec::len).max().unwrap_or(0);

    let mut rendered: Vec<String> = (0..height)
        .map(|row| {
            let mut line = String::new();
            for (col, width) in column_lines.iter().zip(&widths) {
                let cell = col.get(row).cloned().unwrap_or_default();
                let pad = width.saturating_sub(display_width(&cell));
                line.push_str(&cell);
                line.push_str(&" ".repeat(pad + 2));
            }
            line.trim_end().to_string()
        })
        .collect();
    while rendered.last().is_some_and(String::is_empty) {
        rendered.pop();
    }
    rendered
}

/// Wiring the diagram above could not draw, grouped by hint and printed dim.
pub fn not_in_the_diagram(topology: &Topology, width: usize) -> Vec<String> {
    if topology.unresolved.is_empty() {
        return Vec::new();
    }

    // Grouped by hint, not reason: same reason, different hint is a different note.
    let mut grouped: Vec<(&str, Vec<&str>)> = Vec::new();
    for entry in &topology.unresolved {
        if !grouped.iter().any(|(hint, _)| *hint == entry.hint) {
            grouped.push((&entry.hint, Vec::new()));
        }
        let Some((_, nodes)) = grouped.iter_mut().find(|(hint, _)| *hint == entry.hint) else {
            continue;
        };
        if let Some(node) = entry.node.as_deref() {
            nodes.push(node);
        }
    }

    let mut lines = vec![String::new(), format!("{CYAN}not in the diagram{RESET}")];
    for (hint, nodes) in grouped {
        if !nodes.is_empty() {
            lines.push(format!("  {DIM}{}{RESET}", nodes.join(", ")));
        }
        lines.extend(
            wrap(hint, width.saturating_sub(4))
                .into_iter()
                .map(|line| format!("    {DIM}{line}{RESET}")),
        );
    }
    lines
}

/// Greedy word wrap. The hints are plain prose, so nothing cleverer is owed.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Cut a styled string to `limit` visible columns, keeping escape sequences intact.
pub fn truncate(s: &str, limit: usize) -> String {
    if display_width(s) <= limit {
        return s.to_string();
    }
    let mut out = String::new();
    let mut width = 0;
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            out.push(ch);
            for esc in chars.by_ref() {
                out.push(esc);
                if esc == 'm' {
                    break;
                }
            }
        } else {
            if width >= limit {
                break;
            }
            out.push(ch);
            width += 1;
        }
    }
    // Close any open styling so the cut cannot bleed into the next cell.
    out.push_str(crate::style::RESET);
    out
}

/// Visible width, skipping SGR escape sequences.
pub fn display_width(s: &str) -> usize {
    let mut width = 0;
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            for esc in chars.by_ref() {
                if esc == 'm' {
                    break;
                }
            }
        } else {
            width += 1;
        }
    }
    width
}

/// Group nodes into hops from whatever the host drives; unreachable nodes land in rank 0.
fn rank_nodes(topology: &Topology) -> Vec<Vec<String>> {
    let reached: BTreeSet<&str> = topology.edges.iter().map(|e| e.to.as_str()).collect();
    let mut ranks: Vec<Vec<String>> = vec![
        topology
            .nodes
            .iter()
            .filter(|n| !reached.contains(n.id.as_str()))
            .map(|n| n.id.clone())
            .collect(),
    ];
    let mut placed: BTreeSet<String> = ranks
        .first()
        .map(|r| r.iter().cloned().collect())
        .unwrap_or_default();

    // Widen one hop at a time; the guard bounds cycles.
    while placed.len() < topology.nodes.len() && ranks.len() < topology.nodes.len() + 1 {
        let prev = ranks.last().cloned().unwrap_or_default();
        let next: Vec<String> = topology
            .edges
            .iter()
            .filter(|e| prev.contains(&e.from) && !placed.contains(&e.to))
            .map(|e| e.to.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if next.is_empty() {
            break;
        }
        placed.extend(next.iter().cloned());
        ranks.push(next);
    }
    ranks.retain(|r| !r.is_empty());
    ranks
}

/// A single component drawn as a box; focus brightens the border without
/// changing size, so nothing shifts as the focus moves.
fn node_box(
    id: &str,
    role: Role,
    subscriptions: &[String],
    note: Option<&str>,
    focused: bool,
) -> Vec<String> {
    let head = format!("{} {id}", glyph(role));
    let subs = subscriptions.join(" ");
    let details: Vec<&str> = [subs.as_str(), note.unwrap_or_default()]
        .into_iter()
        .filter(|line| !line.is_empty())
        .collect();

    let width = details
        .iter()
        .map(|line| line.chars().count())
        .chain([head.chars().count()])
        .max()
        .unwrap_or(0)
        + 2;
    let edge: String = if focused {
        format!("{BOLD}{BRIGHT_CYAN}")
    } else {
        CYAN.to_string()
    };

    let mut lines = vec![
        format!("{edge}╭{}╮{RESET}", "─".repeat(width)),
        format!(
            "{edge}│{RESET} {BOLD}{head}{RESET}{} {edge}│{RESET}",
            " ".repeat(width.saturating_sub(head.chars().count() + 2))
        ),
    ];
    for detail in details {
        lines.push(format!(
            "{edge}│{RESET} {DIM}{detail}{RESET}{} {edge}│{RESET}",
            " ".repeat(width.saturating_sub(detail.chars().count() + 2))
        ));
    }
    lines.push(format!("{edge}╰{}╯{RESET}", "─".repeat(width)));
    lines
}

/// The arrow entering one box, labelled with its subject (messaging) or interface (direct).
fn connector_block(incoming: &[&Edge], target: &str) -> Block {
    let Some(edge) = incoming.iter().find(|e| e.to == target) else {
        return Vec::new();
    };
    let arrow = match edge.kind {
        EdgeKind::Direct => "───▶",
        EdgeKind::Messaging => "═══▶",
    };
    let label = edge.subject.clone().unwrap_or_else(|| short(&edge.via));
    vec![
        format!("{DIM}{label}{RESET}"),
        format!("{BRIGHT_CYAN}{arrow}{RESET}"),
    ]
}

/// `wasmcloud:messaging/handler@0.2.0` -> `handler`.
fn short(iface: &str) -> String {
    iface
        .rsplit('/')
        .next()
        .unwrap_or(iface)
        .split('@')
        .next()
        .unwrap_or(iface)
        .to_string()
}

/// Per-role icon, restricted to glyphs the common terminal fonts carry.
fn glyph(role: Role) -> &'static str {
    match role {
        Role::Ingress => "◈",
        Role::Worker => "⚙",
        Role::Service => "▣",
        Role::Component => "▪",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_breaks_on_words_and_keeps_them_all() {
        let text = "one two three four five six seven";
        let wrapped = wrap(text, 12);
        assert!(
            wrapped.iter().all(|l| l.chars().count() <= 12),
            "{wrapped:?}"
        );
        assert_eq!(wrapped.join(" "), text, "no word may be lost");
    }
    use crate::schema::{FactsSource, Node, Shape};

    fn topology() -> Topology {
        let node = |id: &str, role: Role| Node {
            id: id.into(),
            role,
            world: None,
            world_match: None,
            facts_from: FactsSource::Wasm,
            file: None,
            subscriptions: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
        };
        Topology {
            schema: crate::SCHEMA_VERSION,
            id: "demo".into(),
            source: "demo".into(),
            repo: None,
            subfolder: None,
            title: None,
            shape: Shape::Chain,
            capabilities: Vec::new(),
            nodes: vec![
                node("ingress", Role::Ingress),
                node("step1", Role::Component),
            ],
            edges: vec![Edge {
                from: "ingress".into(),
                to: "step1".into(),
                via: "demo:app/step1@0.1.0".into(),
                kind: EdgeKind::Direct,
                subject: None,
            }],
            unresolved: Vec::new(),
        }
    }

    fn focused(node: &str) -> Decor {
        Decor {
            focus: Some(node.to_string()),
            ..Decor::default()
        }
    }

    fn noted(node: &str, note: &str) -> Decor {
        Decor {
            notes: [(node.to_string(), note.to_string())].into_iter().collect(),
            ..Decor::default()
        }
    }

    #[test]
    fn undecorated_draws_exactly_what_the_plain_diagram_draws() {
        // All callers must agree on what an undecorated topology looks like.
        let topology = topology();
        assert_eq!(
            diagram_with(&topology, &Decor::default()),
            diagram(&topology)
        );
    }

    #[test]
    fn a_focused_box_changes_only_its_border() {
        let topology = topology();
        let plain = diagram(&topology);
        let drawn = diagram_with(&topology, &focused("step1"));

        assert_eq!(plain.len(), drawn.len(), "focus must not change the layout");
        // Same visible columns, so nothing shifts when the focus moves.
        for (before, after) in plain.iter().zip(&drawn) {
            assert_eq!(display_width(before), display_width(after));
        }
        assert_ne!(plain, drawn, "the border should have brightened");
    }

    #[test]
    fn focusing_a_node_that_is_not_drawn_is_ignored() {
        let topology = topology();
        assert_eq!(
            diagram_with(&topology, &focused("nosuch")),
            diagram(&topology)
        );
    }

    #[test]
    fn a_note_becomes_a_line_inside_that_box_only() {
        let topology = topology();
        let drawn = diagram_with(&topology, &noted("step1", "keyvalue")).join("\n");
        assert!(drawn.contains("keyvalue"), "{drawn}");

        // The note widens and heightens its own box; the other keeps its size.
        let plain = diagram(&topology).join("\n");
        assert!(plain.lines().count() < drawn.lines().count());
        assert!(
            drawn.contains("◈ ingress"),
            "the other box survives: {drawn}"
        );
    }

    #[test]
    fn a_note_longer_than_the_name_widens_the_box() {
        let topology = topology();
        let long = "keyvalue · logging · http-egress";
        let drawn = diagram_with(&topology, &noted("step1", long));

        let widths: Vec<usize> = drawn.iter().map(|l| display_width(l)).collect();
        let note_line = drawn
            .iter()
            .find(|line| line.contains(long))
            .expect("the note is drawn");
        // Borders still line up, which they would not if the box were sized to the name.
        assert!(
            note_line.ends_with(&format!("{CYAN}│{RESET}")),
            "{note_line:?}"
        );
        assert!(widths.iter().all(|w| *w > long.chars().count()));
    }
}
