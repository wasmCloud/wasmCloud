//! CLI command for browsing what wasmCloud already ships — templates,
//! examples, and community projects — and cloning the one you pick, or
//! generating the boilerplate for a new multi-component workload.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use clap::{Args, Subcommand};
use serde_json::json;
use tracing::{info, instrument};
use wash_topology::Topology;

use crate::{
    cli::{CliCommand, CliContext, CommandOutput, new::NewCommand},
    wizard::{
        builder, generate,
        index::{self, EXPERIMENTAL_REPO as TEMPLATE_COMMUNITY, Index, Source},
        picker::{self, Choice},
        plugin, reverse,
        spec::{Capability, CapabilityKind, Edition, Linking, Spec, Trigger},
    },
};

/// Show what a tree of projects would appear as in the catalog, or record a
/// project hosted in another repository via `--link`.
#[derive(Args, Debug, Clone)]
pub struct IndexArgs {
    /// Directory to scan; any directory holding a `.wash/config.*` is a project
    #[arg(long, value_name = "DIR", default_value = ".")]
    path: PathBuf,

    /// Record a project hosted in another repository by writing an origin stub for it
    #[arg(long, value_name = "URL", conflicts_with = "path")]
    link: Option<String>,

    /// Path within --link's repository, when the project is not the whole of it
    #[arg(long, value_name = "PATH", requires = "link")]
    subfolder: Option<String>,

    /// Directory the `--link` stub is written under, e.g. `workload-examples`.
    #[arg(long, value_name = "DIR", requires = "link", default_value = ".")]
    into: PathBuf,

    /// Write the tree's catalog.json — the published document the wizard
    /// fetches instead of cloning. Run it in CI so the catalog tracks main.
    #[arg(long, conflicts_with = "link")]
    write_catalog: bool,
}

#[derive(Subcommand, Debug, Clone)]
pub enum WizardSubcommand {
    /// Preview a tree of projects as catalog entries, or link one hosted elsewhere
    Index(IndexArgs),
}

/// Browse wasmCloud templates, examples and community projects — or generate a new workload
#[derive(Args, Debug, Clone)]
pub struct WizardCommand {
    #[command(subcommand)]
    command: Option<WizardSubcommand>,

    /// Scaffold this architecture by id instead of opening the picker
    #[arg(long)]
    architecture: Option<String>,

    /// Project name and local directory to create (defaults to the architecture's name)
    #[arg(long)]
    name: Option<String>,

    /// Print the `wash new` invocation instead of running it, for scripting
    #[arg(long, conflicts_with_all = ["list", "trigger", "recipe", "from"])]
    print: bool,

    /// List the available architectures and exit, touching nothing
    #[arg(long, conflicts_with = "architecture")]
    list: bool,

    /// Re-clone the templates repository instead of using the cached copy
    #[arg(long)]
    refresh: bool,

    /// Also offer community templates from wasmCloud/awesome-wasmcloud
    #[arg(long)]
    experimental: bool,

    /// Read architectures from this directory rather than cloning
    #[arg(long, value_name = "DIR")]
    local: Option<PathBuf>,

    /// What starts the workload; generates instead of picking a template
    #[arg(long, value_enum, conflicts_with_all = ["architecture", "list"])]
    trigger: Option<Trigger>,

    /// How the triggered component reaches the rest
    #[arg(long, value_enum, default_value = "none", requires = "trigger")]
    linking: Linking,

    /// Which preview of the component model to generate against
    #[arg(long, value_enum, default_value = "p2", requires = "trigger")]
    edition: Edition,

    /// Components past the trigger: hops for a chain, branches for a fan-out
    #[arg(long, default_value_t = 2, requires = "trigger")]
    count: usize,

    /// One branch of the given depth; repeatable, and overrides --linking/--count
    #[arg(long, value_name = "DEPTH", requires = "trigger")]
    branch: Vec<usize>,

    /// Give the generated components outbound HTTP to this host
    #[arg(long, value_name = "HOST", requires = "trigger")]
    egress: Option<String>,

    /// Scaffold a gRPC client against this host (proto, build script, tonic deps)
    #[arg(long, value_name = "HOST", requires = "trigger")]
    grpc: Option<String>,

    /// Wire a host capability into the generated components. Repeatable.
    #[arg(long, value_enum, requires = "trigger")]
    capability: Vec<CapabilityKind>,

    /// Place capabilities on one component, e.g. --place branch1=keyvalue,logging; repeatable, unnamed components get none
    #[arg(long, value_name = "NODE=CAP,...", requires = "trigger")]
    place: Vec<String>,

    /// Generate a host component plugin (always wasip3) providing this WIT interface, plus a consumer wired to it
    #[arg(long, value_name = "NS:PKG/IFACE", conflicts_with_all = ["architecture", "list", "trigger", "recipe", "from", "print"])]
    provide: Option<String>,

    /// Wire host affordances into the generated plugin: identity and/or lifecycle
    #[arg(
        long = "with",
        value_name = "identity|lifecycle",
        value_delimiter = ',',
        requires = "provide"
    )]
    with: Vec<String>,

    /// Replay a saved recipe file, prompting for nothing
    #[arg(long, value_name = "FILE", conflicts_with_all = ["architecture", "list", "trigger"])]
    recipe: Option<PathBuf>,

    /// Preview the workload shape recorded on a pushed OCI artifact, without pulling it
    #[arg(long, value_name = "REF", conflicts_with_all = ["architecture", "list", "trigger", "recipe", "from", "provide", "print"])]
    from_oci: Option<String>,

    /// Print an existing project's shape as wizard answers a --recipe can replay; writes nothing
    #[arg(long, value_name = "DIR", conflicts_with_all = ["architecture", "list", "trigger", "recipe"])]
    from: Option<PathBuf>,
}

impl CliCommand for WizardCommand {
    #[instrument(level = "debug", skip(self, ctx), name = "wizard")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        if let Some(WizardSubcommand::Index(args)) = &self.command {
            return run_index(args).await;
        }

        // Generator entry points skip the index: nothing is chosen from it.
        if let Some(reference) = &self.from_oci {
            return preview_oci(reference, ctx).await;
        }
        if let Some(target) = &self.provide {
            let spec = self.plugin_spec(target)?;
            return run_plugin_generator(&spec, ctx.project_dir()).await;
        }
        if let Some(path) = &self.recipe {
            let raw = tokio::fs::read_to_string(path)
                .await
                .with_context(|| format!("failed to read recipe {}", path.display()))?;
            let mut spec: Spec = serde_yaml_ng::from_str(&raw)
                .with_context(|| format!("failed to parse recipe {}", path.display()))?;
            if let Some(name) = &self.name {
                spec.name = name.clone();
            }
            return run_generator(spec, ctx.project_dir()).await;
        }
        if let Some(trigger) = self.trigger {
            return run_generator(self.spec_from_flags(trigger)?, ctx.project_dir()).await;
        }
        if let Some(project) = &self.from {
            return recover_recipe(project).await;
        }

        // The loop re-enters when the picker's community toggle or refresh is used;
        // the flags only seed the starting values.
        let mut experimental = self.experimental;
        let mut refresh = self.refresh;
        loop {
            let index = index::load(
                &ctx.cache_dir(),
                ctx.project_dir(),
                self.local.as_deref(),
                refresh,
                experimental,
            )
            .await
            .context("failed to load the architecture index")?;
            refresh = false;

            // Only the paths that need a template complain about an empty index.
            if index.topologies.is_empty() && (self.list || self.architecture.is_some()) {
                bail!(
                    "no architectures found under {}.\n\
                     Entries are derived from the projects themselves — any directory \
                     with a `.wash/config.yaml` (or .yml/.json/.toml) — so an empty list \
                     means no projects were found there. Try --refresh, or --local <dir> \
                     to read a checkout directly.",
                    index.root.display()
                );
            }
            info!(
                count = index.topologies.len(),
                source = ?index.source,
                "loaded architectures"
            );

            if self.list {
                return Ok(list_output(&index.topologies, index.source));
            }

            if let Some(id) = &self.architecture {
                let selected =
                    index
                        .topologies
                        .iter()
                        .find(|t| &t.id == id)
                        .with_context(|| {
                            format!(
                                "no architecture named '{id}'; run `wash wizard --list` to see the \
                             {} available",
                                index.topologies.len()
                            )
                        })?;
                return scaffold(selected, &index, self.name.as_deref(), self.print, ctx).await;
            }

            // A picker has no defensible default answer, so refuse rather than choose one.
            if ctx.is_non_interactive() {
                bail!(
                    "wash wizard needs an interactive terminal; pass --architecture <id> \
                     to scaffold without prompting, or --list to see what is available"
                );
            }

            // `Choice::Template` borrows `index`, which lives one iteration.
            match picker::choose(&index).context("failed to run the picker")? {
                Some(Choice::Template(topology)) => {
                    return scaffold(topology, &index, self.name.as_deref(), self.print, ctx).await;
                }
                Some(Choice::Custom) => {
                    let name = self.name.as_deref().unwrap_or("my-workload");
                    return match builder::build(name, None)
                        .context("failed to design the workload")?
                    {
                        Some(spec) => run_generator(spec, ctx.project_dir()).await,
                        None => Ok(CommandOutput::ok("nothing generated", None)),
                    };
                }
                Some(Choice::CustomCapability) => {
                    // The picker has left the alternate screen, so a line prompt lands cleanly.
                    let target = match &self.provide {
                        Some(target) => target.clone(),
                        None => dialoguer::Input::new()
                            .with_prompt("interface to provide (ns:pkg/iface)")
                            .default("acme:cache/store".to_string())
                            .interact_text()
                            .context("failed to read the interface")?,
                    };
                    let spec = self.plugin_spec(&target)?;
                    return run_plugin_generator(&spec, ctx.project_dir()).await;
                }
                Some(Choice::ToggleExperimental) => {
                    experimental = !experimental;
                    // A first fetch takes seconds; without this the terminal looks hung.
                    if experimental {
                        eprintln!("fetching community templates from {TEMPLATE_COMMUNITY}…");
                    }
                }
                Some(Choice::Refresh) => {
                    refresh = true;
                    eprintln!("fetching templates…");
                }
                None => return Ok(CommandOutput::ok("no architecture selected", None)),
            }
        }
    }
}

impl WizardCommand {
    /// The plugin to generate: the interface plus whichever `--with` affordances.
    fn plugin_spec(&self, target: &str) -> anyhow::Result<plugin::PluginSpec> {
        let mut spec = plugin::PluginSpec::parse(target, self.name.as_deref())?;
        for extra in &self.with {
            match extra.as_str() {
                "identity" => spec.with_identity = true,
                "lifecycle" => spec.with_lifecycle = true,
                other => bail!("unknown --with value '{other}'; use identity or lifecycle"),
            }
        }
        Ok(spec)
    }

    fn spec_from_flags(&self, trigger: Trigger) -> anyhow::Result<Spec> {
        let (expanded, over_messaging) = self.linking.expand(self.count);
        let spec = Spec {
            name: self.name.clone().unwrap_or_else(|| default_name(trigger)),
            trigger,
            edition: self.edition,
            branches: if self.branch.is_empty() {
                expanded
            } else {
                self.branch.clone()
            },
            over_messaging: self.branch.is_empty() && over_messaging,
            capabilities: {
                // Deduplicate: a repeated capability would emit its helper twice.
                let mut kinds: Vec<CapabilityKind> = Vec::new();
                for kind in &self.capability {
                    if !kinds.contains(kind) {
                        kinds.push(*kind);
                    }
                }
                kinds.into_iter().map(Capability::from)
            }
            .chain(
                self.egress
                    .as_ref()
                    .map(|host| Capability::HttpEgress { host: host.clone() }),
            )
            .chain(
                self.grpc
                    .as_ref()
                    .map(|host| Capability::Grpc { host: host.clone() }),
            )
            .collect(),
            placement: self.placement()?,
        };
        spec.validate()?;
        Ok(spec)
    }

    /// Parse `--place NODE=CAP,...`; name validation is [`Spec::validate`]'s business.
    fn placement(&self) -> anyhow::Result<std::collections::BTreeMap<String, Vec<String>>> {
        self.place
            .iter()
            .map(|entry| {
                let (node, capabilities) = entry.split_once('=').with_context(|| {
                    format!(
                        "--place takes NODE=CAP,... (for example branch1=keyvalue); got '{entry}'"
                    )
                })?;
                let labels = capabilities
                    .split(',')
                    .map(str::trim)
                    .filter(|label| !label.is_empty())
                    .map(str::to_string)
                    .collect();
                Ok((node.trim().to_string(), labels))
            })
            .collect()
    }
}

fn default_name(trigger: Trigger) -> String {
    match trigger {
        Trigger::Http => "my-api",
        Trigger::Messaging => "my-worker",
        Trigger::Service => "my-service",
    }
    .to_string()
}

/// Preview a tree of projects as catalog entries, or write a linked entry's origin stub.
async fn run_index(args: &IndexArgs) -> anyhow::Result<CommandOutput> {
    if let Some(repo) = &args.link {
        let stub = index::link(repo, args.subfolder.as_deref(), &args.into).await?;
        let path = index::write_stub(&stub).await?;
        return Ok(CommandOutput::ok(
            format!("linked {} at {}", stub.topology.id, path.display()),
            Some(json!({
                "id": stub.topology.id,
                "repo": stub.topology.repo,
                "subfolder": stub.topology.subfolder,
                "stub": path,
            })),
        ));
    }

    // The same derivation the catalog runs, so this previews what the picker shows.
    let entries = index::derive_entries(&args.path).await?;
    if entries.is_empty() {
        bail!(
            "no wash projects under {} — a project is any directory with a \
             `.wash/config.yaml` (or .yml/.json/.toml)",
            args.path.display()
        );
    }

    if args.write_catalog {
        // Sorted by id so the bytes are deterministic across regenerations.
        let mut entries = entries;
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        let path = args.path.join(wash_topology::catalog::CATALOG_NAME);
        tokio::fs::write(&path, wash_topology::catalog::to_catalog_json(&entries)?)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
        return Ok(CommandOutput::ok(
            format!("wrote {} entries to {}", entries.len(), path.display()),
            Some(json!({ "count": entries.len(), "catalog": path })),
        ));
    }
    let summary = entries
        .iter()
        .map(|t| {
            format!(
                "{:<44} {:?}  {} component(s)\n",
                t.id,
                t.shape,
                t.nodes.len()
            )
        })
        .collect::<String>()
        .trim_end()
        .to_string();

    Ok(CommandOutput::ok(
        summary,
        Some(json!({
            "path": args.path,
            "count": entries.len(),
            "projects": entries.iter().map(|t| json!({
                "id": t.id,
                "source": t.source,
                "shape": t.shape,
                "components": t.nodes.len(),
            })).collect::<Vec<_>>(),
        })),
    ))
}

/// Recover a recipe from an existing project and print it; nothing is written.
async fn recover_recipe(project: &Path) -> anyhow::Result<CommandOutput> {
    let spec = reverse::spec_from_project(project)
        .await
        .with_context(|| format!("cannot recover a recipe from {}", project.display()))?;
    let yaml = serde_yaml_ng::to_string(&spec).context("failed to serialize the recipe")?;

    // Capabilities not all at the ends need `--place` to reproduce.
    let places: String = spec
        .placement
        .iter()
        .map(|(node, labels)| format!(" \\\n    --place {node}={}", labels.join(",")))
        .collect();

    // `--linking X --count N` cannot describe branches of differing depth.
    let shape = if spec.is_irregular() {
        spec.branches
            .iter()
            .map(|depth| format!(" --branch {depth}"))
            .collect()
    } else if spec.branches.is_empty() {
        String::new()
    } else {
        // `--count` means hops for a chain and branches for a fan-out.
        let count = match spec.linking() {
            Linking::Chain => spec.branches.first().copied().unwrap_or(1),
            _ => spec.branches.len(),
        };
        format!(" --linking {} --count {count}", spec.linking().label())
    };

    let edition = if spec.edition.is_p3() {
        " --edition p3"
    } else {
        ""
    };
    let mut extras = String::new();
    for capability in &spec.capabilities {
        match capability {
            Capability::HttpEgress { host } => extras.push_str(&format!(" --egress {host}")),
            Capability::Grpc { host } => extras.push_str(&format!(" --grpc {host}")),
            other => extras.push_str(&format!(" --capability {}", other.label())),
        }
    }

    let message = format!(
        "{name}: {trigger} trigger, {linking} linking\n\n{yaml}\n\
         regenerate with:\n  \
         wash wizard --recipe <file>\n  \
         wash wizard --trigger {trigger}{edition}{shape}{extras}{places}",
        name = spec.name,
        trigger = spec.trigger.label(),
        linking = spec.linking().label(),
    );

    Ok(CommandOutput::ok(
        message,
        Some(json!({
            "name": spec.name,
            "trigger": spec.trigger,
            "linking": spec.linking(),
            "branches": spec.branches,
            "capabilities": spec.capabilities,
            "placement": spec.placement,
            "recipe": yaml,
        })),
    ))
}

/// Render the shape a pushed artifact carries, from its manifest alone.
async fn preview_oci(reference: &str, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
    // Same reader, key, and loopback default as `wash oci inspect`.
    let registry = crate::cli::oci::RegistryArgs::default();
    let oci_config = registry.oci_config_for(ctx, Some(reference))?;
    let (topology, _digest) =
        crate::cli::oci::read_topology_annotation(reference, oci_config).await?;
    let Some(topology) = topology else {
        bail!(
            "{reference} carries no workload shape.\n\
             A `wash oci push` from inside a project records one, derived from the \
             project as it is pushed; `wash oci inspect` shows the digest-only case too"
        );
    };

    let mut message = String::new();
    for line in picker::detail_lines(&topology) {
        message.push_str(&line);
        message.push('\n');
    }
    Ok(CommandOutput::ok(
        message,
        Some(json!({ "reference": reference, "topology": topology })),
    ))
}

/// Generate a capability rather than a workload.
async fn run_plugin_generator(
    spec: &plugin::PluginSpec,
    parent: &Path,
) -> anyhow::Result<CommandOutput> {
    let generated = plugin::generate(spec, parent)
        .await
        .context("failed to generate the plugin")?;

    let name = &spec.name;
    // Only mutated when the feature is off, below.
    #[cfg_attr(feature = "host-component-plugins", allow(unused_mut))]
    let mut message = format!(
        "generated a host component plugin at {root}\n\
         \n\
         \x20 plugin/     exports the capability\n\
         \x20 consumer/   imports it, serves HTTP\n\
         \n\
         next:\n\
         \x20 wash -C {name} build\n\
         \x20 wash -C {name} dev\n\
         \x20 curl localhost:8000/\n\
         \n\
         Both bodies carry a TODO; the wiring between them is done.",
        root = generated.root.display(),
    );

    // Say when this build cannot load what it wrote, rather than let `wash dev`
    // silently ignore `dev.host_plugins`.
    #[cfg(not(feature = "host-component-plugins"))]
    message.push_str(
        "\n\nnote: this wash was built without `host-component-plugins`, so \
         `wash dev` will not load the plugin. See the generated README.",
    );

    Ok(CommandOutput::ok(
        message,
        Some(json!({
            "name": spec.name,
            "interface": format!("{}:{}/{}", spec.namespace, spec.package, spec.interface),
            "root": generated.root,
        })),
    ))
}

async fn run_generator(spec: Spec, parent: &Path) -> anyhow::Result<CommandOutput> {
    let generated = generate::generate(&spec, parent)
        .await
        .context("failed to generate the project")?;

    let mut message = String::new();
    for line in wash_topology::diagram(&generated.topology) {
        message.push_str(&line);
        message.push('\n');
    }
    let name = &spec.name;
    message.push_str(&format!(
        "\ngenerated {count} component(s) at {root}\n\
         \n\
         next:\n\
         \x20 wash -C {name} build\n\
         \x20 wash -C {name} dev\n\
         \x20 curl localhost:8000/\n\
         \n\
         The wiring is done; each component body carries a TODO.\n\
         Recover this layout any time with: wash wizard --from {name}",
        count = generated.components.len(),
        root = generated.root.display(),
    ));

    Ok(CommandOutput::ok(
        message,
        Some(json!({
            "name": spec.name,
            "trigger": spec.trigger,
            "linking": spec.linking(),
            "branches": spec.branches,
            "shape": spec.shape(),
            "components": generated.components,
            "output_dir": generated.root,
        })),
    ))
}

/// Where a catalog entry is actually cloned from: a linked entry names its own
/// origin, and that always wins.
fn origin<'a>(topology: &'a Topology, index: &Index) -> (&'a str, Option<&'a str>) {
    match topology.repo.as_deref() {
        Some(repo) => (repo, topology.subfolder.as_deref()),
        None => (index.repo_for(topology), Some(topology.source.as_str())),
    }
}

/// The `wash new` invocation that scaffolds a chosen architecture.
fn scaffold_command(repo: &str, subfolder: Option<&str>, name: &str) -> String {
    let mut command = format!("wash new {repo} --name {name}");
    if let Some(subfolder) = subfolder {
        command.push_str(&format!(" --subfolder {subfolder}"));
    }
    command
}

/// Clone the project behind a catalog entry via `wash new`; `--print` prints
/// the command instead of running it.
async fn scaffold(
    topology: &Topology,
    index: &Index,
    name: Option<&str>,
    print: bool,
    ctx: &CliContext,
) -> anyhow::Result<CommandOutput> {
    let (repo, subfolder) = origin(topology, index);
    let name = name.unwrap_or(&topology.id);
    let command = scaffold_command(repo, subfolder, name);
    let facts = json!({
        "architecture": topology.id,
        "shape": topology.shape,
        "source": topology.source,
        "repo": repo,
        "capabilities": topology.capabilities,
        "command": command,
    });

    let mut message = String::new();
    for line in picker::detail_lines(topology) {
        message.push_str(&line);
        message.push('\n');
    }

    if print {
        message.push_str("\nscaffold with:\n  ");
        message.push_str(&command);
        return Ok(CommandOutput::ok(message, Some(facts)));
    }

    let created = NewCommand::from_template(
        repo.to_string(),
        Some(name.to_string()),
        subfolder.map(str::to_string),
    )
    .handle(ctx)
    .await
    // `wash new`'s already-exists error names only the path.
    .with_context(|| {
        format!(
            "failed to scaffold '{}' — pass --name to choose a different directory",
            topology.id
        )
    })?;

    message.push('\n');
    message.push_str(&created.text());
    Ok(CommandOutput::ok(message, Some(facts)))
}

fn list_output(topologies: &[Topology], source: Source) -> CommandOutput {
    let mut message = String::new();
    let mut current = None;
    for topology in topologies {
        if current != Some(topology.shape) {
            message.push_str(&format!("\n{}\n", topology.shape.label()));
            current = Some(topology.shape);
        }
        message.push_str(&format!(
            "  {:<40} {}\n",
            topology.id,
            topology.title.as_deref().unwrap_or("")
        ));
    }
    CommandOutput::ok(
        message.trim_end().to_string(),
        Some(json!({
            "count": topologies.len(),
            "source": format!("{source:?}").to_lowercase(),
            "architectures": topologies.iter().map(|t| json!({
                "id": t.id,
                "shape": t.shape,
                "source": t.source,
                "capabilities": t.capabilities,
            })).collect::<Vec<_>>(),
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wizard::index::{Index, TEMPLATE_REPO};
    use wash_topology::{FactsSource, Node, Role, Shape};

    fn topology(id: &str) -> Topology {
        Topology {
            schema: 1,
            id: id.into(),
            source: format!("templates/{id}"),
            repo: None,
            subfolder: None,
            title: Some("Demo".into()),
            shape: Shape::Chain,
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

    /// An index holding one entry, from the curated source.
    fn hosted_index(topology: Topology) -> Index {
        Index {
            topologies: vec![topology],
            source: Source::Cache,
            root: PathBuf::new(),
            origins: vec![Source::Cache],
            experimental: false,
            community_age: None,
        }
    }

    #[test]
    fn scaffold_command_uses_the_manifest_source_as_the_subfolder() {
        let index = hosted_index(topology("http-handler"));
        let (repo, subfolder) = origin(&index.topologies[0], &index);
        let command = scaffold_command(repo, subfolder, "http-handler");
        assert!(
            command.contains("--subfolder templates/http-handler"),
            "{command}"
        );
        assert!(command.contains("--name http-handler"), "{command}");
    }

    #[test]
    fn an_explicit_name_overrides_the_architecture_id() {
        let index = hosted_index(topology("http-handler"));
        let (repo, subfolder) = origin(&index.topologies[0], &index);
        let command = scaffold_command(repo, subfolder, "my-app");
        assert!(command.contains("--name my-app"), "{command}");
        // The subfolder still points at the template it came from.
        assert!(
            command.contains("--subfolder templates/http-handler"),
            "{command}"
        );
    }

    #[test]
    fn a_linked_entry_is_cloned_from_the_repository_it_names() {
        // The stub's `source` path exists only in the catalog, not in its repo.
        let mut stub = topology("wasi-ai-app");
        stub.source = "workload-examples/wasi-ai-app".into();
        stub.repo = Some("https://github.com/bharattech/wasi-ai-app".into());
        let index = hosted_index(stub);

        let (repo, subfolder) = origin(&index.topologies[0], &index);
        assert_eq!(repo, "https://github.com/bharattech/wasi-ai-app");
        assert_eq!(
            subfolder, None,
            "a linked project is a repository of its own"
        );

        let command = scaffold_command(repo, subfolder, "wasi-ai-app");
        assert!(!command.contains("--subfolder"), "{command}");
        assert!(!command.contains("wasmCloud/wasmCloud"), "{command}");
    }

    #[test]
    fn a_linked_entry_may_still_name_a_subfolder() {
        let mut stub = topology("thing");
        stub.repo = Some("https://github.com/someone/monorepo".into());
        stub.subfolder = Some("components/thing".into());
        let index = hosted_index(stub);

        let (repo, subfolder) = origin(&index.topologies[0], &index);
        assert_eq!(repo, "https://github.com/someone/monorepo");
        assert_eq!(subfolder, Some("components/thing"));
    }

    #[test]
    fn a_community_entry_is_cloned_from_the_community_repository() {
        // Both entries share an id — unique only within a source — so lookup must not match by id.
        let index = Index {
            topologies: vec![topology("http-handler"), topology("http-handler")],
            source: Source::Cache,
            root: PathBuf::new(),
            origins: vec![Source::Cache, Source::Experimental],
            experimental: true,
            community_age: None,
        };

        assert_eq!(index.repo_for(&index.topologies[0]), TEMPLATE_REPO);
        assert_eq!(index.repo_for(&index.topologies[1]), TEMPLATE_COMMUNITY);
    }
}
