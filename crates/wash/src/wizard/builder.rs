//! Assembling a [`Spec`] on one screen, with the topology drawn as you go.
//! The diagram redraws on every keystroke; nothing is placed for you, and
//! `enter` stays blocked until every enabled capability has a home.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;

use anyhow::{Context as _, Result};
use dialoguer::console::{Key, Term};
use wash_topology::Topology;
use wash_topology::render::{Decor, diagram_with, display_width};
use wash_topology::style::{BOLD, BRIGHT_CYAN, CYAN, DIM, MAGENTA, RESET, YELLOW};

use super::spec::{Capability, CapabilityKind, Edition, Linking, Spec, Trigger};

const ENTER_ALT_SCREEN: &str = "\x1b[?1049h";
const LEAVE_ALT_SCREEN: &str = "\x1b[?1049l";

/// Rows the cursor can rest on. Compared by value, not index: the row list
/// changes shape as the cursor moves, so an index would drift.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
    Trigger,
    Edition,
    /// Cycles a preset shape, resetting `branches` to match.
    Linking,
    /// How many branches come off the trigger.
    BranchCount,
    /// The depth of one branch, so a lopsided shape can be dialled in.
    Branch(usize),
    /// One capability, by index into the offered set.
    Capability(usize),
    /// Whether one component imports the capability above it.
    Place {
        capability: usize,
        node: usize,
    },
}

/// The capabilities the column offers; the host-carrying two share the index space.
const EGRESS_INDEX: usize = CapabilityKind::ALL.len();
const GRPC_INDEX: usize = CapabilityKind::ALL.len() + 1;
const OFFERED: usize = CapabilityKind::ALL.len() + 2;

/// Indent that nests a component's checkbox under its capability's.
const NEST: &str = "      ";

/// Everything being assembled; host-carrying capabilities are edited as flag plus host.
struct Draft {
    name: String,
    trigger: Trigger,
    edition: Edition,
    /// One entry per branch, giving its depth.
    branches: Vec<usize>,
    over_messaging: bool,
    toggled: [bool; CapabilityKind::ALL.len()],
    egress: Option<String>,
    grpc: Option<String>,
    /// Component id to the capability labels it imports, fully materialised
    /// ([`Spec`] stores the sparse form).
    placement: BTreeMap<String, BTreeSet<String>>,
}

impl Draft {
    /// The preset that best describes the current shape.
    fn linking(&self) -> Linking {
        match (self.over_messaging, self.branches.len()) {
            (true, _) => Linking::Messaging,
            (false, 0) => Linking::None,
            (false, 1) => Linking::Chain,
            (false, _) => Linking::FanOut,
        }
    }

    /// The rows on screen; `open` names the one capability showing its components.
    fn rows(&self, open: Option<usize>) -> Vec<Row> {
        let mut rows = vec![Row::Trigger, Row::Edition, Row::Linking];
        match self.branches.len() {
            0 => {}
            // One branch shows only its depth; widening is the linking row's job.
            1 => rows.push(Row::Branch(0)),
            _ => {
                rows.push(Row::BranchCount);
                rows.extend((0..self.branches.len()).map(Row::Branch));
            }
        }
        for capability in 0..OFFERED {
            rows.push(Row::Capability(capability));
            if open == Some(capability) {
                rows.extend(
                    (0..self.placeable(capability)).map(|node| Row::Place { capability, node }),
                );
            }
        }
        rows
    }

    /// How many components this capability can be given to; zero when off or when it emits no code.
    fn placeable(&self, capability: usize) -> usize {
        if !self.is_on(capability) || !self.offered(capability).emits_code() {
            return 0;
        }
        self.node_ids().len()
    }

    /// The capability at one index of the offered set.
    fn offered(&self, index: usize) -> Capability {
        match index {
            EGRESS_INDEX => Capability::HttpEgress {
                host: self.egress.clone().unwrap_or_default(),
            },
            GRPC_INDEX => Capability::Grpc {
                host: self.grpc.clone().unwrap_or_default(),
            },
            _ => CapabilityKind::ALL
                .get(index)
                .copied()
                .map(Capability::from)
                .unwrap_or(Capability::Keyvalue),
        }
    }

    /// A fresh draft: http trigger, p2, single component, nothing ticked.
    fn new(name: &str) -> Self {
        Draft {
            name: name.to_string(),
            trigger: Trigger::Http,
            edition: Edition::P2,
            branches: Vec::new(),
            over_messaging: false,
            toggled: [false; CapabilityKind::ALL.len()],
            egress: None,
            grpc: None,
            placement: BTreeMap::new(),
        }
    }

    /// Seed a draft from an existing recipe, resolving placement through `Spec`.
    fn from_spec(spec: &Spec) -> Self {
        let mut draft = Draft {
            name: spec.name.clone(),
            trigger: spec.trigger,
            edition: spec.edition,
            branches: spec.branches.clone(),
            over_messaging: spec.over_messaging,
            toggled: CapabilityKind::ALL
                .map(|kind| spec.capabilities.contains(&Capability::from(kind))),
            egress: spec.capabilities.iter().find_map(|c| match c {
                Capability::HttpEgress { host } => Some(host.clone()),
                _ => None,
            }),
            grpc: spec.capabilities.iter().find_map(|c| match c {
                Capability::Grpc { host } => Some(host.clone()),
                _ => None,
            }),
            placement: BTreeMap::new(),
        };
        draft.placement = draft
            .spec_with(spec.placement.clone())
            .resolved_placement()
            .into_iter()
            .map(|(id, labels)| (id, labels.into_iter().collect()))
            .collect();
        draft
    }

    /// The editable host string under a cursor position; also gates the `e` key.
    fn host_mut(&mut self, cursor: Row) -> Option<&mut String> {
        match cursor {
            Row::Capability(index) if index == EGRESS_INDEX => self.egress.as_mut(),
            Row::Capability(index) if index == GRPC_INDEX => self.grpc.as_mut(),
            _ => None,
        }
    }

    fn is_on(&self, index: usize) -> bool {
        match index {
            EGRESS_INDEX => self.egress.is_some(),
            GRPC_INDEX => self.grpc.is_some(),
            _ => self.toggled.get(index).copied().unwrap_or(false),
        }
    }

    /// Switch one capability on or off.
    fn set_on(&mut self, index: usize, on: bool) {
        match index {
            EGRESS_INDEX => self.egress = on.then(|| DEFAULT_EGRESS_HOST.to_string()),
            GRPC_INDEX => self.grpc = on.then(|| DEFAULT_GRPC_HOST.to_string()),
            _ => {
                if let Some(flag) = self.toggled.get_mut(index) {
                    *flag = on;
                }
            }
        }
    }

    /// The capabilities in play, in the fixed order the column shows them.
    fn palette(&self) -> Vec<Capability> {
        (0..OFFERED)
            .filter(|index| self.is_on(*index))
            .map(|index| self.offered(index))
            .collect()
    }

    /// The components this shape produces, paired with whether each ends its branch.
    fn node_ids(&self) -> Vec<(String, bool)> {
        self.spec_with(BTreeMap::new()).node_ids()
    }

    /// The component one `Place` row refers to.
    fn node_at(&self, node: usize) -> Option<String> {
        self.node_ids().into_iter().nth(node).map(|(id, _)| id)
    }

    fn is_placed(&self, capability: usize, node: usize) -> bool {
        let Some(id) = self.node_at(node) else {
            return false;
        };
        self.placement
            .get(&id)
            .is_some_and(|labels| labels.contains(self.offered(capability).label()))
    }

    fn toggle_place(&mut self, capability: usize, node: usize) {
        let label = self.offered(capability).label().to_string();
        let Some(id) = self.node_at(node) else {
            return;
        };
        let labels = self.placement.entry(id).or_default();
        if !labels.remove(&label) {
            labels.insert(label);
        }
    }

    /// What each box says it imports, for the diagram.
    fn notes(&self) -> BTreeMap<String, String> {
        let palette = self.palette();
        self.placement
            .iter()
            .filter_map(|(id, labels)| {
                let listed: Vec<&str> = palette
                    .iter()
                    .map(Capability::label)
                    .filter(|label| labels.contains(*label))
                    .collect();
                (!listed.is_empty()).then(|| (id.clone(), listed.join(" · ")))
            })
            .collect()
    }

    /// How many components import one capability, for the count on its row.
    fn placed_count(&self, capability: usize) -> usize {
        let label = self.offered(capability).label();
        self.placement
            .values()
            .filter(|labels| labels.contains(label))
            .count()
    }

    /// Bring the placement back in line with the shape and the palette;
    /// a component that appears is given nothing.
    fn reconcile(&mut self) {
        let palette: BTreeSet<String> = self
            .palette()
            .iter()
            .map(|c| c.label().to_string())
            .collect();

        self.placement = self
            .node_ids()
            .into_iter()
            .map(|(id, _)| {
                let labels = self
                    .placement
                    .get(&id)
                    .map(|existing| existing.intersection(&palette).cloned().collect())
                    .unwrap_or_default();
                (id, labels)
            })
            .collect();
    }

    /// Give a capability the only component there is — the one case with no choice to make.
    fn place_if_no_choice(&mut self, capability: usize) {
        if self.node_ids().len() != 1 || !self.offered(capability).emits_code() {
            return;
        }
        let label = self.offered(capability).label().to_string();
        if let Some(id) = self.node_at(0) {
            self.placement.entry(id).or_default().insert(label);
        }
    }

    fn spec_with(&self, placement: BTreeMap<String, Vec<String>>) -> Spec {
        Spec {
            name: self.name.clone(),
            trigger: self.trigger,
            edition: self.edition,
            branches: self.branches.clone(),
            over_messaging: self.over_messaging,
            capabilities: self.palette(),
            placement,
        }
    }

    fn to_spec(&self) -> Spec {
        let palette = self.palette();
        // Palette order, so this compares equal to `Spec::default_placement`.
        let placement: BTreeMap<String, Vec<String>> = self
            .placement
            .iter()
            .map(|(id, labels)| {
                let ordered = palette
                    .iter()
                    .map(|c| c.label().to_string())
                    .filter(|label| labels.contains(label))
                    .collect();
                (id.clone(), ordered)
            })
            .collect();

        let mut spec = self.spec_with(BTreeMap::new());
        if placement != spec.default_placement() {
            spec.placement = placement;
        }
        spec
    }
}

/// Default host offered when egress or gRPC is switched on.
const DEFAULT_EGRESS_HOST: &str = "example.com";
const DEFAULT_GRPC_HOST: &str = "grpc.example.com";

/// The host-carrying two, hostless — only ever asked for their [`Capability::label`].
const EGRESS: Capability = Capability::HttpEgress {
    host: String::new(),
};
const GRPC: Capability = Capability::Grpc {
    host: String::new(),
};

/// Width of the capability-name column, measured so every description aligns.
fn name_width() -> usize {
    CapabilityKind::ALL
        .iter()
        .map(|kind| Capability::from(*kind).label().len())
        .chain([EGRESS.label().len(), GRPC.label().len()])
        .max()
        .unwrap_or(0)
}

/// Run the builder. Returns `None` if the user quit.
pub fn build(name: &str, seed: Option<Spec>) -> Result<Option<Spec>> {
    let mut draft = match seed {
        Some(spec) => Draft::from_spec(&spec),
        None => Draft::new(name),
    };

    let term = Term::stdout();
    term.write_str(ENTER_ALT_SCREEN).ok();
    term.hide_cursor().ok();
    let outcome = event_loop(&term, &mut draft);
    term.show_cursor().ok();
    term.write_str(LEAVE_ALT_SCREEN).ok();

    match outcome? {
        true => Ok(Some(draft.to_spec())),
        false => Ok(None),
    }
}

fn event_loop(term: &Term, draft: &mut Draft) -> Result<bool> {
    let mut cursor = Row::Trigger;
    // The one capability showing its components; follows the cursor.
    let mut open: Option<usize> = None;
    let mut editing = false;

    loop {
        let rows = draft.rows(open);
        // The row under the cursor can disappear (branch removed, capability off).
        if !rows.contains(&cursor) {
            cursor = rows.first().copied().unwrap_or(Row::Trigger);
        }
        let index = rows.iter().position(|row| *row == cursor).unwrap_or(0);

        let view = View {
            rows: &rows,
            cursor,
            open,
        };
        draw(term, draft, &view).context("failed to draw the builder")?;

        let key = term.read_key().context("failed to read a key")?;

        // Edit mode: keys land in the host string; navigation would collide with typing.
        if editing {
            match key {
                Key::Enter | Key::Escape => editing = false,
                Key::Backspace => {
                    if let Some(host) = draft.host_mut(cursor) {
                        host.pop();
                    }
                }
                Key::Char(c) if !c.is_control() => {
                    if let Some(host) = draft.host_mut(cursor) {
                        host.push(c);
                    }
                }
                _ => {}
            }
            open = opened_by(cursor);
            continue;
        }

        match key {
            // Moved by identity, not index: the row list reshapes as capabilities open.
            Key::ArrowUp | Key::Char('k') => {
                cursor = rows.get(index.saturating_sub(1)).copied().unwrap_or(cursor);
            }
            Key::ArrowDown | Key::Char('j') => {
                cursor = rows.get(index + 1).copied().unwrap_or(cursor);
            }
            Key::ArrowRight | Key::Char('l') | Key::Char(' ') => step(draft, cursor, 1),
            Key::ArrowLeft | Key::Char('h') => step(draft, cursor, -1),
            Key::Char('e') if draft.host_mut(cursor).is_some() => editing = true,
            Key::Enter => {
                // The validation error is already on screen; stay put.
                if draft.to_spec().validate().is_ok() {
                    return Ok(true);
                }
            }
            Key::Escape | Key::Char('q') => return Ok(false),
            _ => {}
        }

        open = opened_by(cursor);
    }
}

/// Which capability should be showing its components, given where the cursor is.
fn opened_by(cursor: Row) -> Option<usize> {
    match cursor {
        Row::Capability(index)
        | Row::Place {
            capability: index, ..
        } => Some(index),
        _ => None,
    }
}

/// What is on screen and where the cursor is in it.
struct View<'a> {
    rows: &'a [Row],
    cursor: Row,
    open: Option<usize>,
}

impl View<'_> {
    /// The component the cursor is on, for the diagram's focus highlight.
    fn focus(&self, draft: &Draft) -> Option<String> {
        match self.cursor {
            Row::Place { node, .. } => draft.node_at(node),
            _ => None,
        }
    }
}

/// Advance the value under the cursor, wrapping; reconciles once at the end.
fn step(draft: &mut Draft, row: Row, delta: isize) {
    // Deferred so the one-component shortcut runs after reconcile.
    let mut switched_on: Option<usize> = None;

    match row {
        Row::Trigger => {
            draft.trigger = cycle(&Trigger::ALL, draft.trigger, delta);
            if let Some(required) = draft.trigger.required_edition() {
                draft.edition = required;
            }
        }
        Row::Edition => {
            if draft.trigger.required_edition().is_none() {
                draft.edition = cycle(&Edition::ALL, draft.edition, delta);
            }
        }
        Row::Linking => {
            let next = cycle(&Linking::ALL, draft.linking(), delta);
            // `width` means different things per preset; it only carries over between like presets.
            let width = match next {
                Linking::Chain => match draft.branches.as_slice() {
                    [depth] => *depth,
                    _ => 2,
                },
                _ => draft.branches.len().max(2),
            };
            let (branches, over_messaging) = next.expand(width);
            draft.branches = branches;
            draft.over_messaging = over_messaging;
        }
        Row::BranchCount => {
            // Floors at two: reaching one would delete the row under the cursor.
            let next = (draft.branches.len() as isize + delta).clamp(2, 8) as usize;
            draft.branches.resize(next, 1);
        }
        Row::Branch(index) => {
            if let Some(depth) = draft.branches.get_mut(index) {
                *depth = (*depth as isize + delta).clamp(1, 8) as usize;
            }
        }
        Row::Capability(index) => {
            let on = !draft.is_on(index);
            draft.set_on(index, on);
            if on {
                switched_on = Some(index);
            }
        }
        Row::Place { capability, node } => draft.toggle_place(capability, node),
    }

    draft.reconcile();
    if let Some(index) = switched_on {
        draft.place_if_no_choice(index);
    }
}

fn cycle<T: Copy + PartialEq>(all: &[T], current: T, delta: isize) -> T {
    let index = all.iter().position(|v| *v == current).unwrap_or(0) as isize;
    let len = all.len() as isize;
    let next = ((index + delta) % len + len) % len;
    all.get(next as usize).copied().unwrap_or(current)
}

fn draw(term: &Term, draft: &Draft, view: &View<'_>) -> Result<()> {
    let out = screen(draft, view);
    term.clear_screen()?;
    let mut stdout = std::io::stdout();
    stdout.write_all(out.as_bytes())?;
    stdout.flush()?;
    Ok(())
}

/// Compose the whole screen; separate from [`draw`] so it can be tested without a terminal.
fn screen(draft: &Draft, view: &View<'_>) -> String {
    let spec = draft.to_spec();
    let names = name_width();
    let mut left: Vec<String> = Vec::new();

    // The first capability row carries the section label.
    let first_capability = view
        .rows
        .iter()
        .position(|row| matches!(row, Row::Capability(_)));

    for (index, row) in view.rows.iter().enumerate() {
        let selected = *row == view.cursor;
        let marker = if selected {
            format!("{BRIGHT_CYAN}❯{RESET}")
        } else {
            " ".to_string()
        };
        left.push(match row {
            Row::Trigger => format!(
                "{marker} {:<12} {}",
                "trigger",
                choices(&Trigger::ALL, draft.trigger, Trigger::label, selected)
            ),
            Row::Edition if draft.trigger.required_edition().is_some() => format!(
                "{marker} {:<12} {BOLD}{MAGENTA}p3{RESET} {DIM}(services are p3){RESET}",
                "edition"
            ),
            Row::Edition => format!(
                "{marker} {:<12} {}",
                "edition",
                choices(&Edition::ALL, draft.edition, Edition::label, selected)
            ),
            Row::Linking => format!(
                "{marker} {:<12} {}",
                "linking",
                choices(&Linking::ALL, draft.linking(), Linking::label, selected)
            ),
            Row::BranchCount => format!(
                "{marker} {:<12} {}",
                "branches",
                value(draft.branches.len(), selected)
            ),
            Row::Branch(_) if draft.branches.len() == 1 => format!(
                "{marker} {:<12} {}",
                "steps",
                value(draft.branches.first().copied().unwrap_or(1), selected)
            ),
            Row::Branch(index) => format!(
                "{marker} {:<12} {} deep",
                format!("  branch {}", index + 1),
                value(draft.branches.get(*index).copied().unwrap_or(1), selected)
            ),
            Row::Capability(i) => {
                let capability = draft.offered(*i);
                format!(
                    "{marker} {:<12} {} {:<names$} {}",
                    if first_capability == Some(index) {
                        "capabilities"
                    } else {
                        ""
                    },
                    checkbox(draft.is_on(*i)),
                    capability.label(),
                    trailer(draft, *i)
                )
            }
            Row::Place { capability, node } => format!(
                "{marker} {:<12} {NEST}{} {}",
                "",
                checkbox(draft.is_placed(*capability, *node)),
                draft.node_at(*node).unwrap_or_default()
            ),
        });
    }

    // Right pane: the draft, drawn with the same renderer the picker uses.
    let mut right = vec![
        format!("{BOLD}{}{RESET}", spec.name),
        format!(
            "{DIM}{} · {}{RESET}",
            spec.trigger.describe(),
            spec.linking().describe()
        ),
        String::new(),
    ];
    right.extend(diagram_with(
        &preview_topology(&spec),
        &Decor {
            focus: view.focus(draft),
            notes: draft.notes(),
        },
    ));
    right.push(String::new());
    if let Err(err) = spec.validate() {
        right.push(format!("{YELLOW}{err}{RESET}"));
    }

    let width = left
        .iter()
        .map(|l| display_width(l))
        .max()
        .unwrap_or(40)
        .max(46);
    let hint = match view.cursor {
        Row::Capability(_) if view.open.is_some_and(|i| draft.placeable(i) > 0) => {
            "↑↓ move · space toggle · ↓ choose components · enter generate · q cancel"
        }
        Row::Place { .. } => {
            "↑↓ move · space give this component the capability · enter generate · q cancel"
        }
        Row::Capability(index)
            if (index == EGRESS_INDEX || index == GRPC_INDEX)
                && view.open.is_none()
                && draft.is_on(index) =>
        {
            "↑↓ move · ←→/space toggle · e edit host · enter generate · q cancel"
        }
        _ => "↑↓ move · ←→/space change · enter generate · q cancel",
    };
    let mut out = format!(
        "\n  {BOLD}{BRIGHT_CYAN}wash wizard{RESET} {DIM}·{RESET} {CYAN}design a workload{RESET}\n  \
         {DIM}{hint}{RESET}\n\n"
    );
    for line in 0..left.len().max(right.len()) {
        let cell = left.get(line).cloned().unwrap_or_default();
        out.push_str("  ");
        out.push_str(&cell);
        out.push_str(&" ".repeat((width + 2).saturating_sub(display_width(&cell))));
        out.push_str(&format!("{DIM}│{RESET}  "));
        out.push_str(right.get(line).map(String::as_str).unwrap_or(""));
        out.push('\n');
    }
    out
}

/// Right of a capability's name: its description when off, its placement when on.
fn trailer(draft: &Draft, capability: usize) -> String {
    if !draft.is_on(capability) {
        return format!("{DIM}{}{RESET}", draft.offered(capability).describe());
    }
    let host = match capability {
        EGRESS_INDEX => draft.egress.as_deref(),
        GRPC_INDEX => draft.grpc.as_deref(),
        _ => None,
    };
    if !draft.offered(capability).emits_code() {
        return format!("{DIM}on, workload-wide{RESET}");
    }

    let placed = draft.placed_count(capability);
    let total = draft.node_ids().len();
    let where_ = if placed == 0 {
        format!("{YELLOW}nowhere{RESET}")
    } else {
        format!("{DIM}on {placed} of {total}{RESET}")
    };
    match host {
        Some(host) => format!("{where_} {DIM}· {host}{RESET}"),
        None => where_,
    }
}

/// A number under the cursor gets arrows around it.
fn value(n: usize, selected: bool) -> String {
    if selected {
        format!("{BRIGHT_CYAN}‹ {n} ›{RESET}")
    } else {
        format!("{n}")
    }
}

fn checkbox(on: bool) -> String {
    if on {
        format!("{BRIGHT_CYAN}[x]{RESET}")
    } else {
        format!("{DIM}[ ]{RESET}")
    }
}

/// Render one axis as its options, marking the chosen one.
fn choices<T: Copy + PartialEq>(
    all: &[T],
    current: T,
    label: fn(T) -> &'static str,
    active: bool,
) -> String {
    all.iter()
        .map(|value| {
            if *value == current && active {
                format!("{BRIGHT_CYAN}‹ {} ›{RESET}", label(*value))
            } else if *value == current {
                format!("{BOLD}{MAGENTA}{}{RESET}", label(*value))
            } else {
                format!("{DIM}{}{RESET}", label(*value))
            }
        })
        .collect::<Vec<_>>()
        .join("  ")
}

/// A [`Topology`] for the renderer; never touches disk.
fn preview_topology(spec: &Spec) -> Topology {
    // The generator's own builder, so the preview cannot drift from the manifest.
    let mut topology = super::generate::build_topology(spec, &spec.plan());
    topology.title = None;
    for node in &mut topology.nodes {
        node.file = None;
        node.imports.clear();
        node.exports.clear();
    }
    topology
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> Draft {
        let mut draft = Draft {
            name: "demo".into(),
            trigger: Trigger::Http,
            edition: Edition::P2,
            branches: Vec::new(),
            over_messaging: false,
            toggled: [false; CapabilityKind::ALL.len()],
            egress: None,
            grpc: None,
            placement: BTreeMap::new(),
        };
        draft.reconcile();
        draft
    }

    /// Index of a capability in the offered set.
    fn row_of(kind: CapabilityKind) -> usize {
        CapabilityKind::ALL
            .iter()
            .position(|k| *k == kind)
            .expect("every kind is offered")
    }

    /// Index of a component, for the `Place` row that edits it.
    fn node_of(draft: &Draft, id: &str) -> usize {
        draft
            .node_ids()
            .iter()
            .position(|(node, _)| node == id)
            .unwrap_or_else(|| panic!("no component {id}"))
    }

    /// Turn a capability on and give it to one component.
    fn place(draft: &mut Draft, kind: CapabilityKind, node: &str) {
        let capability = row_of(kind);
        if !draft.is_on(capability) {
            step(draft, Row::Capability(capability), 1);
        }
        let node = node_of(draft, node);
        step(draft, Row::Place { capability, node }, 1);
    }

    #[test]
    fn cycling_wraps_in_both_directions() {
        assert_eq!(cycle(&Trigger::ALL, Trigger::Service, 1), Trigger::Http);
        assert_eq!(cycle(&Trigger::ALL, Trigger::Http, -1), Trigger::Service);
    }

    // --- shape ------------------------------------------------------------

    #[test]
    fn the_branch_rows_appear_only_when_they_apply() {
        let mut d = draft();
        let rows = d.rows(None);
        assert!(
            !rows.contains(&Row::BranchCount) && !rows.contains(&Row::Branch(0)),
            "nothing to shape for a single component"
        );

        d.branches = vec![3];
        let rows = d.rows(None);
        assert!(
            rows.contains(&Row::Branch(0)),
            "one branch's depth is the chain length, so it must be editable"
        );
        assert!(
            !rows.contains(&Row::BranchCount),
            "'branches 1' is noise, and stepping it would silently widen the chain"
        );

        d.branches = vec![1, 1];
        let rows = d.rows(None);
        assert!(rows.contains(&Row::BranchCount));
        assert!(rows.contains(&Row::Branch(0)) && rows.contains(&Row::Branch(1)));
    }

    #[test]
    fn a_chain_can_be_lengthened_without_ceasing_to_be_a_chain() {
        let mut d = draft();
        step(&mut d, Row::Linking, 1);
        assert_eq!(d.linking().label(), "chain");

        // The shape row a user lands on below `linking` is the chain's length.
        let shape: Vec<Row> = d
            .rows(None)
            .into_iter()
            .filter(|r| matches!(r, Row::BranchCount | Row::Branch(_)))
            .collect();
        assert_eq!(
            shape,
            vec![Row::Branch(0)],
            "one dial, and it is the length"
        );

        step(&mut d, Row::Branch(0), 1);
        assert_eq!(d.branches, vec![3], "the chain got longer");
        assert_eq!(d.linking().label(), "chain", "and it is still a chain");
    }

    #[test]
    fn widening_and_narrowing_is_the_linking_row_s_job() {
        let mut d = draft();
        step(&mut d, Row::Linking, 1);
        step(&mut d, Row::Branch(0), 1);
        assert_eq!(d.branches, vec![3]);

        // chain -> fan-out reseeds; a fan's width counts branches, not hops.
        step(&mut d, Row::Linking, 1);
        assert_eq!(d.linking().label(), "fan-out");
        assert_eq!(d.branches, vec![1, 1]);

        step(&mut d, Row::BranchCount, -1);
        assert_eq!(d.branches, vec![1, 1], "floors at two");
        assert!(d.rows(None).contains(&Row::BranchCount));
    }

    #[test]
    fn depths_are_clamped_rather_than_allowed_to_run_away() {
        let mut d = draft();
        d.branches = vec![1, 1];
        step(&mut d, Row::Branch(0), -1);
        assert_eq!(d.branches[0], 1, "never below one");
        d.branches[0] = 8;
        step(&mut d, Row::Branch(0), 1);
        assert_eq!(d.branches[0], 8, "never past the generator's ceiling");
    }

    #[test]
    fn each_branch_depth_is_edited_independently() {
        let mut d = draft();
        d.branches = vec![1, 1, 1];
        step(&mut d, Row::Branch(0), 1);
        step(&mut d, Row::Branch(2), 1);
        step(&mut d, Row::Branch(2), 1);
        assert_eq!(d.branches, vec![2, 1, 3]);
        assert_eq!(d.to_spec().branches, vec![2, 1, 3]);
    }

    #[test]
    fn cycling_the_preset_reseeds_the_shape() {
        let mut d = draft();
        assert!(d.branches.is_empty());
        step(&mut d, Row::Linking, 1);
        assert_eq!(d.linking(), Linking::Chain);
        step(&mut d, Row::Linking, 1);
        assert_eq!(d.linking(), Linking::FanOut);
        assert!(d.branches.len() > 1, "a fan-out needs more than one branch");
    }

    // --- the accordion ----------------------------------------------------

    #[test]
    fn a_capability_shows_its_components_only_while_the_cursor_is_in_it() {
        let mut d = draft();
        step(&mut d, Row::Linking, 2); // fan-out: ingress + two branches
        let keyvalue = row_of(CapabilityKind::Keyvalue);
        step(&mut d, Row::Capability(keyvalue), 1);

        assert!(
            !d.rows(None)
                .iter()
                .any(|row| matches!(row, Row::Place { .. })),
            "nothing is expanded when the cursor is elsewhere"
        );

        let rows = d.rows(Some(keyvalue));
        let places: Vec<&Row> = rows
            .iter()
            .filter(|row| matches!(row, Row::Place { .. }))
            .collect();
        assert_eq!(places.len(), 3, "ingress and two branches");
        assert!(places.iter().all(|row| matches!(
            row,
            Row::Place { capability, .. } if *capability == keyvalue
        )));
    }

    #[test]
    fn a_capability_that_is_off_has_nothing_to_place() {
        let mut d = draft();
        step(&mut d, Row::Linking, 2);
        let keyvalue = row_of(CapabilityKind::Keyvalue);
        assert_eq!(d.placeable(keyvalue), 0, "off, so there is no question yet");

        step(&mut d, Row::Capability(keyvalue), 1);
        assert_eq!(d.placeable(keyvalue), 3);
    }

    #[test]
    fn a_host_flag_offers_no_components_because_nothing_imports_it() {
        // otel is a host flag, not an interface; nothing imports it.
        let mut d = draft();
        step(&mut d, Row::Linking, 2);
        let otel = row_of(CapabilityKind::Otel);
        step(&mut d, Row::Capability(otel), 1);

        assert!(d.is_on(otel));
        assert_eq!(d.placeable(otel), 0);
        assert!(
            !d.rows(Some(otel))
                .iter()
                .any(|row| matches!(row, Row::Place { .. }))
        );
        assert!(
            d.to_spec()
                .placed_capabilities()
                .contains(&Capability::Otel)
        );
        assert!(d.to_spec().validate().is_ok());
    }

    #[test]
    fn the_cursor_moves_by_identity_so_the_accordion_cannot_shift_it() {
        // A saved index would land somewhere else when the accordion shifts rows.
        let mut d = draft();
        step(&mut d, Row::Linking, 2);
        let keyvalue = row_of(CapabilityKind::Keyvalue);
        let logging = row_of(CapabilityKind::Logging);
        step(&mut d, Row::Capability(keyvalue), 1);
        step(&mut d, Row::Capability(logging), 1);

        let open_first = d.rows(Some(keyvalue));
        let open_second = d.rows(Some(logging));
        let at = |rows: &[Row], row: Row| rows.iter().position(|r| *r == row);

        assert_ne!(
            at(&open_first, Row::Capability(logging)),
            at(&open_second, Row::Capability(logging)),
            "the same row sits at different indices depending on what is open"
        );
        assert!(open_second.contains(&Row::Capability(logging)));
    }

    #[test]
    fn the_open_capability_follows_the_cursor() {
        let keyvalue = row_of(CapabilityKind::Keyvalue);
        assert_eq!(opened_by(Row::Capability(keyvalue)), Some(keyvalue));
        assert_eq!(
            opened_by(Row::Place {
                capability: keyvalue,
                node: 2
            }),
            Some(keyvalue),
            "staying among its components keeps it open"
        );
        assert_eq!(opened_by(Row::Trigger), None, "anything else closes it");
    }

    #[test]
    fn moving_down_from_a_capability_enters_its_components() {
        let mut d = draft();
        step(&mut d, Row::Linking, 2);
        let keyvalue = row_of(CapabilityKind::Keyvalue);
        step(&mut d, Row::Capability(keyvalue), 1);

        // Same two steps the event loop takes.
        let rows = d.rows(opened_by(Row::Capability(keyvalue)));
        let index = rows
            .iter()
            .position(|row| *row == Row::Capability(keyvalue))
            .expect("the capability is on screen");
        assert_eq!(
            rows.get(index + 1),
            Some(&Row::Place {
                capability: keyvalue,
                node: 0
            }),
            "drilling in is always downward"
        );
    }

    // --- nothing is pre-ticked -------------------------------------------

    #[test]
    fn switching_a_capability_on_gives_it_to_nobody() {
        // The wizard must not decide.
        let mut d = draft();
        step(&mut d, Row::Linking, 2); // fan-out
        for kind in [
            CapabilityKind::Keyvalue,
            CapabilityKind::Blobstore,
            CapabilityKind::Logging,
        ] {
            step(&mut d, Row::Capability(row_of(kind)), 1);
        }

        assert!(
            d.placement.values().all(BTreeSet::is_empty),
            "nothing should be assigned: {:?}",
            d.placement
        );
        for node in d.to_spec().plan() {
            assert!(
                node.capabilities.is_empty(),
                "{} was given something",
                node.id
            );
        }
    }

    #[test]
    fn a_capability_nobody_imports_blocks_generating() {
        // enter is refused while validate fails.
        let mut d = draft();
        step(&mut d, Row::Linking, 1); // a chain, so there is a choice
        step(&mut d, Row::Capability(row_of(CapabilityKind::Keyvalue)), 1);

        let err = d
            .to_spec()
            .validate()
            .expect_err("nowhere to go")
            .to_string();
        assert!(err.contains("keyvalue"), "{err}");

        place(&mut d, CapabilityKind::Keyvalue, "step2");
        assert!(d.to_spec().validate().is_ok());
    }

    #[test]
    fn a_single_component_workload_needs_no_choice_made() {
        // The one exception: one component means nothing to decide.
        let mut d = draft();
        assert_eq!(d.node_ids().len(), 1);
        step(&mut d, Row::Capability(row_of(CapabilityKind::Keyvalue)), 1);

        let spec = d.to_spec();
        assert!(spec.validate().is_ok());
        assert!(spec.plan()[0].has(&Capability::Keyvalue));
        assert!(
            spec.placement.is_empty(),
            "the only place it could go is where it would have gone anyway, so \
             the recipe stays as short as the flags would have written it: {:?}",
            spec.placement
        );
    }

    #[test]
    fn growing_the_shape_leaves_a_placed_capability_where_it_was_put() {
        // A placed capability is not re-decided when the shape grows.
        let mut d = draft();
        step(&mut d, Row::Capability(row_of(CapabilityKind::Keyvalue)), 1);
        step(&mut d, Row::Linking, 1); // now a chain of two

        let spec = d.to_spec();
        assert!(spec.validate().is_ok());
        assert!(
            spec.plan()[0].has(&Capability::Keyvalue),
            "still on ingress"
        );
        assert!(
            spec.plan()[2].capabilities.is_empty(),
            "not moved to the leaf"
        );
    }

    #[test]
    fn growing_the_shape_gives_the_new_component_nothing() {
        let mut d = draft();
        step(&mut d, Row::Linking, 2); // fan-out of two
        place(&mut d, CapabilityKind::Keyvalue, "branch1");

        step(&mut d, Row::BranchCount, 1); // a third branch appears
        assert_eq!(
            d.placement.get("branch3").map(BTreeSet::len),
            Some(0),
            "a component that appears is not handed the palette"
        );
        assert!(d.to_spec().validate().is_ok(), "branch1 still has it");
    }

    // --- keeping the placement honest ------------------------------------

    #[test]
    fn turning_a_capability_off_clears_it_from_every_component() {
        let mut d = draft();
        step(&mut d, Row::Linking, 1);
        place(&mut d, CapabilityKind::Keyvalue, "step2");
        step(&mut d, Row::Capability(row_of(CapabilityKind::Keyvalue)), 1);

        assert!(d.palette().is_empty());
        assert!(
            d.placement.values().all(BTreeSet::is_empty),
            "a capability that is not in play cannot be imported: {:?}",
            d.placement
        );
        assert!(d.to_spec().validate().is_ok());
    }

    #[test]
    fn reshaping_drops_components_that_no_longer_exist() {
        let mut d = draft();
        step(&mut d, Row::Linking, 2);
        step(&mut d, Row::BranchCount, 1);
        place(&mut d, CapabilityKind::Keyvalue, "branch3");
        assert!(d.placement.contains_key("branch3"));

        step(&mut d, Row::BranchCount, -1);
        assert!(
            !d.placement.contains_key("branch3"),
            "dropped with the branch"
        );
        // keyvalue went with the branch; the failure is the warning the user needs.
        assert!(d.to_spec().validate().is_err());
    }

    #[test]
    fn switching_to_messaging_renames_the_components_and_the_placement_follows() {
        // Messaging renames branchN to workerN; a stale key would wedge validation.
        let mut d = draft();
        step(&mut d, Row::Linking, 2); // fan-out
        place(&mut d, CapabilityKind::Keyvalue, "branch1");
        assert!(d.placement.contains_key("branch1"));

        step(&mut d, Row::Linking, 1); // messaging
        assert_eq!(d.linking(), Linking::Messaging);
        assert!(
            !d.placement.contains_key("branch1"),
            "renamed, not orphaned"
        );
        assert!(d.placement.contains_key("worker1"));
    }

    #[test]
    fn a_component_can_be_given_a_capability_the_default_would_never_reach() {
        // Mid-chain, where the headless default never lands anything.
        let mut d = draft();
        step(&mut d, Row::Linking, 1); // a chain of two, so step1 is a middle
        place(&mut d, CapabilityKind::Logging, "step1");

        let spec = d.to_spec();
        assert!(!spec.placement.is_empty(), "a deviation, so recorded");
        assert!(
            spec.plan()[1].has(&Capability::Logging),
            "the first hop of a chain is not a terminal, yet it has logging"
        );
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn two_components_can_be_given_different_capabilities() {
        let mut d = draft();
        step(&mut d, Row::Linking, 2); // fan-out
        place(&mut d, CapabilityKind::Keyvalue, "branch1");
        place(&mut d, CapabilityKind::Blobstore, "branch2");

        let nodes = d.to_spec().plan();
        assert_eq!(nodes[1].capabilities, [Capability::Keyvalue]);
        assert_eq!(nodes[2].capabilities, [Capability::Blobstore]);
        assert!(nodes[0].capabilities.is_empty());
    }

    // --- the host-carrying two --------------------------------------------

    #[test]
    fn egress_and_grpc_sit_in_the_same_index_space_as_the_rest() {
        let mut d = draft();
        step(&mut d, Row::Capability(EGRESS_INDEX), 1);
        step(&mut d, Row::Capability(GRPC_INDEX), 1);

        let capabilities = d.to_spec().capabilities;
        assert!(capabilities.iter().any(|c| matches!(
            c,
            Capability::HttpEgress { host } if host == DEFAULT_EGRESS_HOST
        )));
        assert!(capabilities.iter().any(|c| matches!(
            c,
            Capability::Grpc { host } if host == DEFAULT_GRPC_HOST
        )));

        step(&mut d, Row::Capability(EGRESS_INDEX), 1);
        assert!(d.egress.is_none());
    }

    #[test]
    fn a_host_carrying_capability_is_placed_like_any_other() {
        let mut d = draft();
        step(&mut d, Row::Linking, 2);
        step(&mut d, Row::Capability(EGRESS_INDEX), 1);
        assert!(d.to_spec().validate().is_err(), "nowhere yet");

        let node = node_of(&d, "branch2");
        step(
            &mut d,
            Row::Place {
                capability: EGRESS_INDEX,
                node,
            },
            1,
        );
        let spec = d.to_spec();
        assert!(spec.validate().is_ok());
        assert_eq!(spec.plan()[2].egress_host(), Some(DEFAULT_EGRESS_HOST));
        assert!(spec.plan()[1].egress_host().is_none());
    }

    // --- what reaches the spec --------------------------------------------

    #[test]
    fn toggling_a_capability_reaches_the_spec() {
        let mut d = draft();
        let keyvalue = row_of(CapabilityKind::Keyvalue);
        step(&mut d, Row::Capability(keyvalue), 1);
        assert!(d.to_spec().capabilities.contains(&Capability::Keyvalue));
        step(&mut d, Row::Capability(keyvalue), 1);
        assert!(d.to_spec().capabilities.is_empty(), "toggles off again");
    }

    #[test]
    fn a_seeded_placement_opens_showing_that_placement() {
        // Reopening a recipe must not silently reset it.
        let mut spec = Spec {
            name: "seeded".into(),
            trigger: Trigger::Http,
            edition: Edition::P2,
            branches: vec![2],
            over_messaging: false,
            capabilities: vec![Capability::Logging],
            placement: BTreeMap::new(),
        };
        spec.placement = [("step1".to_string(), vec!["logging".to_string()])]
            .into_iter()
            .collect();

        let d = Draft::from_spec(&spec);

        let back = d.to_spec();
        assert_eq!(
            back.placement.get("step2").map(Vec::as_slice),
            Some(&[][..]),
            "a component the recipe left out stays empty"
        );
        assert!(back.plan()[1].has(&Capability::Logging), "step1 keeps it");
        assert!(!back.plan()[2].has(&Capability::Logging));
    }

    #[test]
    fn an_invalid_combination_is_surfaced_rather_than_generated() {
        // Fanning out over messaging would publish onto its own subscription.
        let mut d = draft();
        d.trigger = Trigger::Messaging;
        d.branches = vec![1, 1];
        d.over_messaging = true;
        assert!(d.to_spec().validate().is_err());
    }

    // --- the screen --------------------------------------------------------

    /// The whole screen with the cursor on `row`.
    fn screen_at(d: &Draft, row: Row) -> String {
        let open = opened_by(row);
        let rows = d.rows(open);
        screen(
            d,
            &View {
                rows: &rows,
                cursor: row,
                open,
            },
        )
    }

    #[test]
    fn the_preview_draws_what_would_be_generated() {
        let mut d = draft();
        d.branches = vec![1, 1, 1];
        let drawn = diagram_with(&preview_topology(&d.to_spec()), &Decor::default()).join("\n");
        for branch in ["branch1", "branch2", "branch3"] {
            assert!(drawn.contains(branch), "missing {branch}:\n{drawn}");
        }
    }

    #[test]
    fn a_capability_with_nowhere_to_go_says_so() {
        let mut d = draft();
        step(&mut d, Row::Linking, 1);
        let keyvalue = row_of(CapabilityKind::Keyvalue);
        step(&mut d, Row::Capability(keyvalue), 1);

        let out = screen_at(&d, Row::Capability(keyvalue));
        assert!(out.contains("nowhere"), "{out}");
        assert!(out.contains(YELLOW), "and says it in yellow");

        place(&mut d, CapabilityKind::Keyvalue, "step2");
        let out = screen_at(&d, Row::Capability(keyvalue));
        assert!(out.contains("on 1 of 3"), "{out}");
        assert!(!out.contains("nowhere"), "{out}");
    }

    #[test]
    fn the_boxes_carry_what_their_component_imports() {
        let mut d = draft();
        step(&mut d, Row::Linking, 2);
        place(&mut d, CapabilityKind::Keyvalue, "branch2");
        place(&mut d, CapabilityKind::Logging, "ingress");

        let notes = d.notes();
        assert_eq!(notes.get("ingress").map(String::as_str), Some("logging"));
        assert_eq!(notes.get("branch2").map(String::as_str), Some("keyvalue"));
        assert!(notes.get("branch1").is_none(), "nothing to say");

        let out = screen_at(&d, Row::Trigger);
        assert!(out.contains("logging") && out.contains("keyvalue"), "{out}");
    }

    #[test]
    fn a_component_row_focuses_that_box() {
        let mut d = draft();
        step(&mut d, Row::Linking, 2);
        let keyvalue = row_of(CapabilityKind::Keyvalue);
        step(&mut d, Row::Capability(keyvalue), 1);

        let node = node_of(&d, "branch1");
        let rows = d.rows(Some(keyvalue));
        let view = View {
            rows: &rows,
            cursor: Row::Place {
                capability: keyvalue,
                node,
            },
            open: Some(keyvalue),
        };
        assert_eq!(view.focus(&d).as_deref(), Some("branch1"));

        // Nowhere else in the column focuses anything.
        let view = View {
            rows: &rows,
            cursor: Row::Trigger,
            open: None,
        };
        assert_eq!(view.focus(&d), None);
    }

    #[test]
    fn the_screen_keeps_its_columns_aligned() {
        // Without display_width, styled lines measure wide and the separator wanders.
        let mut d = draft();
        step(&mut d, Row::Linking, 2);
        let keyvalue = row_of(CapabilityKind::Keyvalue);
        step(&mut d, Row::Capability(keyvalue), 1);

        let out = screen_at(
            &d,
            Row::Place {
                capability: keyvalue,
                node: 1,
            },
        );
        let columns: Vec<usize> = out
            .lines()
            .filter(|line| line.contains('│'))
            .map(|line| display_width(line.split('│').next().unwrap_or_default()))
            .collect();
        assert!(columns.len() > 5, "expected a full screen: {out}");
        assert!(
            columns.windows(2).all(|w| w[0] == w[1]),
            "the separator wanders: {columns:?}"
        );
    }

    #[test]
    fn every_capability_description_starts_in_the_same_column() {
        // Located by description text; a fixed-column escape would always pass.
        let d = draft();
        let out = screen_at(&d, Row::Trigger);

        let described: Vec<(&str, &str)> = CapabilityKind::ALL
            .iter()
            .map(|kind| match Capability::from(*kind) {
                Capability::Keyvalue => ("keyvalue", "read and write a key-value bucket"),
                Capability::Blobstore => ("blobstore", "create and use a blob container"),
                Capability::Config => ("config", "read runtime configuration"),
                Capability::Logging => ("logging", "write to the host log"),
                Capability::Postgres => ("postgres", "query a database"),
                _ => ("otel", "host-side tracing"),
            })
            .chain([
                (EGRESS.label(), "call out over HTTP"),
                (GRPC.label(), "call out over gRPC"),
            ])
            .collect();

        let mut starts = Vec::new();
        for (label, description) in &described {
            let line = out
                .lines()
                .find(|line| line.contains(label) && line.contains(description))
                .unwrap_or_else(|| panic!("no row for {label}:\n{out}"));
            let at = line
                .find(description)
                .expect("the row was found by this needle");
            starts.push((*label, display_width(line.get(..at).unwrap_or_default())));
        }

        assert_eq!(starts.len(), CapabilityKind::ALL.len() + 2);
        let first = starts.first().map(|(_, at)| *at).unwrap_or_default();
        assert!(
            starts.iter().all(|(_, at)| *at == first),
            "descriptions start at {starts:?}:\n{out}"
        );
    }

    #[test]
    fn the_screen_shows_the_shape_and_the_capability_being_placed() {
        let mut d = draft();
        step(&mut d, Row::Linking, 2);
        let keyvalue = row_of(CapabilityKind::Keyvalue);
        step(&mut d, Row::Capability(keyvalue), 1);

        let out = screen_at(
            &d,
            Row::Place {
                capability: keyvalue,
                node: 1,
            },
        );
        assert!(out.contains("trigger") && out.contains("linking"), "{out}");
        assert!(out.contains("ingress") && out.contains("branch2"), "{out}");
        assert!(out.contains("branch1"), "{out}");
        assert!(
            out.contains("give this component"),
            "the hint follows: {out}"
        );
    }
}
