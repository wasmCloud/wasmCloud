use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _, Result};
use semver::Version;
use tracing::debug;

use wadm_types::{
    CapabilityProperties, Component, ComponentProperties, ConfigProperty, LinkProperty, Manifest,
    Metadata, Properties, SecretProperty, Specification, TargetConfig, TraitProperty,
};
use wash_lib::generate::emoji;
use wash_lib::parser::{
    DevConfigSpec, DevSecretSpec, InterfaceComponentOverride, ProjectConfig, WitInterfaceSpec,
};
use wasmcloud_core::{
    parse_wit_meta_from_operation, LinkName, WitInterface, WitNamespace, WitPackage,
};

use super::manifest::config_name;
use super::{
    DEFAULT_BLOBSTORE_FS_PROVIDER_IMAGE, DEFAULT_BLOBSTORE_ROOT_DIR,
    DEFAULT_HTTP_CLIENT_PROVIDER_IMAGE, DEFAULT_HTTP_SERVER_PROVIDER_IMAGE,
    DEFAULT_INCOMING_HANDLER_ADDRESS, DEFAULT_KEYVALUE_BUCKET, DEFAULT_KEYVALUE_PROVIDER_IMAGE,
    DEFAULT_MESSAGING_HANDLER_SUBSCRIPTION, DEFAULT_MESSAGING_NATS_PROVIDER_IMAGE,
};

/// Check whether the provided interface is ignored for the purpose of building dependencies.
///
/// Dependencies are ignored normally if they are known-internal packages or interfaces that
/// are built into the host.
fn is_ignored_iface_dep(ns: &str, pkg: &str, iface: &str) -> bool {
    matches!(
        (ns, pkg, iface),
        ("wasi", "blobstore", "container" | "types")
            | ("wasi", "http", "types")
            | ("wasi", "runtime", "config")
            | (
                "wasi",
                "cli" | "clocks" | "filesystem" | "io" | "logging" | "random" | "sockets",
                _
            )
            | ("wasmcloud", "messaging", "types")
            | ("wasmcloud", "secrets" | "bus", _)
    )
}

/// Keys that index the list of dependencies in a [`ProjectDeps`]
///
/// # Examples
///
/// ```ignore
/// let project_key = ProjectDependencyKey::Project {
///   name: "http-hello-world".into(),
///   imports: vec![ ("wasi".into(), "http".into(), "incoming-handler".into(), None) ],
///   exports: vec![ ("wasi".into(), "http".into(), "incoming-handler".into(), None) ],
///   in_workspace: None,
/// };
///
/// let workspace_key = ProjectDependencyKey::Workspace; // alternatively ProjectDependencyKey::default()
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum ProjectDependencyKey {
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
    pub(crate) fn from_project(name: &str, project_dir: impl AsRef<Path>) -> Result<Self> {
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
pub(crate) enum DependencySpec {
    /// A dependency that receives invocations (ex. `keyvalue-nats` receiving a `wasi:keyvalue/get`)
    Exports(DependencySpecInner),
    /// A dependency that performs invocations (ex. `http-server` invoking a component's `wasi:http/incoming-handler` export)
    Imports(DependencySpecInner),
}

impl DependencySpec {
    /// Retrieve the name for this dependency
    pub(crate) fn name(&self) -> String {
        match self {
            DependencySpec::Exports(inner) => inner.name(),
            DependencySpec::Imports(inner) => inner.name(),
        }
    }

    /// Retrieve the image_ref for this dependency
    pub(crate) fn image_ref(&self) -> Option<&str> {
        match self {
            DependencySpec::Exports(inner) => inner.image_ref(),
            DependencySpec::Imports(inner) => inner.image_ref(),
        }
    }

    /// Retrieve whether this spec is a component or not
    ///
    /// Components must be especially noted because by default providers are expected
    /// to provide functionality, but it also possible for components to do so.
    pub(crate) fn is_component(&self) -> bool {
        match self {
            DependencySpec::Exports(inner) => inner.is_component(),
            DependencySpec::Imports(inner) => inner.is_component(),
        }
    }

    /// Retrieve configs for this component spec
    pub(crate) fn configs(&self) -> &Vec<ConfigProperty> {
        match self {
            DependencySpec::Exports(inner) => &inner.configs,
            DependencySpec::Imports(inner) => &inner.configs,
        }
    }

    /// Retrieve the wit for this dependency
    pub(crate) fn wit(&self) -> &WitInterfaceSpec {
        match self {
            DependencySpec::Exports(inner) => &inner.wit,
            DependencySpec::Imports(inner) => &inner.wit,
        }
    }

    /// Get the inner data of the dependency spec
    pub(crate) fn inner(&self) -> &DependencySpecInner {
        match self {
            DependencySpec::Exports(inner) => inner,
            DependencySpec::Imports(inner) => inner,
        }
    }

    /// Get the inner data of the dependency spec
    pub(crate) fn inner_mut(&mut self) -> &mut DependencySpecInner {
        match self {
            DependencySpec::Exports(inner) => inner,
            DependencySpec::Imports(inner) => inner,
        }
    }

    /// Add a configuration to the list of configurations on the dependency
    pub(crate) fn add_config(&mut self, cfg: impl Into<wadm_types::ConfigProperty>) {
        match self {
            DependencySpec::Exports(inner) | DependencySpec::Imports(inner) => {
                inner.configs.push(cfg.into())
            }
        }
    }

    /// Add one or more loose properties to an existing, creating it if it does not exist
    pub(crate) fn add_config_properties_to_existing(
        &mut self,
        name: impl AsRef<str>,
        new_props: impl Into<BTreeMap<String, String>>,
    ) {
        let name = name.as_ref();
        let new_props = new_props.into();
        match self {
            DependencySpec::Exports(inner) | DependencySpec::Imports(inner) => {
                match inner.configs.iter_mut().find(|c| c.name == name) {
                    Some(ConfigProperty {
                        ref mut properties, ..
                    }) => match properties {
                        Some(properties) => properties.extend(new_props),
                        None => {
                            *properties = Some(HashMap::from_iter(new_props));
                        }
                    },
                    None => self.add_config(wadm_types::ConfigProperty {
                        name: name.into(),
                        properties: Some(HashMap::from_iter(new_props)),
                    }),
                }
            }
        }
    }

    /// Retrieve secrets for this component spec
    pub(crate) fn secrets(&self) -> &Vec<SecretProperty> {
        match self {
            DependencySpec::Exports(inner) => &inner.secrets,
            DependencySpec::Imports(inner) => &inner.secrets,
        }
    }

    /// Add a secret to the list of secrets on the dependency
    pub(crate) fn add_secret(&mut self, cfg: impl Into<wadm_types::SecretProperty>) {
        match self {
            DependencySpec::Exports(inner) | DependencySpec::Imports(inner) => {
                inner.secrets.push(cfg.into())
            }
        }
    }

    /// Set the image reference for this dependency spec
    pub(crate) fn set_image_ref(&mut self, s: impl AsRef<str>) {
        match self {
            DependencySpec::Exports(DependencySpecInner { image_ref, .. }) => {
                image_ref.replace(s.as_ref().to_string());
            }
            DependencySpec::Imports(DependencySpecInner { image_ref, .. }) => {
                image_ref.replace(s.as_ref().to_string());
            }
        }
    }

    /// Set the image reference for this dependency spec
    pub(crate) fn set_link_name(&mut self, s: impl AsRef<str>) {
        match self {
            DependencySpec::Exports(DependencySpecInner { link_name, .. }) => {
                *link_name = s.as_ref().to_string();
            }
            DependencySpec::Imports(DependencySpecInner { link_name, .. }) => {
                *link_name = s.as_ref().to_string();
            }
        }
    }

    /// Override the contents of this dependency spec with another
    pub(crate) fn override_with(&mut self, other: &Self) {
        self.inner_mut().override_with(other.inner())
    }

    /// Derive which local component should be used given a WIT interface to be satisified
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let v = from_wit_import_face("wasi:keyvalue/atomics");
    /// # assert!(v.is_some())
    /// ```
    pub(crate) fn from_wit_import_iface(iface: &str) -> Option<Self> {
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
                    wit: WitInterfaceSpec {
                        namespace: ns,
                        package: pkg,
                        interface: Some(iface),
                        function: None,
                        version,
                    },
                    delegated_to_workspace: false,
                    link_name: "default".into(),
                    image_ref: Some(DEFAULT_KEYVALUE_PROVIDER_IMAGE.into()),
                    is_component: false,
                    configs: Default::default(),
                    secrets: Default::default(),
                }))
            }
            ("wasi", "http", "outgoing-handler") => Some(Self::Exports(DependencySpecInner {
                wit: WitInterfaceSpec {
                    namespace: ns,
                    package: pkg,
                    interface: Some(iface),
                    function: None,
                    version,
                },
                delegated_to_workspace: false,
                link_name: "default".into(),
                image_ref: Some(DEFAULT_HTTP_CLIENT_PROVIDER_IMAGE.into()),
                is_component: false,
                configs: Default::default(),
                secrets: Default::default(),
            })),
            ("wasi", "blobstore", "blobstore") | ("wrpc", "blobstore", "blobstore") => {
                Some(Self::Exports(DependencySpecInner {
                    wit: WitInterfaceSpec {
                        namespace: ns,
                        package: pkg,
                        interface: Some(iface),
                        function: None,
                        version,
                    },
                    delegated_to_workspace: false,
                    link_name: "default".into(),
                    image_ref: Some(DEFAULT_BLOBSTORE_FS_PROVIDER_IMAGE.into()),
                    is_component: false,
                    configs: Default::default(),
                    secrets: Default::default(),
                }))
            }
            ("wasmcloud", "messaging", "consumer") => Some(Self::Exports(DependencySpecInner {
                wit: WitInterfaceSpec {
                    namespace: ns,
                    package: pkg,
                    interface: Some(iface),
                    function: None,
                    version,
                },
                delegated_to_workspace: false,
                link_name: "default".into(),
                image_ref: Some(DEFAULT_MESSAGING_NATS_PROVIDER_IMAGE.into()),
                is_component: false,
                configs: Default::default(),
                secrets: Default::default(),
            })),
            // Treat all other dependencies as custom, and track them as dependencies,
            // though they cannot be resolved to a proper dependency without an explicit override/
            // other configuration method
            _ => Some(Self::Exports(DependencySpecInner {
                wit: WitInterfaceSpec {
                    namespace: ns,
                    package: pkg,
                    interface: Some(iface),
                    function: None,
                    version,
                },
                delegated_to_workspace: false,
                link_name: "default".into(),
                image_ref: None,
                is_component: false,
                configs: Default::default(),
                secrets: Default::default(),
            })),
        }
    }

    /// Derive which local component should be used given a WIT interface to be satisified
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let v = from_wit_export_face("wasi:http/incoming-handler");
    /// # assert!(v.is_some())
    /// ```
    pub(crate) fn from_wit_export_iface(iface: &str) -> Option<Self> {
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
                wit: WitInterfaceSpec {
                    namespace: ns,
                    package: pkg,
                    interface: Some(iface),
                    function: None,
                    version,
                },
                delegated_to_workspace: false,
                link_name: "default".into(),
                image_ref: Some(DEFAULT_HTTP_SERVER_PROVIDER_IMAGE.into()),
                is_component: false,
                configs: Default::default(),
                secrets: Default::default(),
            })),
            ("wasmcloud", "messaging", "handler") => Some(Self::Imports(DependencySpecInner {
                wit: WitInterfaceSpec {
                    namespace: ns,
                    package: pkg,
                    interface: Some(iface),
                    function: None,
                    version,
                },
                delegated_to_workspace: false,
                link_name: "default".into(),
                image_ref: Some(DEFAULT_MESSAGING_NATS_PROVIDER_IMAGE.into()),
                is_component: false,
                configs: Default::default(),
                secrets: Default::default(),
            })),
            // Treat all other dependencies as custom, and track them as dependencies,
            // though they cannot be resolved to a proper dependency without an explicit override/
            // other configuration method
            _ => Some(Self::Imports(DependencySpecInner {
                wit: WitInterfaceSpec {
                    namespace: ns,
                    package: pkg,
                    interface: Some(iface),
                    function: None,
                    version,
                },
                delegated_to_workspace: false,
                link_name: "default".into(),
                image_ref: None,
                is_component: false,
                configs: Default::default(),
                secrets: Default::default(),
            })),
        }
    }

    pub(crate) fn generate_properties(&self, name: &str) -> Result<Properties> {
        let properties = match self.image_ref() {
            Some(
                DEFAULT_HTTP_CLIENT_PROVIDER_IMAGE
                | DEFAULT_HTTP_SERVER_PROVIDER_IMAGE
                | DEFAULT_BLOBSTORE_FS_PROVIDER_IMAGE
                | DEFAULT_MESSAGING_NATS_PROVIDER_IMAGE
                | DEFAULT_KEYVALUE_PROVIDER_IMAGE,
            ) => Properties::Capability {
                properties: CapabilityProperties {
                    image: Some(
                        self.image_ref()
                            .with_context(|| {
                                format!(
                                "missing image ref for generated (known) component dependency [{}]",
                                name,
                            )
                            })?
                            .into(),
                    ),
                    application: None,
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
                            image: Some(
                                self.image_ref()
                                    .with_context(|| {
                                        format!(
                                        "missing image ref for generated component dependency [{}]",
                                        self.name()
                                    )
                                    })?
                                    .into(),
                            ),
                            application: None,
                            id: None,
                            config: self.configs().clone(),
                            secrets: self.secrets().clone(),
                        },
                    }
                } else {
                    Properties::Capability {
                        properties: CapabilityProperties {
                            image: Some(
                                self.image_ref()
                                    .with_context(|| {
                                        format!(
                                        "missing image ref for generated provider dependency [{}]",
                                        self.name()
                                    )
                                    })?
                                    .into(),
                            ),
                            application: None,
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
    fn generate_dep_component(&self, suffix: &str) -> Result<Component> {
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

/// Specification of a dependency (possibly implied)
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DependencySpecInner {
    /// Specification of the WIT interface that this dependency fulfills.
    ///
    /// Note that this specification may cover *more than one* interface.
    pub(crate) wit: WitInterfaceSpec,

    /// Whether this dependency should delegated to the workspace
    pub(crate) delegated_to_workspace: bool,

    /// Image reference to the component that should be inserted/used
    ///
    /// This reference *can* be missing if an override is specified with no image,
    /// which can happen in at least two cases:
    /// - custom WIT-defined interface imports/exports (we may not know at WIT processing what their overrides will be)
    /// - project-level with workspace delegation  (we may not know what the image ref is at the project level)
    pub(crate) image_ref: Option<String>,

    /// Whether this dependency represents a WebAssembly component, rather than a (standalone binary) provider
    pub(crate) is_component: bool,

    /// The link name this dependency should be connected over
    ///
    /// In the vast majority of cases, this will be "default", but it may not be
    /// if the same interface is used to link to multiple different providers/components
    pub(crate) link_name: LinkName,

    /// Configurations that must be created and/or consumed by this dependency
    pub(crate) configs: Vec<wadm_types::ConfigProperty>,

    /// Secrets that must be created and/or consumed by this dependency
    ///
    /// [`SecretProperty`] here support a special `policy` value which is 'env'.
    /// Paired with a key that looks like "$SOME_VALUE", the value will be extracted from ENV *prior* and
    pub(crate) secrets: Vec<wadm_types::SecretProperty>,
}

impl DependencySpecInner {
    /// Retrieve the name for this dependency
    pub(crate) fn name(&self) -> String {
        match self.image_ref.as_deref() {
            Some(DEFAULT_KEYVALUE_PROVIDER_IMAGE) => "keyvalue-nats".into(),
            Some(DEFAULT_HTTP_CLIENT_PROVIDER_IMAGE) => "http-client".into(),
            Some(DEFAULT_HTTP_SERVER_PROVIDER_IMAGE) => "http-server".into(),
            Some(DEFAULT_BLOBSTORE_FS_PROVIDER_IMAGE) => "blobstore-fs".into(),
            Some(DEFAULT_MESSAGING_NATS_PROVIDER_IMAGE) => "messaging-nats".into(),
            _ => format!(
                "custom-{}-{}-{}",
                self.wit.namespace,
                self.wit.package,
                self.wit.interface.as_deref().unwrap_or("*"),
            ),
        }
    }

    /// Retrieve the image reference for this dependency
    pub(crate) fn image_ref(&self) -> Option<&str> {
        self.image_ref.as_deref()
    }

    /// Retrieve whether this dependency spec is a component
    pub(crate) fn is_component(&self) -> bool {
        self.is_component
    }

    /// Override the contents of this dependency spec with another
    pub(crate) fn override_with(&mut self, other: &Self) {
        self.is_component = other.is_component;
        self.delegated_to_workspace = other.delegated_to_workspace;
        self.image_ref = other.image_ref.clone();
        self.link_name = other.link_name.clone();
        // NOTE: We depend on the fact that configs and secrets should be processed in order,
        // with later entries *overriding* earlier ones
        self.configs.extend(self.configs.clone());
        // NOTE: we depend on the
        self.secrets.extend(self.secrets.clone());
    }
}

/// Information related to the dependencies of a given project
///
/// Projects can either be inside workspaces, or not (single component/provider).
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectDeps {
    /// ID of a session
    ///
    /// This is normally used when generating sessions to be used with `wash dev`, in order
    /// to make sure various dependencies and related files can be uniquely identified.
    pub(crate) session_id: Option<String>,

    /// Lookup of dependencies by project key, with lookups into the pool
    pub(crate) dependencies: BTreeMap<ProjectDependencyKey, Vec<DependencySpec>>,

    /// The component to which dependencies belong
    ///
    /// When used in the context of `wash dev` this is the component that is being developed
    /// (either a provider or a component).
    pub(crate) component: Option<Component>,

    /// Dependencies that receive invocations for given interfaces (i.e. `keyvalue-nats` receiving a `wasi:keyvalue/get`)
    ///
    /// The keys to this structure are indices into the `dependencies` member.
    pub(crate) exporters:
        BTreeMap<(WitNamespace, WitPackage, WitInterface, Option<Version>), Vec<usize>>,

    /// Dependencies that perform invocations for given interfaces (i.e. `http-server` invoking a component's `wasi:http/incoming-handler` export)
    pub(crate) importers:
        BTreeMap<(WitNamespace, WitPackage, WitInterface, Option<Version>), Vec<usize>>,
}

impl ProjectDeps {
    /// Build a [`ProjectDeps`] for a project/workspace entirely from [`DependencySpec`]s
    pub(crate) fn from_known_deps(
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
    pub(crate) fn from_project_config_overrides(
        pkey: ProjectDependencyKey,
        cfg: &ProjectConfig,
    ) -> Result<Self> {
        // If no overrides were present, we can return immediately
        let imports = &cfg.dev.overrides.imports;
        let exports = &cfg.dev.overrides.exports;
        if imports.is_empty() && exports.is_empty() {
            return Ok(Self::default());
        }

        // Build full list of overrides with generated dep specs
        let mut overrides_with_deps = Vec::with_capacity(imports.len() + exports.len());
        overrides_with_deps.append(
            &mut imports
                .iter()
                .map(|v| {
                    DependencySpec::from_wit_import_iface(&v.interface_spec)
                        .context("failed to build image ref from interface")
                        .map(|dep| (v, dep))
                })
                .collect::<Result<Vec<_>>>()?,
        );
        overrides_with_deps.append(
            &mut exports
                .iter()
                .map(|v| {
                    DependencySpec::from_wit_export_iface(&v.interface_spec)
                        .context("failed to build image ref from interface")
                        .map(|dep| (v, dep))
                })
                .collect::<Result<Vec<_>>>()?,
        );

        // Build a list of the final modified dep specs
        let mut deps = Vec::new();
        for (
            InterfaceComponentOverride {
                config,
                secrets,
                image_ref,
                link_name,
                ..
            },
            mut dep_spec,
        ) in overrides_with_deps.drain(..)
        {
            if let Some(image_ref) = image_ref {
                dep_spec.set_image_ref(image_ref);
            }

            if let Some(link_name) = link_name {
                dep_spec.set_link_name(link_name);
            }

            if let Some(config) = config {
                for config in config.iter() {
                    match config {
                        DevConfigSpec::Named { name } => {
                            dep_spec.add_config(wadm_types::ConfigProperty {
                                name: name.into(),
                                properties: None,
                            })
                        }
                        DevConfigSpec::Values { values } => {
                            dep_spec.add_config_properties_to_existing("default", values.clone())
                        }
                    }
                }
            }

            if let Some(secrets) = secrets {
                for secret in secrets.iter() {
                    match secret {
                        DevSecretSpec::Existing { name, source } => {
                            dep_spec.add_secret(wadm_types::SecretProperty {
                                name: name.into(),
                                properties: source.clone(),
                            });
                        }
                        DevSecretSpec::Values { .. } => {
                            bail!("overriding secret with a on-demand secret is not supported yet")
                        }
                    }
                }
            }

            deps.push(dep_spec)
        }

        Self::from_known_deps(pkey, deps)
    }

    /// Add one or more [`DependencySpec`]s to the current [`ProjectDeps`]
    ///
    /// To add a known dependency to this list of project dependencies, we must know *which* project
    /// the dependency belongs to, or whether it corresponds to the workspace.
    pub(crate) fn add_known_deps(
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
    pub(crate) fn merge_override(&mut self, other: Self) -> Result<()> {
        // ProjectDeps should have matching sessions, if specified
        if self.session_id.is_some()
            && other.session_id.is_some()
            && self.session_id != other.session_id
        {
            bail!(
                "session IDs (if specified) must match for merging deps. Session ID [{}] does not match [{}]",
                self.session_id.as_deref().unwrap_or("<none>"),
                other.session_id.as_deref().unwrap_or("<none>"),
            );
        }

        // Merge dependencies from the other bundle into this one
        for (pkey, other_deps) in other.dependencies {
            let existing_deps = self.dependencies.entry(pkey).or_default();

            // For every dep in other, find existing deps that overlap (i.e. are *not* disjoint)
            for other_dep in other_deps {
                let mut converted = Vec::with_capacity(existing_deps.len());
                let (mut rest, overlapping): (Vec<_>, Vec<_>) = existing_deps
                    .iter_mut()
                    .partition(|d| other_dep.wit().is_disjoint(d.wit()));
                // All overlapping dep specs in self are overridden with the overlapping values in other
                converted.append(&mut rest);
                for dep in overlapping {
                    dep.override_with(&other_dep);
                    converted.push(dep)
                }
            }
        }

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
    pub(crate) fn generate_wadm_manifests(&self) -> Result<impl IntoIterator<Item = Manifest>> {
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

        // Generate components for all the dependencies, using a map from component name to component
        // to remove duplicates
        let mut components = HashMap::new();

        // For each dependency, go through and generate the component along with necessary links
        for dep in self.dependencies.values().flatten() {
            let dep = dep.clone();
            // If a dependency could not be generated into a component, skip it
            let Ok(mut dep_component) = dep
                .generate_dep_component(session_id)
                .context("failed to generate component")
            else {
                eprintln!(
                    "{} Failed to generate component for dep [{}]",
                    emoji::WARN,
                    dep.name()
                );
                continue;
            };

            // Generate links for the given component and its spec, for known interfaces
            match dep {
                DependencySpec::Exports(DependencySpecInner {
                    wit:
                        WitInterfaceSpec {
                            namespace,
                            package,
                            interface,
                            version,
                            ..
                        },
                    ..
                }) => {
                    // Check to see if this link (namespace, package, target) already exists,
                    // and if so, add the interface to the existing link
                    if component
                        .traits
                        .get_or_insert(Vec::new())
                        .iter_mut()
                        .any(|trt| {
                            if let TraitProperty::Link(link) = &mut trt.properties {
                                if link.namespace == namespace
                                    && link.package == package
                                    && link.target.name == dep_component.name
                                {
                                    if let Some(interface) = &interface {
                                        link.interfaces.push(interface.into());
                                    };
                                    return true;
                                }
                            }
                            false
                        })
                    {
                        continue;
                    }

                    // Build the relevant app->dep link trait
                    let mut link_property = LinkProperty {
                        namespace: namespace.clone(),
                        package: package.clone(),
                        interfaces: interface
                            .as_ref()
                            .map(|iface| vec![iface.into()])
                            .unwrap_or_default(),
                        target: TargetConfig {
                            name: dep_component.name.clone(),
                            ..Default::default()
                        },
                        ..Default::default()
                    };

                    // Make interface-specific changes
                    match (
                        namespace.as_ref(),
                        package.as_ref(),
                        interface.as_deref(),
                        version,
                    ) {
                        ("wasi", "blobstore", Some("blobstore"), _)
                        | ("wrpc", "blobstore", Some("blobstore"), _) => {
                            link_property.target.config.push(ConfigProperty {
                                name: config_name(namespace.as_str(), package.as_str()),
                                properties: Some(HashMap::from([(
                                    "root".into(),
                                    DEFAULT_BLOBSTORE_ROOT_DIR.into(),
                                )])),
                            });
                        }
                        ("wasi", "keyvalue", Some("atomics" | "store" | "batch"), _) => {
                            link_property.target.config.push(ConfigProperty {
                                name: config_name(namespace.as_str(), package.as_str()),
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

                    // Add the trait to the app component
                    component.traits.get_or_insert(Vec::new()).push(link_trait);
                }
                DependencySpec::Imports(DependencySpecInner {
                    wit:
                        WitInterfaceSpec {
                            namespace,
                            package,
                            interface,
                            version,
                            ..
                        },
                    ..
                }) => {
                    // Build the relevant dep->app link trait
                    let mut link_property = LinkProperty {
                        namespace: namespace.clone(),
                        package: package.clone(),
                        interfaces: interface
                            .as_ref()
                            .map(|iface| vec![iface.into()])
                            .unwrap_or_default(),
                        target: TargetConfig {
                            name: component.name.clone(),
                            ..Default::default()
                        },
                        ..Default::default()
                    };

                    // Make interface-specific tweaks to the generated trait
                    match (
                        namespace.as_ref(),
                        package.as_ref(),
                        interface.as_deref(),
                        version,
                    ) {
                        ("wasi", "http", Some("incoming-handler"), _) => {
                            link_property
                                .source
                                .get_or_insert(Default::default())
                                .config
                                .push(ConfigProperty {
                                    name: config_name(namespace.as_str(), package.as_str()),
                                    properties: Some(HashMap::from([(
                                        "address".into(),
                                        DEFAULT_INCOMING_HANDLER_ADDRESS.into(),
                                    )])),
                                });
                        }
                        ("wasmcloud", "messaging", Some("handler"), _) => {
                            link_property
                                .source
                                .get_or_insert(Default::default())
                                .config
                                .push(ConfigProperty {
                                    name: config_name(namespace.as_str(), package.as_str()),
                                    properties: Some(HashMap::from([(
                                        "subscriptions".into(),
                                        DEFAULT_MESSAGING_HANDLER_SUBSCRIPTION.into(),
                                    )])),
                                });
                        }
                        _ => {}
                    }

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
            if let Some(c) = components.insert(dep_component.name.clone(), dep_component) {
                debug!("replacing duplicate component [{}]", c.name);
            }
        }

        // Add the application component after we've made necessary links
        if let Some(c) = components.insert(component.name.clone(), component) {
            debug!("replacing duplicate component [{}]", c.name);
        }

        manifests.push(Manifest {
            api_version: "core.oam.dev/v1beta1".into(),
            kind: "Application".into(),
            metadata: Metadata {
                name: app_name,
                // NOTE(brooksmtownsend): We don't include the version annotation here to ensure that
                // subsequent put/deploys of the application won't conflict.
                // NOTE(wadm#466): Don't leave this empty for now.
                annotations: BTreeMap::from([(
                    "wasmcloud.dev/session-id".into(),
                    session_id.into(),
                )]),
                labels: BTreeMap::from([(
                    "wasmcloud.dev/generated-by".into(),
                    format!(
                        "wash-dev{}",
                        std::env::var("CARGO_PKG_VERSION")
                            .map(|s| format!("-{}", s))
                            .unwrap_or_default()
                    ),
                )]),
            },
            spec: Specification {
                components: components.into_values().collect(),
                policies: Vec::with_capacity(0),
            },
        });

        Ok(manifests)
    }

    /// Delete all manifests associated with this [`ProjectDeps`]
    pub(crate) async fn delete_manifests(
        &self,
        client: &async_nats::Client,
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
                None,
            )
            .await
            .with_context(|| {
                format!("failed to delete application [{}]", manifest.metadata.name)
            })?;
        }

        Ok(())
    }
}
