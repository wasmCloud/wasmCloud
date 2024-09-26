use std::collections::{BTreeMap, HashMap};
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, ensure, Context as _, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use console::style;
use notify::event::ModifyKind;
use notify::{event::EventKind, Event as NotifyEvent, RecursiveMode, Watcher};
use rand::{distributions::Alphanumeric, Rng};
use semver::Version;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncBufReadExt as _;
use tokio::process::Child;
use tokio::{select, sync::mpsc};
use wash_lib::app::AppManifest;
use wash_lib::cli::stop::stop_provider;
use wash_lib::common::CommandGroupUsage;
use wash_lib::component::{scale_component, ScaleComponentArgs};
use wasmcloud_control_interface::Client as CtlClient;
use wit_parser::{Resolve, WorldId};

use wadm_types::{
    CapabilityProperties, Component, ComponentProperties, ConfigProperty, LinkProperty, Manifest,
    Metadata, Policy, Properties, SecretProperty, Specification, SpreadScalerProperty,
    TargetConfig, TraitProperty,
};
use wash_lib::build::{build_project, SignConfig};
use wash_lib::cli::CommandOutput;
use wash_lib::config::{cfg_dir, downloads_dir};
use wash_lib::generate::emoji;
use wash_lib::id::ServerId;
use wash_lib::parser::{get_config, ProjectConfig};
use wash_lib::start::{
    ensure_nats_server, ensure_wadm, ensure_wasmcloud, start_wadm, start_wasmcloud_host,
    NatsConfig, WadmConfig, NATS_SERVER_BINARY,
};
use wasmcloud_core::{
    parse_wit_meta_from_operation, LinkName, WitInterface, WitNamespace, WitPackage,
};

use crate::app::deploy_model_from_manifest;
use crate::cmd::up::{
    nats_client_from_wasmcloud_opts, remove_wadm_pidfile, start_nats, NatsOpts, WadmOpts,
    WasmcloudOpts,
};
use crate::config::{configure_host_env, DEFAULT_NATS_HOST};
use crate::down::stop_nats;

const DEFAULT_KEYVALUE_PROVIDER_IMAGE: &str = "ghcr.io/wasmcloud/keyvalue-nats:0.3.0";
const DEFAULT_HTTP_CLIENT_PROVIDER_IMAGE: &str = "ghcr.io/wasmcloud/http-client:0.12.1";
const DEFAULT_HTTP_SERVER_PROVIDER_IMAGE: &str = "ghcr.io/wasmcloud/http-server:0.23.1";
const DEFAULT_BLOBSTORE_FS_PROVIDER_IMAGE: &str = "ghcr.io/wasmcloud/blobstore-fs:0.8.0";
const DEFAULT_MESSAGING_NATS_PROVIDER_IMAGE: &str = "ghcr.io/wasmcloud/messaging-nats:0.22.0";

const DEFAULT_INCOMING_HANDLER_ADDRESS: &str = "127.0.0.1:8000";
const DEFAULT_MESSAGING_HANDLER_SUBSCRIPTION: &str = "wasmcloud.dev";
const DEFAULT_BLOBSTORE_ROOT_DIR: &str = "/tmp";
const DEFAULT_KEYVALUE_BUCKET: &str = "wasmcloud";

const WASH_DEV_DIR: &str = "dev";
const WASH_SESSIONS_FILE_NAME: &str = "wash-dev-sessions.json";

const SESSIONS_FILE_VERSION: Version = Version::new(0, 1, 0);
const SESSION_ID_LEN: usize = 6;

const DEFAULT_PROVIDER_STOP_TIMEOUT_MS: u64 = 3000;

#[derive(Debug, Clone, Parser)]
pub struct DevCommand {
    #[clap(flatten)]
    pub nats_opts: NatsOpts,

    #[clap(flatten)]
    pub wasmcloud_opts: WasmcloudOpts,

    #[clap(flatten)]
    pub wadm_opts: WadmOpts,

    /// ID of the host to use for `wash dev`
    /// if one is not selected, `wash dev` will attempt to use the single host in the lattice
    #[clap(long = "host-id", name = "host-id", value_parser)]
    pub host_id: Option<ServerId>,

    /// Path to code directory
    #[clap(name = "code-dir", long = "work-dir", env = "WASH_DEV_CODE_DIR")]
    pub code_dir: Option<PathBuf>,

    /// Whether to leave the host running after dev
    #[clap(
        name = "leave-host-running",
        long = "leave-host-running",
        env = "WASH_DEV_LEAVE_HOST_RUNNING",
        default_value = "false",
        help = "Leave the wasmCloud host running after stopping the devloop"
    )]
    pub leave_host_running: bool,

    /// Write generated WADM manifest(s) to a given folder (every time they are generated)
    #[clap(long = "manifest-output-dir", env = "WASH_DEV_MANIFEST_OUTPUT_DIR")]
    pub manifest_output_dir: Option<PathBuf>,
}

/// Keys that index the list of dependencies in a [`ProjectDeps`]
///
/// # Examples
///
/// ```
/// let project_key = ProjectDependencyKey::Project {
///   name: "http-hello-world".into(),
///   imports: vec![ ("wasi".into(), "http".into(), "incoming-handler".into(), None) ],
///   exports: vec![ ("wasi".into(), "http".into(), "incoming-handler".into(), None) ],
///   in_workspace: None,
/// };
///
/// let workspace_key = ProjectDependencyKey::Workspace; // alternatively ProjectDependencyKey::default()
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ProjectDependencyKey {
    #[allow(unused)]
    RootWorkspace { name: String, path: PathBuf },
    /// Identifies a nested workspace inside the root workspace
    ///
    /// Workspaces are hierarchical: they may contain one or more projects *or* other workspaces,
    /// with the path structure of workspaces being the arbiter of which workspaces are above others.
    ///
    /// Only one workspace can be the *top-most* workspace, in that it contains
    /// all other workspaces and projects.
    #[allow(unused)]
    Workspace {
        name: String,
        path: PathBuf,
        root: bool,
    },
    /// Identifies a project inside the root workspace
    Project { name: String, path: PathBuf },
}

impl ProjectDependencyKey {
    /// Create a [`ProjectDependencyKey`] from the name of a project
    ///
    /// The supplied `project_dir` must be a folder containing a `wasmcloud.toml`
    fn from_project(name: &str, project_dir: impl AsRef<Path>) -> Result<Self> {
        Ok(Self::Project {
            name: name.into(),
            path: project_dir.as_ref().into(),
        })
    }
}

/// Specification for a single dependency in a given project
///
/// [`DependencySpec`]s are normally gleaned from some source of project metadata, for example:
///
/// - dependency overrides in a project-level `wasmcloud.toml`
/// - dependency overrides in a workspace-level `wasmcloud.toml`
/// - WIT interface of a project
///
/// A `DependencySpec` represents a single dependency in the project, categorized into what part it is expected
/// to play in in fulfilling WIT interfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DependencySpec {
    /// A dependency that receives invocations (ex. `keyvalue-nats` receiving a `wasi:keyvalue/get`)
    Exports(DependencySpecInner),
    /// A dependency that performs invocations (ex. `http-server` invoking a component's `wasi:http/incoming-handler` export)
    Imports(DependencySpecInner),
}

impl DependencySpec {
    /// Retrieve the name for this dependency
    fn name(&self) -> String {
        match self {
            DependencySpec::Exports(inner) => inner.name(),
            DependencySpec::Imports(inner) => inner.name(),
        }
    }

    /// Retrieve the image_ref for this dependency
    fn image_ref(&self) -> Option<&str> {
        match self {
            DependencySpec::Exports(inner) => inner.image_ref(),
            DependencySpec::Imports(inner) => inner.image_ref(),
        }
    }

    /// Retrieve whether this spec is a component or not
    ///
    /// Components must be especially noted because by default providers are expected
    /// to provide functionality, but it also possible for components to do so.
    fn is_component(&self) -> bool {
        match self {
            DependencySpec::Exports(inner) => inner.is_component(),
            DependencySpec::Imports(inner) => inner.is_component(),
        }
    }

    /// Retrieve configs for this component spec
    fn configs(&self) -> &Vec<ConfigProperty> {
        match self {
            DependencySpec::Exports(inner) => &inner.configs,
            DependencySpec::Imports(inner) => &inner.configs,
        }
    }

    /// Retrieve secrets for this component spec
    fn secrets(&self) -> &Vec<SecretProperty> {
        match self {
            DependencySpec::Exports(inner) => &inner.secrets,
            DependencySpec::Imports(inner) => &inner.secrets,
        }
    }
}

impl DependencySpec {
    /// Derive which local component should be used given a WIT interface to be satisified
    ///
    /// # Examples
    ///
    /// ```
    /// let v = from_wit_import_face("wasi:keyvalue/atomics");
    /// # assert!(v.is_some())
    /// ```
    fn from_wit_import_iface(iface: &str) -> Option<Self> {
        let (iface, version) = match iface.split_once('@') {
            None => (iface, None),
            Some((iface, version)) => (iface, semver::Version::parse(version).ok()),
        };
        let (ns, pkg, iface, _) = parse_wit_meta_from_operation(format!("{iface}.none")).ok()?;
        match (ns.as_str(), pkg.as_str(), iface.as_str()) {
            // Skip explicitly ignored (normally internal) interfaces
            (ns, pkg, iface) if is_ignored_iface_dep(ns, pkg, iface) => None,
            // Deal with known prefixes
            ("wasi", "keyvalue", "atomics" | "store" | "batch") => {
                Some(Self::Exports(DependencySpecInner {
                    wit: (
                        ns,
                        pkg,
                        iface,
                        version.map(VersionCoverage::SemVer).unwrap_or_default(),
                    ),
                    delegated_to_workspace: false,
                    link_name: "default".into(),
                    image_ref: Some(DEFAULT_KEYVALUE_PROVIDER_IMAGE.into()),
                    // TODO: needs config on the source->target link (Bucket name)
                    ..Default::default()
                }))
            }
            ("wasi", "http", "outgoing-handler") => Some(Self::Exports(DependencySpecInner {
                wit: (
                    ns,
                    pkg,
                    iface,
                    version.map(VersionCoverage::SemVer).unwrap_or_default(),
                ),
                delegated_to_workspace: false,
                link_name: "default".into(),
                image_ref: Some(DEFAULT_HTTP_CLIENT_PROVIDER_IMAGE.into()),
                ..Default::default()
            })),
            ("wasi", "blobstore", "blobstore") | ("wrpc", "blobstore", "blobstore") => {
                Some(Self::Exports(DependencySpecInner {
                    wit: (
                        ns,
                        pkg,
                        iface,
                        version.map(VersionCoverage::SemVer).unwrap_or_default(),
                    ),
                    delegated_to_workspace: false,
                    link_name: "default".into(),
                    image_ref: Some(DEFAULT_BLOBSTORE_FS_PROVIDER_IMAGE.into()),
                    // TODO: needs config on source->target link (ROOT)
                    ..Default::default()
                }))
            }
            ("wasmcloud", "messaging", "consumer") => Some(Self::Exports(DependencySpecInner {
                wit: (
                    ns,
                    pkg,
                    iface,
                    version.map(VersionCoverage::SemVer).unwrap_or_default(),
                ),
                delegated_to_workspace: false,
                link_name: "default".into(),
                image_ref: Some(DEFAULT_MESSAGING_NATS_PROVIDER_IMAGE.into()),
                ..Default::default()
            })),
            // Treat all other dependencies as custom, and track them as dependencies,
            // though they cannot be resolved to a proper dependency without an explicit override/
            // other configuration method
            _ => Some(Self::Exports(DependencySpecInner {
                wit: (
                    ns,
                    pkg,
                    iface,
                    version.map(VersionCoverage::SemVer).unwrap_or_default(),
                ),
                delegated_to_workspace: false,
                link_name: "default".into(),
                image_ref: None,
                ..Default::default()
            })),
        }
    }

    /// Derive which local component should be used given a WIT interface to be satisified
    ///
    /// # Examples
    ///
    /// ```
    /// let v = from_wit_export_face("wasi:http/incoming-handler");
    /// # assert!(v.is_some())
    /// ```
    fn from_wit_export_iface(iface: &str) -> Option<Self> {
        let (iface, version) = match iface.split_once('@') {
            None => (iface, None),
            Some((iface, version)) => (iface, semver::Version::parse(version).ok()),
        };
        let (ns, pkg, iface, _) = parse_wit_meta_from_operation(format!("{iface}.none")).ok()?;
        match (ns.as_ref(), pkg.as_ref(), iface.as_ref()) {
            // Skip explicitly ignored (normally internal) interfaces
            (ns, pkg, iface) if is_ignored_iface_dep(ns, pkg, iface) => None,
            // Handle known interfaces
            ("wasi", "http", "incoming-handler") => Some(Self::Imports(DependencySpecInner {
                wit: (
                    ns,
                    pkg,
                    iface,
                    version.map(VersionCoverage::SemVer).unwrap_or_default(),
                ),
                delegated_to_workspace: false,
                link_name: "default".into(),
                image_ref: Some(DEFAULT_HTTP_SERVER_PROVIDER_IMAGE.into()),
                ..Default::default()
            })),
            ("wasmcloud", "messaging", "handler") => Some(Self::Imports(DependencySpecInner {
                wit: (
                    ns,
                    pkg,
                    iface,
                    version.map(VersionCoverage::SemVer).unwrap_or_default(),
                ),
                delegated_to_workspace: false,
                link_name: "default".into(),
                image_ref: Some(DEFAULT_MESSAGING_NATS_PROVIDER_IMAGE.into()),
                // TODO: needs config on the provider->component link (subscriptions)
                ..Default::default()
            })),
            // Treat all other dependencies as custom, and track them as dependencies,
            // though they cannot be resolved to a proper dependency without an explicit override/
            // other configuration method
            _ => Some(Self::Imports(DependencySpecInner {
                wit: (
                    ns,
                    pkg,
                    iface,
                    version.map(VersionCoverage::SemVer).unwrap_or_default(),
                ),
                delegated_to_workspace: false,
                link_name: "default".into(),
                image_ref: Some(DEFAULT_MESSAGING_NATS_PROVIDER_IMAGE.into()),
                // TODO: needs config on the provider->component link (subscriptions)
                ..Default::default()
            })),
        }
    }

    fn generate_properties(&self, name: &str) -> Result<Properties> {
        let properties = match self.image_ref() {
            Some(
                DEFAULT_HTTP_CLIENT_PROVIDER_IMAGE
                | DEFAULT_HTTP_SERVER_PROVIDER_IMAGE
                | DEFAULT_BLOBSTORE_FS_PROVIDER_IMAGE
                | DEFAULT_MESSAGING_NATS_PROVIDER_IMAGE
                | DEFAULT_KEYVALUE_PROVIDER_IMAGE,
            ) => Properties::Capability {
                properties: CapabilityProperties {
                    image: self
                        .image_ref()
                        .with_context(|| {
                            format!(
                                "missing image ref for generated (known) component dependency [{}]",
                                name,
                            )
                        })?
                        .into(),
                    id: None,
                    config: self.configs().clone(),
                    secrets: self.secrets().clone(),
                },
            },
            // For image refs that we don't recognize, we can't tell easily
            // if they are capabilities or components and could well be either.
            _ => {
                if self.is_component() {
                    Properties::Component {
                        properties: ComponentProperties {
                            image: self
                                .image_ref()
                                .with_context(|| {
                                    format!(
                                        "missing image ref for generated component dependency [{}]",
                                        self.name()
                                    )
                                })?
                                .into(),
                            id: None,
                            config: self.configs().clone(),
                            secrets: self.secrets().clone(),
                        },
                    }
                } else {
                    Properties::Capability {
                        properties: CapabilityProperties {
                            image: self
                                .image_ref()
                                .with_context(|| {
                                    format!(
                                        "missing image ref for generated provider dependency [{}]",
                                        self.name()
                                    )
                                })?
                                .into(),
                            id: None,
                            config: self.configs().clone(),
                            secrets: self.secrets().clone(),
                        },
                    }
                }
            }
        };
        Ok(properties)
    }

    /// Convert to a component that can be used in a [`Manifest`] with a given suffix for uniqueness
    fn generate_component(&self, suffix: &str) -> Result<Component> {
        let name = format!("{}-dep-{}", suffix, self.name());
        let properties = self
            .generate_properties(suffix)
            .context("failed to generate properties for component")?;
        Ok(Component {
            name,
            properties,
            traits: Some(Vec::new()),
        })
    }
}

/// Versions of interfaces (in this context WIT interfaces) that are covered
///
/// Generally, this enum is used to resolve conflicts between providers
/// that satisfy similar (possibly just slightly differently versioned) interfaces.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
enum VersionCoverage {
    #[default]
    All,
    SemVer(Version),
}

/// Specification of a dependency (possibly implied)
#[derive(Default, Debug, Clone, PartialEq, Eq)]
struct DependencySpecInner {
    /// Relevant WIT information that represents the dependency
    ///
    /// The interfaces that the dependency receives
    ///
    /// This generally means that the component will
    wit: (WitNamespace, WitPackage, WitInterface, VersionCoverage),

    /// Whether this dependency should delegated to the workspace
    delegated_to_workspace: bool,

    /// Image reference to the component that should be inserted/used
    ///
    /// This reference *can* be missing if an override is specified with no image,
    /// which can happen in at least two cases:
    /// - custom WIT-defined interface imports/exports (we may not know at WIT processing what their overrides will be)
    /// - project-level with workspace delegation  (we may not know what the image ref is at the project level)
    image_ref: Option<String>,

    /// Whether this dependency represents a WebAssembly component, rather than a (standalone binary) provider
    is_component: bool,

    /// The link name this dependency should be connected over
    ///
    /// In the vast majority of cases, this will be "default", but it may not be
    /// if the same interface is used to link to multiple different providers/components
    link_name: LinkName,

    /// Configurations that must be created and/or consumed by this dependency
    configs: Vec<wadm_types::ConfigProperty>,

    /// Secrets that must be created and/or consumed by this dependency
    ///
    /// [`SecretProperty`] here support a special `policy` value which is 'env'.
    /// Paired with a key that looks like "$SOME_VALUE", the value will be extracted from ENV *prior* and
    secrets: Vec<wadm_types::SecretProperty>,
}

impl DependencySpecInner {
    /// Retrieve the name for this dependency
    fn name(&self) -> String {
        match self.image_ref.as_deref() {
            Some(DEFAULT_KEYVALUE_PROVIDER_IMAGE) => "keyvalue-nats".into(),
            Some(DEFAULT_HTTP_CLIENT_PROVIDER_IMAGE) => "http-client".into(),
            Some(DEFAULT_HTTP_SERVER_PROVIDER_IMAGE) => "http-server".into(),
            Some(DEFAULT_BLOBSTORE_FS_PROVIDER_IMAGE) => "blobstore-fs".into(),
            Some(DEFAULT_MESSAGING_NATS_PROVIDER_IMAGE) => "messaging-nats".into(),
            _ => format!("custom-{}-{}", self.wit.0, self.wit.1),
        }
    }

    /// Retrieve the image reference for this dependency
    fn image_ref(&self) -> Option<&str> {
        self.image_ref.as_deref()
    }

    /// Retrieve whether this dependency spec is a component
    fn is_component(&self) -> bool {
        self.is_component
    }
}

/// Information related to the dependencies of a given project
///
/// Projects can either be inside workspaces, or not (single component/provider).
#[derive(Default, Debug, Clone, PartialEq, Eq)]
struct ProjectDeps {
    /// ID of a session
    ///
    /// This is normally used when generating sessions to be used with `wash dev`, in order
    /// to make sure various dependencies and related files can be uniquely identified.
    pub(crate) session_id: Option<String>,

    /// Lookup of dependencies by project key, with lookups into the pool
    dependencies: HashMap<ProjectDependencyKey, Vec<DependencySpec>>,

    /// The component to which dependencies belong
    ///
    /// When used in the context of `wash dev` this is the component that is being developed
    /// (either a provider or a component).
    component: Option<Component>,

    /// Dependencies that receive invocations for given interfaces (i.e. `keyvalue-nats` receiving a `wasi:keyvalue/get`)
    ///
    /// The keys to this structure are indices into the `dependencies` member.
    exporters: HashMap<(WitNamespace, WitPackage, WitInterface, Option<Version>), Vec<usize>>,

    /// Dependencies that perform invocations for given interfaces (i.e. `http-server` invoking a component's `wasi:http/incoming-handler` export)
    importers: HashMap<(WitNamespace, WitPackage, WitInterface, Option<Version>), Vec<usize>>,
}

impl ProjectDeps {
    /// Build a [`ProjectDeps`] for a project/workspace entirely from [`DependencySpec`]s
    pub fn from_known_deps(
        pkey: ProjectDependencyKey,
        deps: impl IntoIterator<Item = DependencySpec>,
    ) -> Result<Self> {
        let mut pdeps = Self::default();
        pdeps
            .add_known_deps(deps.into_iter().map(|dep| (pkey.clone(), dep)))
            .context("failed to add deps while building project dependencies")?;
        Ok(pdeps)
    }

    /// Build a [`ProjectDeps`] from a given project/workspace configuration
    pub fn from_project_config(_cfg: &ProjectConfig) -> Result<Self> {
        // TODO: implement overrides
        Ok(Self::default())
    }

    /// Add one or more [`DependencySpec`]s to the current [`ProjectDeps`]
    ///
    /// To add a known dependency to this list of project dependencies, we must know *which* project
    /// the dependency belongs to, or whether it corresponds to the workspace.
    pub fn add_known_deps(
        &mut self,
        deps: impl IntoIterator<Item = (ProjectDependencyKey, DependencySpec)>,
    ) -> Result<()> {
        for (pkey, dep) in deps.into_iter() {
            self.dependencies.entry(pkey).or_default().push(dep);
        }
        Ok(())
    }

    /// Merge another bundle of dependencies (possibly derived from some other source of metadata)
    ///
    /// Note that the `other` will override the values `self`, where necessary.
    fn merge_override(&mut self, other: Self) -> Result<()> {
        // ProjectDeps should have matching sessions, if specified
        if self.session_id != other.session_id {
            bail!(
                "session IDs (if specified) must match for merging deps. Session ID [{}] does not match [{}]",
                self.session_id.as_deref().unwrap_or("<none>"),
                other.session_id.as_deref().unwrap_or("<none>"),
            );
        }

        // Add dependencies from other
        self.dependencies.extend(other.dependencies);

        // TODO: merge dependencies with identical keys, while implementing overrides

        Ok(())
    }

    /// Generate a WADM manifest from the current group of project dependencies
    ///
    /// A session ID, when provided, is uses to distinguish resources from others that might be running in the lattice.
    /// Primarily this session ID is used to distinguish the names of WADM application manifests, enabling them to
    /// be easily referenced/compared, and replaced if necessary.
    ///
    /// Project dependencies, spread across large/nested enough workspace can lead to *multiple* Applications manifests
    /// being produced -- in general every workspace *should* produce a distinct manifest, but it is possible a workspace
    /// could produce zero manifests (for example, if all resources are delegated to a higher level manifest).
    fn generate_wadm_manifests(&self) -> Result<impl IntoIterator<Item = Manifest>> {
        let mut manifests = Vec::<Manifest>::new();
        let session_id = self
            .session_id
            .as_ref()
            .context("missing/invalid session ID")?;
        let mut component = self
            .component
            .clone()
            .context("missing/invalid component under test")?;
        let app_name = format!("dev-{}", component.name.to_lowercase().replace(" ", "-"));

        // Generate components for all the dependencies
        let mut components = Vec::new();

        // For each dependency, go through and generate the component along with necessary links
        for dep in self.dependencies.values().flatten() {
            let dep = dep.clone();
            let mut dep_component = dep
                .generate_component(session_id)
                .context("failed to generate component")?;

            // Generate links for the given component and it's spec, for known interfaces
            match dep {
                DependencySpec::Exports(DependencySpecInner {
                    wit: (ns, pkg, iface, version),
                    ..
                }) => {
                    // Build the relevant app->dep link trait
                    let mut link_property = LinkProperty {
                        namespace: ns.clone(),
                        package: pkg.clone(),
                        interfaces: vec![iface.clone()],
                        target: TargetConfig {
                            name: dep_component.name.clone(),
                            ..Default::default()
                        },
                        ..Default::default()
                    };

                    // Make interface-specific changes
                    match (ns.as_ref(), pkg.as_ref(), iface.as_ref(), version) {
                        ("wasi", "blobstore", "blobstore", _)
                        | ("wrpc", "blobstore", "blobstore", _) => {
                            link_property.target.config.push(ConfigProperty {
                                name: "default".into(),
                                properties: Some(HashMap::from([(
                                    "root".into(),
                                    DEFAULT_BLOBSTORE_ROOT_DIR.into(),
                                )])),
                            });
                        }
                        ("wasi", "keyvalue", "atomics" | "store" | "batch", _) => {
                            link_property.target.config.push(ConfigProperty {
                                name: "default".into(),
                                properties: Some(HashMap::from([
                                    ("bucket".into(), DEFAULT_KEYVALUE_BUCKET.into()),
                                    ("enable_bucket_auto_create".into(), "true".into()),
                                ])),
                            });
                        }
                        _ => {}
                    }

                    let link_trait = wadm_types::Trait {
                        trait_type: "link".into(),
                        properties: TraitProperty::Link(link_property),
                    };

                    // TODO: Check for on-dep overrides/additional configuration to add to the link

                    // Add the trait to the app component
                    component.traits.get_or_insert(Vec::new()).push(link_trait);
                }
                DependencySpec::Imports(DependencySpecInner {
                    wit: (ns, pkg, iface, version),
                    ..
                }) => {
                    // Build the relevant dep->app link trait
                    let mut link_property = LinkProperty {
                        namespace: ns.clone(),
                        package: pkg.clone(),
                        interfaces: vec![iface.clone()],
                        target: TargetConfig {
                            name: component.name.clone(),
                            ..Default::default()
                        },
                        ..Default::default()
                    };

                    // Make interface-specific tweaks to the generated trait
                    match (ns.as_ref(), pkg.as_ref(), iface.as_ref(), version) {
                        ("wasi", "http", "incoming-handler", _) => {
                            link_property
                                .source
                                .get_or_insert(Default::default())
                                .config
                                .push(ConfigProperty {
                                    name: "default".into(),
                                    properties: Some(HashMap::from([(
                                        "address".into(),
                                        DEFAULT_INCOMING_HANDLER_ADDRESS.into(),
                                    )])),
                                });
                        }
                        ("wasmcloud", "messaging", "handler", _) => {
                            link_property
                                .source
                                .get_or_insert(Default::default())
                                .config
                                .push(ConfigProperty {
                                    name: "default".into(),
                                    properties: Some(HashMap::from([(
                                        "subscriptions".into(),
                                        DEFAULT_MESSAGING_HANDLER_SUBSCRIPTION.into(),
                                    )])),
                                });
                        }
                        _ => {}
                    }

                    // TODO: Check for on-dep overrides/additional configuration to add to the link

                    let link_trait = wadm_types::Trait {
                        trait_type: "link".into(),
                        properties: TraitProperty::Link(link_property),
                    };

                    // Add the trait
                    dep_component
                        .traits
                        .get_or_insert(Vec::new())
                        .push(link_trait);
                }
            }

            // Add the dependency component after we've made necessary links
            components.push(dep_component);
        }

        // Add the application component after we've made necessary links
        components.push(component);

        manifests.push(Manifest {
            api_version: "0.0.0".into(),
            kind: "Application".into(),
            metadata: Metadata {
                name: app_name,
                annotations: BTreeMap::from([("version".into(), "v0.0.0".into())]),
                labels: BTreeMap::new(),
            },
            spec: Specification {
                components,
                policies: Vec::from([Policy {
                    name: "nats-kv".into(),
                    policy_type: "policy.secret.wasmcloud.dev/v1alpha1".into(),
                    properties: BTreeMap::from([("backend".into(), "nats-kv".into())]),
                }]),
            },
        });

        Ok(manifests)
    }

    /// Delete all manifests associated with this [`ProjectDeps`]
    async fn delete_manifests(
        &self,
        client: &async_nats_0_33::Client,
        lattice: &str,
    ) -> Result<()> {
        for manifest in self
            .generate_wadm_manifests()
            .context("failed to generate manifests")?
            .into_iter()
        {
            wash_lib::app::delete_model_version(
                client,
                Some(lattice.into()),
                &manifest.metadata.name,
                Some(manifest.version().into()),
            )
            .await
            .with_context(|| {
                format!("failed to delete application [{}]", manifest.metadata.name)
            })?;
        }

        Ok(())
    }
}

/// Check whether the provided interface is ignored for the purpose of building dependencies.
///
/// Dependencies are ignored normally if they are known-internal packages.
fn is_ignored_iface_dep(ns: &str, pkg: &str, iface: &str) -> bool {
    matches!(
        (ns, pkg, iface),
        ("wasi", "io", _) | ("wasi", "clocks", _) | ("wasi", "http", "types")
    )
}

/// Parse Build a [`wit_parser::Resolve`] from a provided directory
/// and select a given world
pub fn parse_project_wit(project_cfg: &ProjectConfig) -> Result<(Resolve, WorldId)> {
    let project_dir = &project_cfg.common.path;
    let wit_dir = project_dir.join("wit");
    let world = project_cfg.project_type.wit_world();

    // Resolve the WIT directory packages & worlds
    let mut resolve = wit_parser::Resolve::default();
    let (package_id, _paths) = resolve
        .push_dir(wit_dir)
        .with_context(|| format!("failed to add WIT directory @ [{}]", project_dir.display()))?;

    // Select the target world that was specified by the user
    let world_id = resolve
        .select_world(package_id, world.as_deref())
        .context("failed to select world from built resolver")?;

    Ok((resolve, world_id))
}

/// Resolve the dependencies of a given WIT world that map to WADM components
///
/// Normally, this means converting imports that the component depends on to
/// components that can be run on the lattice.
fn discover_dependencies_from_wit(
    resolve: Resolve,
    world_id: WorldId,
) -> Result<Vec<DependencySpec>> {
    let mut deps = Vec::new();
    let world = resolve
        .worlds
        .get(world_id)
        .context("selected WIT world is missing")?;
    // Process imports
    for (_key, item) in world.imports.iter() {
        if let wit_parser::WorldItem::Interface { id, .. } = item {
            let iface = resolve
                .interfaces
                .get(*id)
                .context("unexpectedly missing iface")?;
            let pkg = resolve
                .packages
                .get(iface.package.context("iface missing package")?)
                .context("failed to find package")?;
            let iface_name = &format!(
                "{}:{}/{}",
                pkg.name.namespace,
                pkg.name.name,
                iface.name.as_ref().context("interface missing name")?,
            );
            if let Some(dep) = DependencySpec::from_wit_import_iface(iface_name) {
                deps.push(dep);
            }
        }
    }
    // Process exports
    for (_key, item) in world.exports.iter() {
        if let wit_parser::WorldItem::Interface { id, .. } = item {
            let iface = resolve
                .interfaces
                .get(*id)
                .context("unexpectedly missing iface")?;
            let pkg = resolve
                .packages
                .get(iface.package.context("iface missing package")?)
                .context("failed to find package")?;
            let iface_name = &format!(
                "{}:{}/{}",
                pkg.name.namespace,
                pkg.name.name,
                iface.name.as_ref().context("interface missing name")?,
            );
            if let Some(dep) = DependencySpec::from_wit_export_iface(iface_name) {
                deps.push(dep);
            }
        }
    }

    Ok(deps)
}

/// Generate a WADM component from a project configuration
fn generate_component_from_project_cfg(
    cfg: &ProjectConfig,
    component_id: &str,
    image_ref: &str,
) -> Result<Component> {
    Ok(Component {
        name: component_id.into(),
        properties: match &cfg.project_type {
            wash_lib::parser::TypeConfig::Component(_c) => Properties::Component {
                properties: ComponentProperties {
                    image: image_ref.into(),
                    id: Some(component_id.into()),
                    config: Vec::with_capacity(0),
                    secrets: Vec::with_capacity(0),
                },
            },
            wash_lib::parser::TypeConfig::Provider(_p) => Properties::Capability {
                properties: CapabilityProperties {
                    image: image_ref.into(),
                    id: Some(component_id.into()),
                    config: Vec::with_capacity(0),
                    secrets: Vec::with_capacity(0),
                },
            },
        },
        traits: Some(vec![wadm_types::Trait {
            trait_type: "spreadscaler".into(),
            properties: TraitProperty::SpreadScaler(SpreadScalerProperty {
                instances: 1,
                spread: Vec::new(),
            }),
        }]),
    })
}

/// The path to the dev directory for wash
async fn dev_dir() -> Result<PathBuf> {
    let dir = cfg_dir()
        .context("failed to resolve config dir")?
        .join(WASH_DEV_DIR);
    if !tokio::fs::try_exists(&dir)
        .await
        .context("failed to check if dev dir exists")?
    {
        tokio::fs::create_dir(&dir)
            .await
            .with_context(|| format!("failed to create dir [{}]", dir.display()))?
    }
    Ok(dir)
}

/// Retrieve the path to the file that stores
async fn sessions_file_path() -> Result<PathBuf> {
    dev_dir()
        .await
        .map(|p| p.join(WASH_SESSIONS_FILE_NAME))
        .context("failed to get dev dir")
}

/// Metadata related to a single `wash dev` session
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WashDevSession {
    /// Session ID
    id: String,
    /// Absolute path to the directory in which `wash dev` was run
    project_path: PathBuf,
    /// Tuple containing data about the host, in particular the
    /// host ID and path to log file
    ///
    /// This value may start out empty, but is filled in when a host is started
    host_data: Option<(String, PathBuf)>,
    /// Whether this session is currently in use
    in_use: bool,
    /// When this session was created
    created_at: DateTime<Utc>,
    /// When the wash dev session was last used
    last_used_at: DateTime<Utc>,
}

/// The structure of an a file containing sessions of `wash dev`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// Version of the sessions sessions file
    version: Version,
    /// Sessions of `wash dev` that have been run at some point
    sessions: Vec<WashDevSession>,
}

impl SessionMetadata {
    /// Get the session file
    async fn open_sessions_file() -> Result<std::fs::File> {
        let sessions_file_path = sessions_file_path().await?;
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .append(false)
            .truncate(false)
            .open(&sessions_file_path)
            .with_context(|| {
                format!(
                    "failed to open sessions file [{}]",
                    sessions_file_path.display()
                )
            })
    }

    /// Build metadata from default file on disk
    async fn from_sessions_file() -> Result<Self> {
        // Open and lock the sessions file
        let mut sessions_file = Self::open_sessions_file().await?;
        let mut lock = file_guard::lock(&mut sessions_file, file_guard::Lock::Exclusive, 0, 1)?;

        // Load session metadata, if present
        let file_size = (*lock)
            .metadata()
            .context("failed to get sessions file metadata")?
            .len();
        let session_metadata: SessionMetadata = if file_size == 0 {
            SessionMetadata::default()
        } else {
            let sessions_file_path = sessions_file_path().await?;
            tokio::task::block_in_place(move || {
                let mut file_contents = Vec::with_capacity(
                    usize::try_from(file_size).context("failed to convert file size to usize")?,
                );
                lock.read_to_end(&mut file_contents)
                    .context("failed to read file contents")?;
                serde_json::from_slice::<SessionMetadata>(&file_contents).with_context(|| {
                    format!(
                        "failed to parse session metadata from file [{}]",
                        sessions_file_path.display(),
                    )
                })
            })
            .with_context(|| format!("failed to read session metadata ({file_size} bytes)"))?
        };
        Ok(session_metadata)
    }

    /// Persist a single session to the metadata file that is on disk
    async fn persist_session(session: &WashDevSession) -> Result<()> {
        // Lock the session file
        let sessions_file_path = sessions_file_path().await?;
        let mut sessions_file = Self::open_sessions_file().await?;
        let mut lock = file_guard::lock(&mut sessions_file, file_guard::Lock::Exclusive, 0, 1)?;

        // Read the session file and ensure that the content is exactly similar to what we have now
        let file_size = (*lock)
            .metadata()
            .context("failed to get sessions file metadata")?
            .len();
        let mut session_metadata = if file_size == 0 {
            SessionMetadata::default()
        } else {
            tokio::task::block_in_place(|| {
                let mut file_contents = Vec::with_capacity(
                    usize::try_from(file_size).context("failed to convert file size to usize")?,
                );
                lock.read_to_end(&mut file_contents)
                    .context("failed to read file contents")?;
                serde_json::from_slice::<SessionMetadata>(&file_contents).with_context(|| {
                    format!(
                        "failed to parse session metadata from file [{}]",
                        sessions_file_path.display(),
                    )
                })
            })
            .context("failed to read session metadata while modifying session")?
        };

        // Update the session that was present
        if let Some(s) = session_metadata
            .sessions
            .iter_mut()
            .find(|s| s.id == session.id)
        {
            *s = session.clone();
        }

        // Write current updated session metadata to file
        tokio::fs::write(
            sessions_file_path,
            &serde_json::to_vec_pretty(&session_metadata)
                .context("failed to write session metadata")?,
        )
        .await?;

        Ok(())
    }
}

impl Default for SessionMetadata {
    fn default() -> Self {
        Self {
            version: SESSIONS_FILE_VERSION,
            sessions: Vec::new(),
        }
    }
}

impl WashDevSession {
    /// Get the directory into which all related log files/ancillary data should be stored
    async fn base_dir(&self) -> Result<PathBuf> {
        let base_dir = dev_dir().await.map(|p| p.join(&self.id))?;
        if !tokio::fs::try_exists(&base_dir)
            .await
            .context("failed to check if dev dir exists")?
        {
            tokio::fs::create_dir_all(&base_dir)
                .await
                .with_context(|| format!("failed to create dir [{}]", base_dir.display()))?
        }
        Ok(base_dir)
    }

    /// Retrieve or create a `wash dev` session from a file on disk containing [`SessionMetadata`]
    async fn from_sessions_file(project_path: impl AsRef<Path>) -> Result<Self> {
        let mut session_metadata = SessionMetadata::from_sessions_file()
            .await
            .context("failed to load session metadata")?;
        let project_path = project_path.as_ref();

        // Attempt to find an session with the given path, creating one if necessary
        let session = match session_metadata
            .sessions
            .iter()
            .find(|s| s.project_path == project_path && !s.in_use)
        {
            Some(existing_session) => existing_session.clone(),
            None => {
                let session = WashDevSession {
                    id: rand::thread_rng()
                        .sample_iter(&Alphanumeric)
                        .take(SESSION_ID_LEN)
                        .map(char::from)
                        .collect(),
                    project_path: project_path.into(),
                    host_data: None,
                    in_use: true,
                    created_at: Utc::now(),
                    last_used_at: Utc::now(),
                };
                session_metadata.sessions.push(session.clone());
                session
            }
        };

        Ok(session)
    }

    /// Start a host for the given session, if one is not present
    async fn start_host(
        &mut self,
        mut wasmcloud_opts: WasmcloudOpts,
        nats_opts: NatsOpts,
        wadm_opts: WadmOpts,
    ) -> Result<(Option<Child>, Option<Child>, Option<Child>)> {
        if self.host_data.is_some() {
            return Ok((None, None, None));
        }

        eprintln!(
            "{} {}",
            emoji::CONSTRUCTION_BARRIER,
            style("Starting a new host...").bold()
        );
        // Ensure that file loads are allowed
        wasmcloud_opts.allow_file_load = Some(true);
        wasmcloud_opts.multi_local = true;

        let session_dir = self.base_dir().await?;

        // Start NATS
        let install_dir = downloads_dir()?;
        let nats_log_path = session_dir.join("nats.log");
        let nats_binary = ensure_nats_server(&nats_opts.nats_version, &install_dir).await?;
        let nats_host = nats_opts.nats_host.clone().unwrap_or_else(|| {
            wasmcloud_opts
                .ctl_host
                .clone()
                .unwrap_or_else(|| DEFAULT_NATS_HOST.into())
        });
        let nats_port = nats_opts
            .nats_port
            .unwrap_or(wasmcloud_opts.ctl_port.unwrap_or(4222));
        let nats_listen_address = format!("{}:{}", nats_host, nats_port);
        let nats_config = NatsConfig {
            host: nats_host,
            port: nats_port,
            store_dir: std::env::temp_dir().join(format!("wash-jetstream-{nats_port}")),
            js_domain: nats_opts.nats_js_domain,
            remote_url: nats_opts.nats_remote_url,
            credentials: nats_opts.nats_credsfile.clone(),
            websocket_port: nats_opts.nats_websocket_port,
            config_path: nats_opts.nats_configfile,
        };
        let nats_child = match start_nats(
            &install_dir,
            &nats_binary,
            nats_config,
            &nats_log_path,
            CommandGroupUsage::CreateNew,
        )
        .await
        {
            Ok(c) => Some(c),
            Err(e) if e.to_string().contains("already listening") => None,
            Err(e) => bail!("failed to start NATS server for wash dev: {e}"),
        };

        // Start WADM
        let wadm_log_path = session_dir.join("wadm.log");
        let config = WadmConfig {
            structured_logging: wasmcloud_opts.enable_structured_logging,
            js_domain: wadm_opts.wadm_js_domain.clone(),
            nats_server_url: nats_listen_address,
            nats_credsfile: nats_opts.nats_credsfile,
        };
        let wadm_log_file = tokio::fs::File::create(&wadm_log_path)
            .await
            .with_context(|| {
                format!(
                    "failed to create wadm log file @ [{}]",
                    wadm_log_path.display()
                )
            })?;
        let wadm_binary = ensure_wadm(&wadm_opts.wadm_version, &install_dir).await?;
        let wadm_child = match start_wadm(
            &session_dir,
            &wadm_binary,
            wadm_log_file.into_std().await,
            Some(config),
            CommandGroupUsage::CreateNew,
        )
        .await
        {
            Ok(c) => Some(c),
            Err(e) if e.to_string().contains("already listening") => None,
            Err(e) => bail!("failed to start wadm for wash dev: {e}"),
        };

        // Start the host in detached mode, w/ custom log file
        let wasmcloud_log_path = session_dir.join("wasmcloud.log");
        let wasmcloud_binary =
            ensure_wasmcloud(&wasmcloud_opts.wasmcloud_version, &install_dir).await?;
        let log_output: Stdio = tokio::fs::File::create(&wasmcloud_log_path)
            .await
            .with_context(|| {
                format!(
                    "failed to create log file @ [{}]",
                    wasmcloud_log_path.display()
                )
            })?
            .into_std()
            .await
            .into();
        let host_env = configure_host_env(wasmcloud_opts.clone()).await?;
        let wasmcloud_child = match start_wasmcloud_host(
            &wasmcloud_binary,
            std::process::Stdio::null(),
            log_output,
            host_env,
        )
        .await
        {
            Ok(child) => Some(child),
            Err(e) => {
                eprintln!("{} Failed to start wasmCloud instance", emoji::ERROR);
                if let Some(mut wadm) = wadm_child {
                    wadm.kill()
                        .await
                        .context("failed to stop wadm child process")?;
                    remove_wadm_pidfile(session_dir)
                        .await
                        .context("failed to remove wadm pidfile")?;
                }
                let nats_bin = install_dir.join(NATS_SERVER_BINARY);
                let _ = stop_nats(install_dir, nats_bin).await?;
                bail!("failed to start wasmCloud instance: {e}");
            }
        };

        eprintln!(
            "{} {}",
            emoji::WRENCH,
            style("Successfully started wasmCloud instance").bold(),
        );

        // Read the log until we get output that
        let _wasmcloud_log_path = wasmcloud_log_path.clone();
        let host_id = tokio::time::timeout(tokio::time::Duration::from_secs(1), async move {
            let log_file = tokio::fs::File::open(&_wasmcloud_log_path)
                .await
                .with_context(|| {
                    format!(
                        "failed to open log file @ [{}]",
                        _wasmcloud_log_path.display()
                    )
                })?;
            let mut lines = tokio::io::BufReader::new(log_file).lines();
            loop {
                if let Some(line) = lines
                    .next_line()
                    .await
                    .context("failed to read line from file")?
                {
                    if let Some(host_id) = line
                        .split_once("host_id=\"")
                        .map(|(_, rhs)| &rhs[0..rhs.len() - 1])
                    {
                        return Ok(host_id.to_string()) as anyhow::Result<String>;
                    }
                }
            }
        })
        .await
        .context("timeout expired while reading for Host ID in logs")?
        .context("failed to retrieve host ID from logs")?;

        eprintln!(
            "{} {}",
            emoji::GREEN_CHECK,
            style(format!(
                "Successfully started host, logs @ [{}]",
                wasmcloud_log_path.display()
            ))
            .bold()
        );

        self.host_data = Some((host_id, wasmcloud_log_path));

        Ok((nats_child, wadm_child, wasmcloud_child))
    }
}

/// Find the first config value for provider  trait configuration configuration which has a certain name
fn find_provider_source_trait_config_value<'a>(
    component: &'a Component,
    config_name: &'a str,
    property_key: &'a str,
) -> Option<&'a str> {
    // Retrieve link traits
    if let Some(link_traits) = component
        .traits
        .as_ref()
        .map(|ts| ts.iter().filter(|t| t.trait_type == "link"))
    {
        // Find the first link config that is named "default" and has "address"
        for link_trait in link_traits {
            if let TraitProperty::Link(l) = &link_trait.properties {
                if let Some(def) = &l.source {
                    for cfg in &def.config {
                        if let (name, Some(Some(value))) = (
                            &cfg.name,
                            cfg.properties.as_ref().map(|p| p.get(property_key)),
                        ) {
                            if name == config_name {
                                return Some(value);
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

/// Generate help text for manifest with components that we recognize
fn generate_help_text_for_manifest(manifest: &Manifest) -> Vec<String> {
    let mut lines = Vec::new();
    for component in manifest.spec.components.iter() {
        match &component.properties {
            // Add help text for HTTP server
            Properties::Capability { properties }
                if properties
                    .image
                    .starts_with("ghcr.io/wasmcloud/http-server") =>
            {
                if let Some(address) =
                    find_provider_source_trait_config_value(component, "default", "address")
                {
                    lines.push(format!(
                        "{} {}",
                        emoji::INFO_SQUARE,
                        style(format!(
                            "HTTP Server: Access your application at {}",
                            if address.starts_with("http") {
                                address.into()
                            } else {
                                format!("http://{address}")
                            }
                        ))
                        .bold()
                    ));
                }
            }
            // Add help text for Messaging server
            Properties::Capability { properties }
                if properties
                    .image
                    .starts_with("ghcr.io/wasmcloud/messaging-nats") =>
            {
                if let Some(subscriptions) =
                    find_provider_source_trait_config_value(component, "default", "subscriptions")
                {
                    lines.push(format!(
                        "{} {}",
                        emoji::INFO,
                        style(format!(
                            "Messaging NATS: Listening on the following subscriptions [{}]",
                            subscriptions.split(",").collect::<Vec<&str>>().join(", "),
                        ))
                        .bold()
                    ));
                }
            }
            _ => {}
        }
    }

    lines
}

struct RunDevLoopArgs<'a> {
    dev_session: &'a WashDevSession,
    nats_client: &'a async_nats_0_33::Client,
    ctl_client: &'a CtlClient,
    project_cfg: &'a ProjectConfig,
    lattice: &'a str,
    session_id: &'a str,
    manifest_output_dir: Option<&'a PathBuf>,
    previous_deps: &'a mut Option<ProjectDeps>,
}

/// Run one iteration of the development loop
async fn run_dev_loop(
    RunDevLoopArgs {
        dev_session,
        nats_client,
        ctl_client,
        project_cfg,
        lattice,
        session_id,
        manifest_output_dir,
        previous_deps,
    }: RunDevLoopArgs<'_>,
) -> Result<()> {
    // Build the project (equivalent to `wash build`)
    eprintln!(
        "{} {}",
        emoji::CONSTRUCTION_BARRIER,
        style("Building project...").bold(),
    );
    let artifact_path = build_project(project_cfg, Some(&SignConfig::default()))
        .await
        .context("failed to build project")?
        .canonicalize()
        .context("failed to canonicalize path")?;
    eprintln!(
        "✅ successfully built project at [{}]",
        artifact_path.display()
    );
    let component_ref = format!("file://{}", artifact_path.display());

    // After the project is built, we must ensure dependencies are set up and running
    let (resolve, world_id) =
        parse_project_wit(project_cfg).context("failed to parse WIT from project dir")?;

    // Pull implied dependencies from WIT
    let wit_implied_deps = discover_dependencies_from_wit(resolve, world_id)
        .context("failed to resolve dependent components")?;
    eprintln!(
        "Detected component dependencies: {:?}",
        wit_implied_deps
            .iter()
            .map(DependencySpec::name)
            .collect::<Vec<String>>()
    );
    let pkey =
        ProjectDependencyKey::from_project(&project_cfg.common.name, &project_cfg.common.path)
            .context("failed to build key for project")?;
    let mut current_project_deps = ProjectDeps::from_known_deps(pkey, wit_implied_deps)
        .context("failed to build project dependencies")?;

    // Pull and merge in implied dependencies from project-level wasmcloud.toml
    let implied_project_deps =
        ProjectDeps::from_project_config(project_cfg).with_context(|| {
            format!(
                "failed to discover project dependencies from config [{}]",
                project_cfg.common.path.display(),
            )
        })?;
    current_project_deps
        .merge_override(implied_project_deps)
        .context("failed to merge & override project-specified deps")?;

    // After we've merged, we can update the session ID to belong to this session
    current_project_deps.session_id = Some(session_id.into());

    // Generate component that represents the main Webassembly component/provider being developed
    let component_id = format!(
        "{}-{}",
        session_id,
        project_cfg.common.name.to_lowercase().replace(" ", "-"),
    );
    let component = generate_component_from_project_cfg(project_cfg, &component_id, &component_ref)
        .context("failed to generate app component")?;

    // If deps haven't changed, then we can simply restart the component and return
    if previous_deps
        .as_ref()
        .is_some_and(|deps| *deps == current_project_deps)
    {
        eprintln!(
            "{} {}",
            emoji::RECYCLE,
            style(format!(
                "(Fast-)Reloading component [{component_id}] (no dependencies have changed)..."
            ))
            .bold()
        );
        // Scale the component to zero, trusting that wadm will re-create it
        scale_down_component(
            ctl_client,
            project_cfg,
            &dev_session
                .host_data
                .as_ref()
                .context("missing host ID for session")?
                .0,
            &component_id,
            &component_ref,
        )
        .await
        .with_context(|| format!("failed to reload component [{component_id}]"))?;
        return Ok(());
    }

    current_project_deps.component = Some(component);

    // Convert the project deps into a fully-baked WADM manifests
    let manifests = current_project_deps
        .generate_wadm_manifests()
        .with_context(|| {
            format!("failed to generate a WADM manifest from (session [{session_id}])")
        })?;

    // Apply all manifests
    for manifest in manifests {
        // Generate all help text for this manifest
        let help_text_lines = generate_help_text_for_manifest(&manifest);

        let model_json =
            serde_json::to_string(&manifest).context("failed to convert manifest to JSON")?;

        // Write out manifests to local file if provided
        if let Some(output_dir) = manifest_output_dir {
            ensure!(
                tokio::fs::metadata(output_dir)
                    .await
                    .context("failed to get manifest output dir metadata")
                    .is_ok_and(|f| f.is_dir()),
                "manifest output directory [{}] must be a folder",
                output_dir.display()
            );
            tokio::fs::write(
                output_dir.join(format!("{}.yaml", manifest.metadata.name)),
                serde_yaml::to_string(&manifest).context("failed to convert manifest to YAML")?,
            )
            .await
            .with_context(|| {
                format!(
                    "failed to write out manifest YAML to output dir [{}]",
                    output_dir.display(),
                )
            })?
        }

        // Put the manifest
        match wash_lib::app::put_model(nats_client, Some(lattice.into()), &model_json).await {
            Ok(_) => {}
            Err(e) if e.to_string().contains("already exists") => {}
            Err(e) => {
                bail!("failed to put model [{}]: {e}", manifest.metadata.name);
            }
        }

        // Deploy the manifest
        deploy_model_from_manifest(
            nats_client,
            Some(lattice.into()),
            AppManifest::ModelName(manifest.metadata.name.clone()),
            None,
        )
        .await
        .context("failed to deploy manifest")?;

        eprintln!(
            "{} {}",
            emoji::RECYCLE,
            style(format!(
                "Deployed development manifest for application [{}]",
                manifest.metadata.name,
            ))
            .bold(),
        );

        // Print all help text lines
        eprintln!("{}", help_text_lines.join("\n"));
    }

    eprintln!(
        "{} {}",
        emoji::RECYCLE,
        style(format!("Reloading component [{component_id}]...")).bold()
    );
    // Scale the component to zero, trusting that wadm will re-create it
    scale_down_component(
        ctl_client,
        project_cfg,
        &dev_session
            .host_data
            .as_ref()
            .context("missing host ID for session")?
            .0,
        &component_id,
        &component_ref,
    )
    .await
    .with_context(|| format!("failed to reload component [{component_id}]"))?;

    // Update deps, since they must be different
    *previous_deps = Some(current_project_deps);

    Ok(())
}

/// Scale a component to zero
async fn scale_down_component(
    client: &CtlClient,
    project_cfg: &ProjectConfig,
    host_id: &str,
    component_id: &str,
    component_ref: &str,
) -> Result<()> {
    // Now that backing infrastructure has changed, we should scale the component
    // as the component (if it was running before) has *not* changed.
    //
    // Scale the WADM component (which can be either a component or provider) down,
    // expecting that WADM should restore it (and trigger a reload)
    match project_cfg.project_type {
        wash_lib::parser::TypeConfig::Component(_) => {
            scale_component(ScaleComponentArgs {
                client,
                host_id,
                component_id,
                component_ref,
                max_instances: 0,
                annotations: None,
                config: vec![],
                skip_wait: false,
                timeout_ms: None,
            })
            .await
            .with_context(|| {
                format!("failed to scale down component [{component_id}] for reload")
            })?;
        }
        wash_lib::parser::TypeConfig::Provider(_) => {
            if let Err(e) = stop_provider(
                client,
                Some(host_id),
                component_id,
                false,
                DEFAULT_PROVIDER_STOP_TIMEOUT_MS,
            )
            .await
            {
                eprintln!(
                    "{} Failed to stop provider component [{component_id}] during wash dev: {e}",
                    emoji::WARN,
                );
            }
        }
    }

    Ok(())
}

/// Handle `wash dev`
pub async fn handle_command(
    cmd: DevCommand,
    _output_kind: wash_lib::cli::OutputKind,
) -> Result<CommandOutput> {
    let current_dir = std::env::current_dir()?;
    let project_path = cmd.code_dir.unwrap_or(current_dir);
    let project_cfg = get_config(Some(project_path.clone()), Some(true))?;

    let mut wash_dev_session = WashDevSession::from_sessions_file(&project_path)
        .await
        .context("failed to build wash dev session")?;
    let session_id = wash_dev_session.id.clone();
    eprintln!("{} Resolved wash session ID [{session_id}]", emoji::INFO);

    let (mut nats_child, mut wadm_child, mut wasmcloud_child) = (None, None, None);

    // If there is not a running host for this session, then we can start one
    if wash_dev_session.host_data.is_none() {
        (nats_child, wadm_child, wasmcloud_child) = wash_dev_session
            .start_host(
                cmd.wasmcloud_opts.clone(),
                cmd.nats_opts.clone(),
                cmd.wadm_opts.clone(),
            )
            .await
            .with_context(|| format!("failed to start host for session [{session_id}]"))?;
    }
    let host_id = wash_dev_session
        .host_data
        .clone()
        .context("missing host_id, after ensuring host has started")?
        .0;

    // Create NATS and control interface client to use to connect
    let nats_client = nats_client_from_wasmcloud_opts(&cmd.wasmcloud_opts).await?;
    let ctl_client = Arc::new(
        cmd.wasmcloud_opts
            .clone()
            .into_ctl_client(None)
            .await
            .context("failed to create wasmcloud control client")?,
    );
    let lattice = &ctl_client.lattice;

    // See if the host is running by retrieving an inventory
    if let Err(_e) = ctl_client.get_host_inventory(&host_id).await {
        eprintln!(
            "{} Failed to retrieve inventory from host [{host_id}]... Is it running?",
            emoji::WARN
        );
        eprintln!(
            "{} {}",
            emoji::CONSTRUCTION_BARRIER,
            style(format!(
                "Starting host for wash dev session [{session_id}]...",
            ))
            .bold(),
        );
        (nats_child, wadm_child, wasmcloud_child) = wash_dev_session
            .start_host(
                cmd.wasmcloud_opts.clone(),
                cmd.nats_opts.clone(),
                cmd.wadm_opts.clone(),
            )
            .await
            .context("failed to start host for session")?;
    }

    // Set up a oneshot channel to perform graceful shutdown, handle Ctrl + c w/ tokio
    let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
    let (reload_tx, mut reload_rx) = mpsc::channel::<()>(1);
    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .context("failed to wait for ctrl_c signal")?;
        stop_tx
            .send(())
            .await
            .context("failed to send stop signal after receiving Ctrl + c")?;
        Result::<_, anyhow::Error>::Ok(())
    });

    // Enable/disable watching to prevent having the output artifact trigger a rebuild
    let pause_watch = Arc::new(AtomicBool::new(false));
    let watcher_paused = pause_watch.clone();

    // Spawn a file watcher to listen for changes and send on reload_tx
    let mut watcher = notify::recommended_watcher(move |res: _| match res {
        Ok(event) => match event {
            NotifyEvent {
                kind: EventKind::Create(_),
                ..
            }
            | NotifyEvent {
                kind: EventKind::Modify(ModifyKind::Data(_)),
                ..
            }
            | NotifyEvent {
                kind: EventKind::Remove(_),
                ..
            } => {
                // If watch has been paused for any reason, skip notifications
                if watcher_paused.load(Ordering::SeqCst) {
                    return;
                }

                let _ = reload_tx.blocking_send(());
            }
            _ => {}
        },
        Err(e) => {
            eprintln!("[error] watch failed: {:?}", e);
        }
    })?;
    watcher.watch(&project_path.clone(), RecursiveMode::Recursive)?;

    // Run the dev loop once (building the application, deploying deps, etc), before we start watching
    let mut dependencies = None;
    run_dev_loop(RunDevLoopArgs {
        dev_session: &wash_dev_session,
        nats_client: &nats_client,
        ctl_client: &ctl_client,
        project_cfg: &project_cfg,
        lattice,
        session_id: &session_id,
        manifest_output_dir: cmd.manifest_output_dir.as_ref(),
        previous_deps: &mut dependencies,
    })
    .await
    .context("failed to run initial dev loop iteration")?;

    // Watch FS for changes and listen for Ctrl + C in tandem
    eprintln!("👀 watching for file changes (press Ctrl+c to stop)...");
    loop {
        select! {
            // Process a file change/reload
            _ = reload_rx.recv() => {
                pause_watch.store(true, Ordering::SeqCst);
                run_dev_loop(RunDevLoopArgs {
                    dev_session: &wash_dev_session,
                    nats_client: &nats_client,
                    ctl_client: &ctl_client,
                    project_cfg: &project_cfg,
                    lattice,
                    session_id: &session_id,
                    manifest_output_dir: cmd.manifest_output_dir.as_ref(),
                    previous_deps: &mut dependencies,
                })
                    .await
                    .context("failed to run dev loop iteration")?;
                pause_watch.store(false, Ordering::SeqCst);
                eprintln!("\n👀 watching for file changes (press Ctrl+c to stop)...");
            },

            // Process a stop
            _ = stop_rx.recv() => {
                pause_watch.store(true, Ordering::SeqCst);
                eprintln!("🛑 received Ctrl + c, stopping devloop...");

                // Update the sessions file with the fact that this session stopped
                wash_dev_session.in_use = false;
                SessionMetadata::persist_session(&wash_dev_session).await?;

                // Delete manifests related to the application
                if let Some(dependencies) = dependencies {
                    eprintln!("🧹 Cleaning up deployed WADM application(s)...");
                    dependencies.delete_manifests(&nats_client, lattice).await?;
                }

                // Stop the host, unless explicitly instructed to leave host running
                if !cmd.leave_host_running {
                    eprintln!("⏳ stopping wasmCloud instance...");

                    // Stop host
                    if let Some(mut host) = wasmcloud_child {
                        host
                            .kill()
                            .await
                            .context("failed to stop wasmcloud process")?;
                    }

                    // Stop WADM
                    if let Some(mut wadm) = wadm_child {
                        wadm
                            .kill()
                            .await
                            .context("failed to stop wadm child process")?;
                        remove_wadm_pidfile(wash_dev_session.base_dir().await?)
                            .await
                            .context("failed to remove wadm pidfile")?;
                    }

                    // Stop NATS
                    if let Some(mut nats) = nats_child {
                        nats.kill().await?;
                    }
                }

                break Ok(CommandOutput::default());
            },
        }
    }
}
