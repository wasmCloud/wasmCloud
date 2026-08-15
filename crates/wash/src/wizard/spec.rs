//! The answers a wizard run collects ([`Spec`]) and the node graph they imply.
//! The wizard writes boilerplate, not behaviour: every generated body is a `TODO`.
//! A [`Spec`] is serialisable, so a run replays with `--recipe`.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use wash_topology::{Role, Shape};

/// What the host calls to start work; matches `Ingress` in `wash_runtime::host::trigger_service`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[clap(rename_all = "kebab-case")]
pub enum Trigger {
    /// An HTTP request. Exports `wasi:http/incoming-handler`.
    Http,
    /// A message on a subscribed subject. Exports `wasmcloud:messaging/handler`.
    Messaging,
    /// A long-lived process. Exports `wasi:cli/run`.
    Service,
}

impl Trigger {
    pub fn label(self) -> &'static str {
        match self {
            Trigger::Http => "http",
            Trigger::Messaging => "messaging",
            Trigger::Service => "service",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Trigger::Http => "an HTTP request arrives",
            Trigger::Messaging => "a message lands on a subscribed subject",
            Trigger::Service => "a long-lived process, started with the host",
        }
    }

    /// Every trigger, in the order the builder cycles them.
    pub const ALL: [Trigger; 3] = [Trigger::Http, Trigger::Messaging, Trigger::Service];

    /// The edition a trigger demands, if any (services are p3-only).
    pub fn required_edition(self) -> Option<Edition> {
        match self {
            Trigger::Service => Some(Edition::P3),
            Trigger::Http | Trigger::Messaging => None,
        }
    }

    fn role(self) -> Role {
        match self {
            Trigger::Http => Role::Ingress,
            Trigger::Messaging => Role::Worker,
            Trigger::Service => Role::Service,
        }
    }
}

/// How the triggered component reaches the rest of the workload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[clap(rename_all = "kebab-case")]
pub enum Linking {
    /// One component; nothing to reach.
    None,
    /// Calls the next component, which calls the next.
    Chain,
    /// Calls several components directly, each over its own interface.
    FanOut,
    /// Publishes to workers that subscribe by subject.
    Messaging,
}

impl Linking {
    /// Expand this shorthand into per-branch depths plus whether links go over the broker.
    pub fn expand(self, count: usize) -> (Vec<usize>, bool) {
        match self {
            Linking::None => (Vec::new(), false),
            Linking::Chain => (vec![count.max(1)], false),
            Linking::FanOut => (vec![1; count.max(1)], false),
            Linking::Messaging => (vec![1; count.max(1)], true),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Linking::None => "none",
            Linking::Chain => "chain",
            Linking::FanOut => "fan-out",
            Linking::Messaging => "messaging",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Linking::None => "a single component",
            Linking::Chain => "calls through a series of components",
            Linking::FanOut => "calls several components directly",
            Linking::Messaging => "publishes to workers by subject",
        }
    }

    /// Every linking style, in the order the builder cycles them.
    pub const ALL: [Linking; 4] = [
        Linking::None,
        Linking::Chain,
        Linking::FanOut,
        Linking::Messaging,
    ];
}

/// Which preview of the component model to generate against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
#[clap(rename_all = "kebab-case")]
pub enum Edition {
    /// `wasi:http/incoming-handler@0.2.2`, synchronous exports, wstd macros.
    P2,
    /// `wasi:http/handler@0.3.0`, async exports, raw wit-bindgen.
    P3,
}

impl Edition {
    pub fn label(self) -> &'static str {
        match self {
            Edition::P2 => "p2",
            Edition::P3 => "p3",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Edition::P2 => "wasi 0.2 — synchronous, widest support",
            Edition::P3 => "wasi 0.3 — async handlers and streams",
        }
    }

    pub const ALL: [Edition; 2] = [Edition::P2, Edition::P3];

    pub fn is_p3(self) -> bool {
        matches!(self, Edition::P3)
    }

    // Trigger contracts, per edition — shared by the writers, the manifest, and the reverse reader.

    /// What an HTTP ingress exports.
    pub fn http_ingress_export(self) -> &'static str {
        match self {
            Edition::P2 => "wasi:http/incoming-handler@0.2.2",
            Edition::P3 => "wasi:http/handler@0.3.0",
        }
    }

    /// What a service exports; the p2 arm survives only for reading back old manifests.
    pub fn cli_run_export(self) -> &'static str {
        match self {
            Edition::P2 => "wasi:cli/run@0.2.0",
            Edition::P3 => "wasi:cli/run@0.3.0",
        }
    }

    /// What a messaging worker exports.
    pub fn messaging_handler(self) -> &'static str {
        match self {
            Edition::P2 => "wasmcloud:messaging/handler@0.2.0",
            Edition::P3 => "wasmcloud:messaging/handler@0.3.0",
        }
    }

    /// What a publishing trigger imports.
    pub fn messaging_consumer(self) -> &'static str {
        match self {
            Edition::P2 => "wasmcloud:messaging/consumer@0.2.0",
            Edition::P3 => "wasmcloud:messaging/consumer@0.3.0",
        }
    }

    /// The HTTP types companion of [`http_ingress_export`](Self::http_ingress_export).
    pub fn http_types_import(self) -> &'static str {
        match self {
            Edition::P2 => "wasi:http/types@0.2.2",
            Edition::P3 => "wasi:http/types@0.3.0",
        }
    }

    /// The monotonic clock a generated service sleeps on.
    pub fn monotonic_clock_import(self) -> &'static str {
        match self {
            Edition::P2 => "wasi:clocks/monotonic-clock@0.2.0",
            Edition::P3 => "wasi:clocks/monotonic-clock@0.3.0",
        }
    }
}

/// A host capability wired into the terminal component(s); boilerplate only, every body a `TODO`.
///
/// # Coverage ledger
///
/// Host features deliberately not offered:
/// - `wasi:tls` — p3-only, behind a non-default host feature.
/// - `wasi:sockets` service shapes — a shape, not a toggle; `templates/service-tcp` is the pattern.
/// - volumes / `wasi:filesystem` — belongs to workload-definition work, not a capability toggle.
/// - named/multiplexed instances — needs a placement-model extension first.
/// - `poolSize`/`maxInvocations`/`maxConcurrency`, per-component allowlists — runtime policy.
/// - cron/scheduled — not a host trigger; a service loop already expresses it.
/// - `wasmcloud:keyvalue/watcher` — declared in WIT, served by nothing.
/// - `wasi:otel`'s own interfaces — processor callbacks; the `Otel` variant is the host-side flag.
/// - registry-sourced catalog entries — the picker lists repositories, not registries.
///
/// Deferred structural debts:
/// - `Index.origins` parallels `topologies`, matched by pointer identity in `repo_for`.
/// - capability config contributions live as if-blocks in `generate::build_config`.
/// - `wizard-verify` builds every case cold; no shared `CARGO_TARGET_DIR`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    /// `wasi:keyvalue/store` — in-memory by default, so it needs nothing running.
    Keyvalue,
    /// `wasi:blobstore/blobstore` — in-memory by default.
    Blobstore,
    /// `wasi:config/store` — reads keys the generator seeds into `workload.config`.
    Config,
    /// `wasi:logging/logging`.
    Logging,
    /// `wasmcloud:postgres/query` — needs `dev.postgres_url` to do anything.
    Postgres,
    /// Host-side tracing via `dev.wasi_otel`; config only, no imports.
    Otel,
    /// `wasmcloud:secrets` — reveal-gated secret handles; p3-only.
    Secrets,
    /// `wasi:webgpu` — GPU compute; p3-only, macOS/Linux.
    Webgpu,
    /// `wasi:http/outgoing-handler`, with the host allowlisted.
    HttpEgress { host: String },
    /// gRPC over HTTP egress: a `.proto` plus tonic scaffold, not a WIT interface of its own.
    Grpc { host: String },
}

impl Capability {
    pub fn label(&self) -> &'static str {
        match self {
            Capability::Keyvalue => "keyvalue",
            Capability::Blobstore => "blobstore",
            Capability::Config => "config",
            Capability::Logging => "logging",
            Capability::Postgres => "postgres",
            Capability::Otel => "otel",
            Capability::Secrets => "secrets",
            Capability::Webgpu => "webgpu",
            Capability::HttpEgress { .. } => "http-egress",
            Capability::Grpc { .. } => "grpc",
        }
    }

    pub fn describe(&self) -> &'static str {
        match self {
            Capability::Keyvalue => "read and write a key-value bucket",
            Capability::Blobstore => "create and use a blob container",
            Capability::Config => "read runtime configuration",
            Capability::Logging => "write to the host log",
            Capability::Postgres => "query a database (needs dev.postgres_url)",
            Capability::Otel => "host-side tracing (config only, no code)",
            Capability::Secrets => "fetch secrets as unloggable handles (p3)",
            Capability::Webgpu => "run GPU compute (p3; macOS/Linux)",
            Capability::HttpEgress { .. } => "call out over HTTP",
            Capability::Grpc { .. } => "call out over gRPC (proto + tonic scaffold)",
        }
    }

    /// Whether the generated component imports anything for this (`Otel` is config-only).
    pub fn emits_code(&self) -> bool {
        !matches!(self, Capability::Otel)
    }

    // The p2→p3 move is a package switch, not a version bump.

    /// WIT imports this capability puts in its component's world; versions are what the host serves.
    pub fn wit_imports(&self, edition: Edition) -> &'static [&'static str] {
        match (self, edition) {
            (Capability::Keyvalue, Edition::P2) => &["wasi:keyvalue/store@0.2.0-draft"],
            (Capability::Keyvalue, Edition::P3) => &["wasmcloud:keyvalue/store@0.2.0"],
            (Capability::Blobstore, Edition::P2) => &["wasi:blobstore/blobstore@0.2.0-draft"],
            (Capability::Blobstore, Edition::P3) => &["wasmcloud:blobstore/blobstore@0.1.0"],
            // No p3 revision exists for these two; a p3 world imports the same interface.
            (Capability::Config, _) => &["wasi:config/store@0.2.0-rc.1"],
            (Capability::Logging, _) => &["wasi:logging/logging@0.1.0-draft"],
            (Capability::Postgres, Edition::P2) => &["wasmcloud:postgres/query@0.1.1-draft"],
            (Capability::Postgres, Edition::P3) => &[
                "wasmcloud:postgres/query@0.2.0",
                "wasmcloud:postgres/types@0.2.0",
            ],
            (Capability::Otel, _) => &[],
            (Capability::Secrets, _) => &[
                "wasmcloud:secrets/store@2.1.0",
                "wasmcloud:secrets/reveal@2.1.0",
            ],
            (Capability::Webgpu, _) => &["wasi:webgpu/webgpu@0.3.0-rc.2"],
            // p3 has no separate outgoing-handler; outbound HTTP imports the unified `handler`.
            (Capability::HttpEgress { .. }, Edition::P2) => &["wasi:http/outgoing-handler@0.2.2"],
            (Capability::HttpEgress { .. }, Edition::P3) => &["wasi:http/handler@0.3.0"],
            // p2-only; `validate()` refuses the p3 pairing.
            (Capability::Grpc { .. }, _) => &["wasi:http/outgoing-handler@0.2.2"],
        }
    }

    /// The interface a derived manifest records — the first of [`wit_imports`](Self::wit_imports).
    pub fn manifest_interface(&self, edition: Edition) -> Option<&'static str> {
        self.wit_imports(edition).first().copied()
    }

    /// Whether this capability's helper is an `async fn`, per edition.
    pub fn is_async(&self, edition: Edition) -> bool {
        match edition {
            Edition::P2 => matches!(self, Capability::Grpc { .. }),
            Edition::P3 => matches!(
                self,
                Capability::Keyvalue
                    | Capability::Blobstore
                    | Capability::Postgres
                    | Capability::Secrets
                    | Capability::Webgpu
                    | Capability::HttpEgress { .. }
                    | Capability::Grpc { .. }
            ),
        }
    }
}

/// The payload-free capabilities, for `--capability` and the builder's toggle list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum CapabilityKind {
    Keyvalue,
    Blobstore,
    Config,
    Logging,
    Postgres,
    Otel,
    Secrets,
    Webgpu,
}

impl CapabilityKind {
    /// Every toggleable capability, in the order the builder shows them.
    pub const ALL: [CapabilityKind; 8] = [
        CapabilityKind::Keyvalue,
        CapabilityKind::Blobstore,
        CapabilityKind::Config,
        CapabilityKind::Logging,
        CapabilityKind::Postgres,
        CapabilityKind::Otel,
        CapabilityKind::Secrets,
        CapabilityKind::Webgpu,
    ];
}

impl From<CapabilityKind> for Capability {
    fn from(kind: CapabilityKind) -> Self {
        match kind {
            CapabilityKind::Keyvalue => Capability::Keyvalue,
            CapabilityKind::Blobstore => Capability::Blobstore,
            CapabilityKind::Config => Capability::Config,
            CapabilityKind::Logging => Capability::Logging,
            CapabilityKind::Postgres => Capability::Postgres,
            CapabilityKind::Otel => Capability::Otel,
            CapabilityKind::Secrets => Capability::Secrets,
            CapabilityKind::Webgpu => Capability::Webgpu,
        }
    }
}

/// A complete set of wizard answers; serialisable for `--from` and `--recipe`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spec {
    /// Project directory name; also the WIT namespace.
    pub name: String,
    pub trigger: Trigger,
    /// Which preview of the component model to generate against.
    #[serde(default = "default_edition")]
    pub edition: Edition,
    /// One entry per branch off the trigger, giving that branch's depth (`[]` = single component).
    #[serde(default)]
    pub branches: Vec<usize>,
    /// Whether the trigger reaches its branches over the broker.
    #[serde(default)]
    pub over_messaging: bool,
    /// The capabilities in play across the workload — the palette.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<Capability>,
    /// Node id to the [`Capability::label`]s that node imports. Empty means the default
    /// (every terminal gets the palette); non-empty is authoritative — unnamed nodes get nothing.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub placement: BTreeMap<String, Vec<String>>,
}

fn default_edition() -> Edition {
    Edition::P2
}

/// Upper bound on generated components.
const MAX_COUNT: usize = 8;

impl Spec {
    /// The shorthand that best describes this shape; derived, never stored.
    pub fn linking(&self) -> Linking {
        match (self.over_messaging, self.branches.len()) {
            (true, _) => Linking::Messaging,
            (false, 0) => Linking::None,
            (false, 1) => Linking::Chain,
            (false, _) => Linking::FanOut,
        }
    }

    /// Whether the shape is lopsided — branches of differing depth.
    pub fn is_irregular(&self) -> bool {
        self.branches.len() > 1 && self.branches.iter().any(|d| *d != self.branches[0])
    }

    /// Total components past the trigger.
    pub fn component_count(&self) -> usize {
        self.branches.iter().sum()
    }

    /// Every component id in [`Spec::plan`] order, paired with whether it ends its branch.
    pub fn node_ids(&self) -> Vec<(String, bool)> {
        let mut ids = vec![("ingress".to_string(), self.branches.is_empty())];
        for (branch, depth) in self.branches.iter().enumerate() {
            for hop in 1..=*depth {
                ids.push((self.node_id(branch, hop), hop == *depth));
            }
        }
        ids
    }

    /// Which capabilities one component imports, in stable palette order.
    fn capabilities_for(&self, id: &str, is_terminal: bool) -> Vec<Capability> {
        if self.placement.is_empty() {
            return if is_terminal {
                self.capabilities.clone()
            } else {
                Vec::new()
            };
        }
        let placed = self
            .placement
            .get(id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        self.capabilities
            .iter()
            .filter(|capability| placed.iter().any(|label| label == capability.label()))
            .cloned()
            .collect()
    }

    /// This spec's placement spelled out: every component, with what it gets.
    pub fn resolved_placement(&self) -> BTreeMap<String, Vec<String>> {
        self.node_ids()
            .into_iter()
            .map(|(id, is_terminal)| {
                let labels = self
                    .capabilities_for(&id, is_terminal)
                    .iter()
                    .map(|c| c.label().to_string())
                    .collect();
                (id, labels)
            })
            .collect()
    }

    /// The placement an empty [`Spec::placement`] stands for.
    pub fn default_placement(&self) -> BTreeMap<String, Vec<String>> {
        Spec {
            placement: BTreeMap::new(),
            ..self.clone()
        }
        .resolved_placement()
    }

    /// The capabilities some component actually imports; config-only ones
    /// (`otel`) are always included, since nothing can place them.
    pub fn placed_capabilities(&self) -> Vec<Capability> {
        if self.placement.is_empty() {
            return self.capabilities.clone();
        }
        let placed: std::collections::BTreeSet<&str> = self
            .placement
            .values()
            .flatten()
            .map(String::as_str)
            .collect();
        self.capabilities
            .iter()
            .filter(|capability| !capability.emits_code() || placed.contains(capability.label()))
            .cloned()
            .collect()
    }

    /// Reject anything that would emit a project that cannot build.
    pub fn validate(&self) -> Result<()> {
        if !is_wit_ident(&self.name) {
            bail!(
                "project name '{}' is not usable as a WIT namespace: use lowercase \
                 words separated by hyphens, each starting with a letter (e.g. my-app)",
                self.name
            );
        }
        if self.branches.iter().any(|depth| *depth == 0) {
            bail!("a branch needs at least one component; use no branches for a single component");
        }
        if self.branches.len() > MAX_COUNT {
            bail!(
                "{} branches is more than this generates ({MAX_COUNT} max)",
                self.branches.len()
            );
        }
        if self.component_count() > MAX_COUNT {
            bail!(
                "{} components is more than this generates ({MAX_COUNT} max); \
                 scaffold a smaller shape and grow it",
                self.component_count()
            );
        }
        if self.trigger == Trigger::Messaging && self.over_messaging {
            bail!(
                "a messaging trigger cannot also fan out over messaging: the worker \
                 would publish to itself. Use `--linking chain` or `--linking fan-out`"
            );
        }
        if let Some(required) = self.trigger.required_edition()
            && self.edition != required
        {
            bail!(
                "a {} trigger is {}: the host serves services (and co-drives \
                 their handlers) on the p3 path. Use `--edition {}`",
                self.trigger.label(),
                required.label(),
                required.label()
            );
        }
        if !self.edition.is_p3() {
            for capability in &self.capabilities {
                if matches!(capability, Capability::Secrets | Capability::Webgpu) {
                    bail!(
                        "the {} capability is wasip3-only (its interface is `async func`). \
                         Use `--edition p3`",
                        capability.label()
                    );
                }
            }
        }
        if self.edition.is_p3()
            && self
                .capabilities
                .iter()
                .any(|c| matches!(c, Capability::Grpc { .. }))
        {
            bail!(
                "the gRPC scaffold is p2-only today (its transport adapter is built \
                 on wstd). Use `--edition p2`, or `--egress` for plain HTTP egress"
            );
        }
        for capability in &self.capabilities {
            if let Capability::HttpEgress { host } | Capability::Grpc { host } = capability {
                host.parse::<wash_runtime::host::allowed_hosts::AllowedHost>()
                    .map_err(|err| {
                        anyhow::anyhow!(
                            "'{host}' is not a valid egress host ({err}); \
                         use e.g. api.example.com, *.example.com, or https://host:8443"
                        )
                    })?;
            }
        }
        // Checked after the shape, so the ids named are ones a valid shape produces.
        if !self.placement.is_empty() {
            let ids: Vec<String> = self.node_ids().into_iter().map(|(id, _)| id).collect();
            for (node, labels) in &self.placement {
                if !ids.contains(node) {
                    bail!(
                        "placement names '{node}', which is not a component of this shape. \
                         It has: {}",
                        ids.join(", ")
                    );
                }
                for label in labels {
                    if !self.capabilities.iter().any(|c| c.label() == label) {
                        bail!(
                            "'{node}' is given '{label}', which is not one of this \
                             workload's capabilities ({})",
                            if self.capabilities.is_empty() {
                                "none are selected".to_string()
                            } else {
                                self.capabilities
                                    .iter()
                                    .map(Capability::label)
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            }
                        );
                    }
                }
            }

            let placed = self.placed_capabilities();
            if let Some(homeless) = self
                .capabilities
                .iter()
                .find(|capability| !placed.contains(capability))
            {
                bail!(
                    "'{}' is selected but nothing imports it — place it on a \
                     component, or switch it off",
                    homeless.label()
                );
            }
        }
        Ok(())
    }

    /// How the topology deriver will classify this project once built.
    pub fn shape(&self) -> Shape {
        match self.linking() {
            Linking::None => match self.trigger {
                Trigger::Service => Shape::Service,
                _ => Shape::Single,
            },
            Linking::Chain => Shape::Chain,
            Linking::FanOut | Linking::Messaging => Shape::FanOut,
        }
    }

    /// WIT package this project's own interfaces live in.
    pub fn wit_package(&self) -> String {
        format!("{}:app@0.1.0", self.name)
    }

    /// Rust path segment for the WIT namespace (`my-app` -> `my_app`).
    pub fn wit_namespace_ident(&self) -> String {
        rust_ident(&self.name)
    }

    /// Whether the trigger component publishes to workers over the broker.
    pub fn links_over_messaging(&self) -> bool {
        self.over_messaging && !self.branches.is_empty()
    }

    /// Whether any component in this workload touches the broker at all.
    pub fn uses_messaging(&self) -> bool {
        self.trigger == Trigger::Messaging || self.links_over_messaging()
    }

    /// The components to emit, trigger first, then each branch in order. Each direct
    /// edge gets its own interface; messaging workers share one handler instead.
    pub fn plan(&self) -> Vec<PlannedNode> {
        let terminal_is_trigger = self.branches.is_empty();
        let mut nodes = vec![PlannedNode {
            id: "ingress".to_string(),
            world: "ingress".to_string(),
            role: self.trigger.role(),
            is_trigger: true,
            exports: None,
            imports: Vec::new(),
            subscriptions: match self.trigger {
                Trigger::Messaging => vec!["tasks.ingress".to_string()],
                _ => Vec::new(),
            },
            capabilities: self.capabilities_for("ingress", terminal_is_trigger),
        }];

        for (branch, depth) in self.branches.iter().enumerate() {
            // Each node registers with its predecessor only, so middle hops are wired once.
            let mut predecessor = 0usize;

            for hop in 1..=*depth {
                let id = self.node_id(branch, hop);
                let is_last = hop == *depth;

                // A broker branch's first hop is reached by subject, not by an import.
                let over_broker = self.links_over_messaging() && hop == 1;
                if !over_broker {
                    nodes[predecessor].imports.push(id.clone());
                }

                nodes.push(PlannedNode {
                    world: format!("{id}-component"),
                    role: if over_broker {
                        Role::Worker
                    } else {
                        Role::Component
                    },
                    is_trigger: false,
                    exports: (!over_broker).then(|| id.clone()),
                    imports: Vec::new(),
                    subscriptions: if over_broker {
                        vec![format!("tasks.{id}")]
                    } else {
                        Vec::new()
                    },
                    capabilities: self.capabilities_for(&id, is_last),
                    id,
                });
                predecessor = nodes.len() - 1;
            }
        }
        nodes
    }

    /// Name for hop `hop` (1-based) of branch `branch` (0-based); every segment
    /// starts with a letter, which keeps it a valid WIT identifier.
    fn node_id(&self, branch: usize, hop: usize) -> String {
        let stem = if self.branches.len() == 1 {
            return format!("step{hop}");
        } else if self.links_over_messaging() {
            format!("worker{}", branch + 1)
        } else {
            format!("branch{}", branch + 1)
        };
        if hop == 1 {
            stem
        } else {
            format!("{stem}step{hop}")
        }
    }
}

/// One component to emit.
#[derive(Debug, Clone)]
pub struct PlannedNode {
    /// Crate name and directory name.
    pub id: String,
    /// World this component's bindings are generated from.
    pub world: String,
    pub role: Role,
    /// Whether the host drives this component directly.
    pub is_trigger: bool,
    /// The project-local interface this component exports, if any.
    pub exports: Option<String>,
    /// Project-local interfaces this component imports.
    pub imports: Vec<String>,
    /// Messaging subjects this component subscribes to.
    pub subscriptions: Vec<String>,
    pub capabilities: Vec<Capability>,
}

impl PlannedNode {
    /// Path the build writes this component to (cdylib naming: hyphens become underscores).
    pub fn wasm_path(&self) -> String {
        let stem = self.id.replace('-', "_");
        format!("target/wasm32-wasip2/release/{stem}.wasm")
    }

    /// The outbound host, when this node has an egress capability.
    pub fn egress_host(&self) -> Option<&str> {
        self.capabilities.iter().find_map(|c| match c {
            Capability::HttpEgress { host } | Capability::Grpc { host } => Some(host.as_str()),
            _ => None,
        })
    }

    pub fn has(&self, capability: &Capability) -> bool {
        self.capabilities.contains(capability)
    }
}

/// `my-app` -> `my_app`, for referring to WIT names from Rust.
pub fn rust_ident(kebab: &str) -> String {
    kebab.replace('-', "_")
}

/// WIT identifier check: hyphen-separated lowercase words, each starting with a letter.
fn is_wit_ident(value: &str) -> bool {
    !value.is_empty()
        && value.split('-').all(|word| {
            word.starts_with(|c: char| c.is_ascii_lowercase())
                && word
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(trigger: Trigger, linking: Linking, count: usize) -> Spec {
        let (branches, over_messaging) = linking.expand(count);
        Spec {
            name: "demo".into(),
            trigger,
            edition: Edition::P2,
            branches,
            over_messaging,
            capabilities: Vec::new(),
            placement: BTreeMap::new(),
        }
    }

    /// A placement map from `(node, labels)` pairs.
    fn placed(entries: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        entries
            .iter()
            .map(|(node, labels)| {
                (
                    (*node).to_string(),
                    labels.iter().map(|l| (*l).to_string()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn a_chain_links_each_hop_to_the_next_exactly_once() {
        let nodes = spec(Trigger::Http, Linking::Chain, 2).plan();
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].imports, ["step1"]);
        assert_eq!(
            nodes[1].imports,
            ["step2"],
            "middle hop wired once, not twice"
        );
        assert!(nodes[2].imports.is_empty(), "the last hop calls nothing");
    }

    #[test]
    fn fan_out_gives_every_branch_its_own_interface() {
        let nodes = spec(Trigger::Http, Linking::FanOut, 3).plan();
        let exports: Vec<&str> = nodes.iter().filter_map(|n| n.exports.as_deref()).collect();
        assert_eq!(exports, ["branch1", "branch2", "branch3"]);
        let unique: std::collections::BTreeSet<_> = exports.iter().collect();
        assert_eq!(unique.len(), exports.len());
    }

    #[test]
    fn messaging_workers_share_one_handler_and_differ_by_subject() {
        let nodes = spec(Trigger::Http, Linking::Messaging, 2).plan();
        assert!(nodes.iter().skip(1).all(|n| n.exports.is_none()));
        assert_eq!(nodes[1].subscriptions, ["tasks.worker1"]);
        assert_eq!(nodes[2].subscriptions, ["tasks.worker2"]);
    }

    #[test]
    fn a_messaging_trigger_subscribes_and_exports_no_http() {
        let nodes = spec(Trigger::Messaging, Linking::None, 1).plan();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].role, Role::Worker);
        assert_eq!(nodes[0].subscriptions, ["tasks.ingress"]);
    }

    #[test]
    fn trigger_and_linking_compose_independently() {
        let nodes = spec(Trigger::Messaging, Linking::Chain, 2).plan();
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].role, Role::Worker, "still message-triggered");
        assert_eq!(nodes[0].imports, ["step1"], "and still chains");
    }

    #[test]
    fn every_artifact_name_is_the_underscored_component_id() {
        let nodes = spec(Trigger::Service, Linking::None, 1).plan();
        assert!(nodes[0].wasm_path().ends_with("/ingress.wasm"));

        let cdylib = spec(Trigger::Http, Linking::Chain, 1).plan();
        assert!(cdylib[1].wasm_path().ends_with("/step1.wasm"));
    }

    #[test]
    fn capabilities_land_on_the_terminal_components_only() {
        let mut s = spec(Trigger::Http, Linking::Chain, 2);
        s.capabilities = vec![Capability::Keyvalue];
        let nodes = s.plan();
        assert!(nodes[0].capabilities.is_empty());
        assert!(nodes[1].capabilities.is_empty());
        assert!(nodes[2].has(&Capability::Keyvalue));
    }

    #[test]
    fn a_single_component_is_its_own_terminal() {
        let mut s = spec(Trigger::Http, Linking::None, 1);
        s.capabilities = vec![Capability::HttpEgress {
            host: "example.com".into(),
        }];
        assert_eq!(s.plan()[0].egress_host(), Some("example.com"));
    }

    #[test]
    fn a_messaging_trigger_may_not_also_fan_out_over_messaging() {
        assert!(
            spec(Trigger::Messaging, Linking::Messaging, 2)
                .validate()
                .is_err()
        );
    }

    #[test]
    fn a_lopsided_fan_out_gives_each_branch_its_own_depth() {
        let s = Spec {
            name: "demo".into(),
            trigger: Trigger::Http,
            edition: Edition::P2,
            branches: vec![2, 1, 3],
            over_messaging: false,
            capabilities: Vec::new(),
            placement: BTreeMap::new(),
        };
        assert!(s.validate().is_ok());
        assert!(s.is_irregular());
        assert_eq!(s.component_count(), 6);

        let nodes = s.plan();
        let ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "ingress",
                "branch1",
                "branch1step2",
                "branch2",
                "branch3",
                "branch3step2",
                "branch3step3",
            ]
        );

        assert_eq!(nodes[0].imports, ["branch1", "branch2", "branch3"]);
        assert_eq!(nodes[1].imports, ["branch1step2"]);
        assert!(nodes[2].imports.is_empty(), "branch1 ends after two");
        assert!(nodes[3].imports.is_empty(), "branch2 is one deep");
        assert_eq!(nodes[4].imports, ["branch3step2"]);
    }

    #[test]
    fn capabilities_land_at_the_end_of_every_branch() {
        let s = Spec {
            name: "demo".into(),
            trigger: Trigger::Http,
            edition: Edition::P2,
            branches: vec![2, 1],
            over_messaging: false,
            capabilities: vec![Capability::Keyvalue],
            placement: BTreeMap::new(),
        };
        let nodes = s.plan();
        assert!(
            !nodes[1].has(&Capability::Keyvalue),
            "mid-branch is not the end"
        );
        assert!(nodes[2].has(&Capability::Keyvalue), "branch1 ends here");
        assert!(nodes[3].has(&Capability::Keyvalue), "branch2 is one deep");
    }

    #[test]
    fn the_planned_ids_are_the_ids_placement_is_checked_against() {
        let mut s = spec(Trigger::Http, Linking::FanOut, 3);
        s.branches = vec![2, 1, 3];
        let planned: Vec<String> = s.plan().into_iter().map(|n| n.id).collect();
        let listed: Vec<String> = s.node_ids().into_iter().map(|(id, _)| id).collect();
        assert_eq!(planned, listed);

        // The terminal flag must agree with where `plan` puts capabilities.
        s.capabilities = vec![Capability::Keyvalue];
        let terminals: Vec<String> = s
            .node_ids()
            .into_iter()
            .filter(|(_, terminal)| *terminal)
            .map(|(id, _)| id)
            .collect();
        let with_capability: Vec<String> = s
            .plan()
            .into_iter()
            .filter(|n| !n.capabilities.is_empty())
            .map(|n| n.id)
            .collect();
        assert_eq!(terminals, with_capability);
    }

    #[test]
    fn placement_puts_a_capability_anywhere_including_mid_chain() {
        let mut s = spec(Trigger::Http, Linking::Chain, 3);
        s.capabilities = vec![Capability::Keyvalue, Capability::Logging];
        s.placement = placed(&[
            ("ingress", &["logging"]),
            ("step1", &[]),
            ("step2", &["keyvalue"]),
            ("step3", &[]),
        ]);
        assert!(s.validate().is_ok());

        let nodes = s.plan();
        assert_eq!(nodes[0].capabilities, [Capability::Logging]);
        assert!(nodes[1].capabilities.is_empty());
        assert_eq!(nodes[2].capabilities, [Capability::Keyvalue], "mid-chain");
        assert!(nodes[3].capabilities.is_empty(), "a terminal with none");
    }

    #[test]
    fn two_branches_can_be_given_different_capabilities() {
        let mut s = spec(Trigger::Http, Linking::FanOut, 2);
        s.capabilities = vec![Capability::Keyvalue, Capability::Blobstore];
        s.placement = placed(&[("branch1", &["keyvalue"]), ("branch2", &["blobstore"])]);

        let nodes = s.plan();
        assert_eq!(nodes[1].capabilities, [Capability::Keyvalue]);
        assert_eq!(nodes[2].capabilities, [Capability::Blobstore]);
        assert!(
            nodes[0].capabilities.is_empty(),
            "unnamed nodes get nothing"
        );
    }

    #[test]
    fn a_placed_component_takes_the_capabilities_in_palette_order() {
        let mut s = spec(Trigger::Http, Linking::None, 1);
        s.capabilities = vec![Capability::Keyvalue, Capability::Logging];
        s.placement = placed(&[("ingress", &["logging", "keyvalue"])]);
        assert_eq!(
            s.plan()[0].capabilities,
            [Capability::Keyvalue, Capability::Logging]
        );
    }

    #[test]
    fn an_empty_placement_is_exactly_the_old_behaviour() {
        let mut s = spec(Trigger::Http, Linking::Chain, 2);
        s.capabilities = vec![Capability::Keyvalue];
        let default_wiring: Vec<Vec<Capability>> =
            s.plan().into_iter().map(|n| n.capabilities).collect();

        s.placement = s.default_placement();
        let spelled_out: Vec<Vec<Capability>> =
            s.plan().into_iter().map(|n| n.capabilities).collect();
        assert_eq!(default_wiring, spelled_out);
    }

    #[test]
    fn the_placed_union_is_what_config_should_be_built_from() {
        let mut s = spec(Trigger::Http, Linking::FanOut, 2);
        s.capabilities = vec![Capability::Keyvalue, Capability::Postgres];
        // Postgres is selected but placed nowhere.
        s.placement = placed(&[("branch1", &["keyvalue"]), ("branch2", &["keyvalue"])]);
        assert_eq!(s.placed_capabilities(), [Capability::Keyvalue]);

        s.placement = BTreeMap::new();
        assert_eq!(s.placed_capabilities(), s.capabilities);
    }

    #[test]
    fn a_capability_no_component_imports_is_refused() {
        let mut s = spec(Trigger::Http, Linking::Chain, 2);
        s.capabilities = vec![Capability::Keyvalue];
        s.placement = placed(&[("ingress", &[]), ("step1", &[]), ("step2", &[])]);

        let err = s
            .validate()
            .expect_err("keyvalue reaches nothing")
            .to_string();
        assert!(err.contains("keyvalue"), "{err}");
        assert!(
            err.contains("place it on a component"),
            "say how to fix it: {err}"
        );
    }

    #[test]
    fn otel_needs_no_component_because_nothing_can_import_it() {
        let mut s = spec(Trigger::Http, Linking::Chain, 2);
        s.capabilities = vec![Capability::Otel, Capability::Keyvalue];
        s.placement = placed(&[("step2", &["keyvalue"])]);

        assert!(s.validate().is_ok(), "{:?}", s.validate());
        assert!(
            s.placed_capabilities().contains(&Capability::Otel),
            "and it still reaches dev.wasi_otel"
        );
    }

    #[test]
    fn a_placement_naming_something_that_is_not_a_component_is_refused() {
        let mut s = spec(Trigger::Http, Linking::Chain, 2);
        s.capabilities = vec![Capability::Keyvalue];
        s.placement = placed(&[("step9", &["keyvalue"])]);

        let err = s.validate().expect_err("step9 does not exist").to_string();
        assert!(err.contains("step9"), "{err}");
        assert!(err.contains("ingress, step1, step2"), "{err}");
    }

    #[test]
    fn placing_a_capability_that_was_never_selected_is_refused() {
        let mut s = spec(Trigger::Http, Linking::Chain, 2);
        s.capabilities = vec![Capability::Keyvalue];
        s.placement = placed(&[("step1", &["postgres"])]);

        let err = s
            .validate()
            .expect_err("postgres is not selected")
            .to_string();
        assert!(err.contains("postgres"), "{err}");
        assert!(err.contains("keyvalue"), "should list the palette: {err}");
    }

    #[test]
    fn linking_is_derived_from_the_branches_not_stored() {
        let mut s = spec(Trigger::Http, Linking::None, 1);
        assert_eq!(s.linking(), Linking::None);
        s.branches = vec![3];
        assert_eq!(s.linking(), Linking::Chain);
        s.branches = vec![1, 1];
        assert_eq!(s.linking(), Linking::FanOut);
        s.over_messaging = true;
        assert_eq!(s.linking(), Linking::Messaging);
    }

    #[test]
    fn shape_reflects_both_axes() {
        assert_eq!(spec(Trigger::Http, Linking::None, 1).shape(), Shape::Single);
        assert_eq!(
            spec(Trigger::Service, Linking::None, 1).shape(),
            Shape::Service
        );
        assert_eq!(spec(Trigger::Http, Linking::Chain, 1).shape(), Shape::Chain);
        assert_eq!(
            spec(Trigger::Messaging, Linking::FanOut, 2).shape(),
            Shape::FanOut
        );
    }

    #[test]
    fn names_that_are_not_wit_identifiers_are_rejected() {
        for bad in ["My-App", "1st-app", "my_app", "app-1", ""] {
            let mut s = spec(Trigger::Http, Linking::None, 1);
            s.name = bad.into();
            assert!(s.validate().is_err(), "{bad:?} should be rejected");
        }
        for good in ["demo", "my-app", "app2", "a-b-c"] {
            let mut s = spec(Trigger::Http, Linking::None, 1);
            s.name = good.into();
            assert!(s.validate().is_ok(), "{good:?} should be accepted");
        }
    }

    #[test]
    fn grpc_is_allowed_on_every_trigger() {
        for trigger in [Trigger::Http, Trigger::Messaging] {
            let mut s = spec(trigger, Linking::None, 1);
            s.capabilities = vec![Capability::Grpc {
                host: "grpc.example.com".into(),
            }];
            assert!(
                s.validate().is_ok(),
                "{} should allow grpc",
                trigger.label()
            );
            assert!(
                s.plan()[0]
                    .capabilities
                    .iter()
                    .any(|c| c.is_async(Edition::P2))
            );
        }
        // Services are p3 and the gRPC scaffold is p2-only.
        let mut s = spec(Trigger::Service, Linking::None, 1);
        s.edition = Edition::P3;
        s.capabilities = vec![Capability::Grpc {
            host: "grpc.example.com".into(),
        }];
        assert!(
            s.validate().is_err(),
            "grpc on a (p3) service must be refused"
        );
    }

    #[test]
    fn otel_is_the_one_capability_that_emits_no_code() {
        assert!(!Capability::Otel.emits_code());
        for capability in [
            Capability::Keyvalue,
            Capability::Blobstore,
            Capability::Config,
            Capability::Logging,
            Capability::Postgres,
        ] {
            assert!(capability.emits_code(), "{} must emit", capability.label());
        }
    }
}
