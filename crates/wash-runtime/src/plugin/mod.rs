//! Plugin system for extending host capabilities.
//!
//! This module provides the plugin framework that allows the wasmcloud host
//! to support different WASI interfaces and capabilities. Plugins implement
//! specific functionality that components can use through standard interfaces.
//!
//! # Plugin Architecture
//!
//! Plugins are Rust types that implement the [`HostPlugin`] trait. They:
//! - Declare which WIT interfaces they provide via [`HostPlugin::world`]
//! - Bind to components that need their capabilities via [`HostPlugin::bind_component`]
//! - Can participate in workload lifecycle events
//! - Are automatically linked into the wasmtime runtime
//!
//! # Built-in Plugins
//!
//! The crate provides several built-in plugins for common WASI interfaces:
//! - [`wasi_http`] - HTTP server capabilities (`wasi:http/incoming-handler`)
//! - [`wasi_config`] - Runtime configuration (`wasi:config/store`)
//! - [`wasi_blobstore`] - Object storage (`wasi:blobstore`)
//! - [`wasi_keyvalue`] - Key-value storage (`wasi:keyvalue`)
//! - [`wasi_logging`] - Structured logging (`wasi:logging`)
//! - [`wasi_otel`] - OpenTelemetry tracing, metrics, and logs (`wasi:otel/*`)
//! - [`wasmcloud_secrets`] - Secrets delivery from bind-time config (`wasmcloud:secrets`)

use std::collections::HashMap;
use std::future::Future;
#[cfg(any(feature = "wasi-blobstore", feature = "wasi-keyvalue"))]
use std::path::{Path, PathBuf};

use crate::engine::workload::WorkloadItem;
use crate::{
    engine::workload::{ResolvedWorkload, UnresolvedWorkload, WorkloadMetadata},
    wit::{WitInterface, WitWorld},
};

#[cfg(feature = "wasi-config")]
pub mod wasi_config;

#[cfg(feature = "wasi-blobstore")]
pub mod wasi_blobstore;

#[cfg(feature = "wasi-keyvalue")]
pub mod wasi_keyvalue;

#[cfg(feature = "wasi-logging")]
pub mod wasi_logging;

#[cfg(all(feature = "wasmcloud-postgres", not(doctest)))]
pub mod wasmcloud_postgres;

#[cfg(feature = "wasi-otel")]
pub mod wasi_otel;

pub mod wasmcloud_messaging;

pub mod wasmcloud_secrets;

/// NATS-native capability: core pub/sub, JetStream, and KV.
#[cfg(feature = "wasmcloud-nats")]
pub mod wasmcloud_nats;

/// Host capabilities provided by a WebAssembly component running in its own
/// supervised store (rather than by a Rust plugin running in-store). Needs
/// `oci` for the loader that fetches a plugin's wasm.
#[cfg(all(feature = "host-component-plugins", feature = "oci"))]
pub mod component_host;

/// Declarative spec for a host component plugin (id + wasm source). Compiled
/// whenever the sources it can name are reachable, independent of
/// `host-component-plugins`, so front-ends can accept a plugin declaration and
/// fail clearly when built without that feature; the loader that consumes it
/// lives in [`component_host`].
#[cfg(feature = "oci")]
pub mod component_plugin_spec;
#[cfg(feature = "oci")]
pub use component_plugin_spec::ComponentPluginSpec;

/// Every plugin id this codebase has, independent of which cargo features a
/// given build enabled.
///
/// The roster exists to tell two conditions apart, because only one of them
/// should refuse to start: an id that is not here at all is a typo, and may be
/// shadowing a real plugin the operator meant to constrain — an inert
/// `workloadConfig: deny` is exactly the shape that rule guards against. An id
/// that *is* here but was not compiled in is a chart/binary skew, and refusing
/// there buys nothing: if the plugin is absent, no workload can bind it either,
/// so the declaration denies nothing and protects nothing.
///
/// Native ids only. A component plugin's id is whatever the operator wrote on
/// the entry that loads it, so it cannot be enumerated here — and does not need
/// to be, since a host that loaded one has it registered and never reaches the
/// roster.
///
/// Keep it in sync when a plugin id is added. A missing entry only downgrades a
/// warning to a refusal, and only on a host that was not built with the plugin,
/// so the failure is loud rather than silent.
pub const KNOWN_PLUGIN_IDS: &[&str] = &[
    "wasi-blobstore",
    "wasi-blobstore-multiplexed",
    "wasi-config",
    "wasi-keyvalue",
    "wasi-keyvalue-multiplexed",
    "wasi-logging",
    "wasi-otel",
    "wasi-webgpu",
    "wasmcloud-blobstore-multiplexed",
    "wasmcloud-keyvalue-multiplexed",
    "wasmcloud-messaging",
    "wasmcloud-messaging-async-multiplexed",
    "wasmcloud-messaging-memory",
    "wasmcloud-messaging-multiplexed",
    "wasmcloud-nats",
    "wasmcloud-postgres",
    "wasmcloud-secrets",
];

#[cfg(test)]
mod roster_tests {
    /// Every native plugin's id, from the constant the plugin itself uses. A
    /// plugin added without its roster entry gets a refusal instead of a
    /// warning on any build that did not compile it in.
    #[test]
    fn the_roster_names_every_native_plugin() {
        #[allow(unused_mut)]
        let mut ids: Vec<&str> = vec![
            super::wasmcloud_messaging::nats::PLUGIN_MESSAGING_ID,
            super::wasmcloud_messaging::in_memory::PLUGIN_MESSAGING_MEMORY_ID,
            super::wasmcloud_secrets::WASMCLOUD_SECRETS_ID,
        ];
        // Every multiplexer carries the same gate as its module: the plugin's
        // own feature does not compile one in.
        #[cfg(feature = "wasm_component_model_implements")]
        ids.extend([
            super::wasmcloud_messaging::multiplexed::MULTIPLEXED_MESSAGING_ID,
            super::wasmcloud_messaging::multiplexed_async::MULTIPLEXED_ASYNC_MESSAGING_ID,
        ]);
        #[cfg(feature = "wasi-blobstore")]
        ids.push(super::wasi_blobstore::nats::PLUGIN_BLOBSTORE_ID);
        #[cfg(all(
            feature = "wasi-blobstore",
            feature = "wasm_component_model_implements"
        ))]
        ids.extend([
            super::wasi_blobstore::multiplexed::MULTIPLEXED_BLOBSTORE_ID,
            super::wasi_blobstore::multiplexed_async::MULTIPLEXED_ASYNC_BLOBSTORE_ID,
        ]);
        #[cfg(feature = "wasi-keyvalue")]
        ids.push(super::wasi_keyvalue::nats::PLUGIN_KEYVALUE_ID);
        #[cfg(all(feature = "wasi-keyvalue", feature = "wasm_component_model_implements"))]
        ids.extend([
            super::wasi_keyvalue::multiplexed::MULTIPLEXED_KEYVALUE_ID,
            super::wasi_keyvalue::multiplexed_async::MULTIPLEXED_ASYNC_KEYVALUE_ID,
        ]);
        #[cfg(feature = "wasi-config")]
        ids.push(super::wasi_config::WASI_CONFIG_ID);
        #[cfg(feature = "wasi-logging")]
        ids.push(super::wasi_logging::PLUGIN_LOGGING_ID);
        #[cfg(feature = "wasi-otel")]
        ids.push(super::wasi_otel::WASI_OTEL_ID);
        // Same gate as the module: the feature alone does not compile it in.
        #[cfg(all(
            feature = "wasi-webgpu",
            not(target_os = "windows"),
            not(target_arch = "s390x")
        ))]
        ids.push(super::wasi_webgpu::WASI_WEBGPU_ID);
        #[cfg(all(feature = "wasmcloud-postgres", not(doctest)))]
        ids.push(super::wasmcloud_postgres::PLUGIN_POSTGRES_ID);
        #[cfg(feature = "wasmcloud-nats")]
        ids.push(super::wasmcloud_nats::PLUGIN_NATS_ID);

        let missing: Vec<&str> = ids
            .into_iter()
            .filter(|id| !super::KNOWN_PLUGIN_IDS.contains(id))
            .collect();
        assert!(
            missing.is_empty(),
            "missing from KNOWN_PLUGIN_IDS: {missing:?}"
        );
    }
}

/// Operator-declared bindings: the host's side of an
/// `interface-binding{name, config}`, and the `workloadConfig` policy that
/// decides whether a workload may write over it.
pub mod bindings;
pub use bindings::{
    BindingSchema, KeyOwnership, PluginBindingSet, PluginBindings, WorkloadConfigPolicy,
};

/// Shared `(implements ..)` multiplexing core
#[cfg(feature = "wasm_component_model_implements")]
pub mod multiplex;

/// Shared bounded `stream<u8>` collection for host impls whose backend takes a
/// complete payload.
pub(crate) mod stream_collect;

#[cfg(all(
    feature = "wasi-webgpu",
    not(target_os = "windows"),
    not(target_arch = "s390x")
))]
pub mod wasi_webgpu;

/// A wrapper around a [`std::collections::HashSet`] of [`crate::wit::WitInterface`] that provides convenience methods for looking up interfaces by namespace, package, and set of interfaces.
#[derive(Debug)]
pub struct WitInterfaces<'a> {
    inner: &'a std::collections::HashSet<crate::wit::WitInterface>,
}

impl<'a> WitInterfaces<'a> {
    /// Wraps a borrowed set of [`crate::wit::WitInterface`]s.
    pub fn new(inner: &'a std::collections::HashSet<crate::wit::WitInterface>) -> Self {
        Self { inner }
    }

    /// Returns an iterator over the wrapped [`crate::wit::WitInterface`]s.
    pub fn iter(&self) -> impl Iterator<Item = &'a crate::wit::WitInterface> + 'a {
        self.inner.iter()
    }

    /// Every entry matching the given namespace, package, and set of
    /// interfaces, ordered by [`entry_order`].
    ///
    /// A workload may declare one package across several entries, and all of
    /// them were matched to this plugin. What a plugin leaves unread here is
    /// config a workload declared and no component ever sees, so a plugin
    /// serving one instance folds the whole set rather than picking from it.
    pub fn matching(
        &self,
        namespace: &str,
        package: &str,
        interfaces: &[&str],
    ) -> Vec<&'a crate::wit::WitInterface> {
        let mut matched: Vec<&crate::wit::WitInterface> = self
            .inner
            .iter()
            .filter(|interface| entry_matches(interface, namespace, package, interfaces))
            .collect();
        matched.sort_by_cached_key(|interface| entry_order(interface));
        matched
    }

    /// The first entry matching the given namespace, package, and set of
    /// interfaces — the unnamed entry, a package's default route, when there is
    /// one.
    ///
    /// For a plugin that reads a single entry; one that must see everything a
    /// workload declared for its package wants [`Self::matching`].
    pub fn get(
        &self,
        namespace: &str,
        package: &str,
        interfaces: &[&str],
    ) -> Option<&'a crate::wit::WitInterface> {
        self.inner
            .iter()
            .filter(|interface| entry_matches(interface, namespace, package, interfaces))
            .min_by_key(|interface| entry_order(interface))
    }

    /// Returns `true` if the given namespace, package, and set of interfaces are contained within this collection of [`crate::wit::WitInterface`]s.
    pub fn contains(&self, namespace: &str, package: &str, interfaces: &[&str]) -> bool {
        self.inner
            .iter()
            .any(|interface| entry_matches(interface, namespace, package, interfaces))
    }
}

/// Whether one entry answers for `namespace:package` and covers `interfaces`.
///
/// An entry naming no interfaces covers its whole package, the rule
/// [`crate::wit::WitWorld::uses`] matched it to the plugin under. Reading it
/// more strictly here leaves the plugin bound and its interface unserved.
fn entry_matches(
    interface: &crate::wit::WitInterface,
    namespace: &str,
    package: &str,
    interfaces: &[&str],
) -> bool {
    interface.namespace == namespace
        && interface.package == package
        && (interface.interfaces.is_empty()
            || interfaces.iter().all(|i| interface.interfaces.contains(*i)))
}

/// Orders the entries of one package: the unnamed entry — a package's default
/// route — first, then by name, instance, interfaces, and the keys each entry
/// configures. Two entries alike in all of those carry the same resolved
/// config, so their order between themselves does not change what a plugin
/// reads.
///
/// Keys, never values: once bindings resolve, an entry's config carries
/// whatever `secretFrom` produced.
fn entry_order(interface: &crate::wit::WitInterface) -> (Option<&str>, String, String, String) {
    let sorted = |mut items: Vec<&str>| {
        items.sort_unstable();
        items.join(",")
    };
    (
        // `None` sorts before `Some`, putting the unnamed entry first.
        interface.name.as_deref(),
        interface.instance(),
        sorted(interface.interfaces.iter().map(String::as_str).collect()),
        sorted(interface.config.keys().map(String::as_str).collect()),
    )
}

/// A workload a plugin has failed out of band (after it was already running),
/// delivered to the host so it can transition the workload to a failed state.
/// Used when a host component plugin evicts a workload whose `on-workload-bind`
/// crash-loops the shared store.
pub struct WorkloadFailure {
    /// The workload to fail.
    pub workload_id: String,
    /// Human-readable cause, surfaced in the workload's status message.
    pub reason: String,
}

/// A cheap, cloneable handle a plugin uses to report a [`WorkloadFailure`] to
/// the host. The host injects one into each plugin at start
/// ([`HostPlugin::set_workload_failure_sink`]) and drains it on a background
/// task, transitioning each reported workload to a failed state.
#[derive(Clone)]
pub struct WorkloadFailureSink(tokio::sync::mpsc::UnboundedSender<WorkloadFailure>);

impl WorkloadFailureSink {
    /// Wrap the sender end of the host's workload-failure channel.
    pub fn new(tx: tokio::sync::mpsc::UnboundedSender<WorkloadFailure>) -> Self {
        Self(tx)
    }

    /// Report that `workload_id` has failed for `reason`. Non-blocking and
    /// infallible from the caller's view: if the host has torn down the
    /// receiver, the report is dropped (the host is stopping anyway).
    pub fn report(&self, workload_id: impl Into<String>, reason: impl Into<String>) {
        let _ = self.0.send(WorkloadFailure {
            workload_id: workload_id.into(),
            reason: reason.into(),
        });
    }
}

/// The [`HostPlugin`] trait provides an interface for implementing built-in plugins for the host.
/// A plugin is primarily responsible for implementing a specific [`WitWorld`] as a collection of
/// imports and exports that will be directly linked to the workload's [`wasmtime::component::Linker`].
///
/// For example, the runtime doesn't implement `wasi:keyvalue`, but it's a key capability for many component
/// applications. This crate provides a [`wasi_keyvalue::WasiKeyvalue`] built-in that persists key-value data
/// in-memory and implements the component imports of `wasi:keyvalue` atomics, batch and store.
///
/// You can supply your own [`HostPlugin`] implementations to the [`crate::host::HostBuilder::with_plugin`] function.
#[async_trait::async_trait]
pub trait HostPlugin: std::any::Any + Send + Sync + 'static {
    /// Returns the unique identifier for this plugin.
    ///
    /// This ID must be unique across all plugins registered with a host.
    /// It's used to retrieve plugin instances and avoid conflicts.
    ///
    /// # Returns
    /// A static string slice containing the plugin's unique identifier.
    fn id(&self) -> &'static str;

    /// Returns the WIT interfaces that this plugin provides.
    ///
    /// The returned `WitWorld` contains the imports and exports that this plugin
    /// implements. The plugin's `bind_component` method will only be called if
    /// a workload requires one of these interfaces.
    ///
    /// # Returns
    /// A `WitWorld` containing the plugin's imports and exports.
    fn world(&self) -> WitWorld;

    /// Returns whether this plugin will actually serve `interface`, for an
    /// interface its [`HostPlugin::world`] already matched.
    ///
    /// World matching answers "same namespace:package, compatible version,
    /// interfaces covered", and it treats a host-interface entry that omits its
    /// version as compatible with *any* version. That is the right default —
    /// most workloads pin no version and expect the one plugin serving the
    /// package to take it — but it leaves no way to say "only a version I can
    /// actually bind". Two plugins serving one namespace:package at different
    /// revisions (the sync and async `wasmcloud:messaging` multiplexers) both
    /// match a versionless entry, and only one of them can serve it.
    ///
    /// A matched interface a plugin does not claim is offered to the next
    /// plugin instead, so refusing here hands it to a sibling rather than
    /// failing the workload. Overriding this is only necessary when
    /// [`HostPlugin::world`] cannot express the distinction; the default claims
    /// everything the world matched.
    fn claims(&self, _interface: &WitInterface) -> bool {
        true
    }

    /// The config keys this plugin considers the host operator's rather than a
    /// workload's — where a binding connects, as whom, and what it may reach.
    ///
    /// Only consulted under `workloadConfig: deny`, where these keys are
    /// refused to a workload even when no operator set them: an ungranted
    /// allowlist has to resolve to empty, not to whatever the manifest wrote.
    /// Name every alias the plugin's own reader accepts — a deny set covering
    /// one spelling of a key read under two denies nothing.
    ///
    /// The default owns nothing, which leaves a plugin's config entirely the
    /// workload's to write, as it was before bindings existed.
    fn binding_schema(&self) -> BindingSchema {
        BindingSchema::empty()
    }

    /// Check the operator's declaration for this plugin at host startup.
    ///
    /// Called once by [`crate::host::HostBuilder::build`] with whatever
    /// `host.plugins` declared for this id. A plugin parses each of
    /// [`PluginBindingSet::host_layers`] with its own reader here, so a
    /// declaration it cannot use fails the host rather than the first workload
    /// that needs it — and so an operator cannot write a value the manifest
    /// parser would have rejected.
    ///
    /// The default accepts everything, which is what a plugin with no
    /// [`HostPlugin::binding_schema`] wants.
    ///
    /// # Errors
    ///
    /// Whatever the plugin's own parser refuses. The error should name the
    /// binding.
    fn validate_bindings(&self, _declared: &PluginBindingSet) -> anyhow::Result<()> {
        Ok(())
    }

    /// Whether a workload's `value` for a narrowable `key` is contained by the
    /// host's `ceiling`.
    ///
    /// Called only for keys [`HostPlugin::binding_schema`] marks
    /// [`KeyOwnership::HostCeiling`], only under
    /// [`WorkloadConfigPolicy::Deny`], and only when both sides set the key.
    /// Returning `false` refuses the bind. The generic layer has no opinion
    /// about what containment means for any key.
    ///
    /// A predicate rather than a merge function returning the intersection:
    /// a merge would let a plugin produce a value *neither* side wrote, so the
    /// resolved config would contain a computed artifact and an operator
    /// debugging a grant would not be reading anything anyone declared. Here
    /// the resolved value is always exactly the manifest's, and the host's is
    /// purely a gate.
    ///
    /// The default refuses everything, so a plugin that classifies a key
    /// narrowable and forgets the predicate gets a refusal rather than a silent
    /// acceptance: the unsafe direction is the one that requires writing code.
    fn narrows(&self, _key: &str, _ceiling: &str, _value: &str) -> bool {
        false
    }

    /// Returns whether this plugin supports handling multiple named instances
    /// of the same namespace:package interface.
    ///
    /// When a workload declares multiple host interfaces with the same
    /// namespace:package but different names (e.g., two `wasi:keyvalue` entries
    /// named "cache" and "sessions"), the plugin must be able to distinguish
    /// and route to each named backend.
    ///
    /// The default is `false`, which causes binding to fail if the workload
    /// requires named multiplexing. Plugins that implement multiplexing
    /// should override this to return `true`.
    fn supports_named_instances(&self) -> bool {
        false
    }

    /// Whether this plugin hands a plain, *unlabeled* import to a
    /// single-backend plugin that also serves it.
    ///
    /// Defaults to [`HostPlugin::supports_named_instances`], which is the rule
    /// a closed multiplexer wants: it exists to route `(implements ..)` labels
    /// between backends it names in code, so where a single-backend plugin
    /// serves the same interface, that one is the better answer for an import
    /// carrying no label.
    ///
    /// A plugin that serves both off one backend — a host component plugin
    /// answers a labeled and an unlabeled import from the same store —
    /// overrides this to `false`. Deferring would hand its plain imports to the
    /// very native an operator deployed it to replace, and it would happen the
    /// moment they declared a binding, which says which labels this plugin
    /// answers to and nothing about the imports it already served.
    fn defers_unnamed_instances(&self) -> bool {
        self.supports_named_instances()
    }

    /// Injects metrics into the plugin.
    ///
    /// # Arguments
    /// * `meters` - A `Meters` object containing the metrics to inject.
    async fn inject_meters(&self, _meters: &crate::observability::Meters) {}

    /// Inject a sink the plugin can use to report that a workload it manages has
    /// failed out of band — after it was already running — so the host
    /// transitions it to a failed state. Called once during host start, before
    /// [`HostPlugin::start`]. The default ignores it; a plugin only needs to
    /// override this if it can fail a workload asynchronously (e.g. a host
    /// component plugin evicting a workload whose lifecycle bind crash-loops).
    fn set_workload_failure_sink(&self, _sink: WorkloadFailureSink) {}

    /// Called when the plugin is started during host initialization.
    ///
    /// This method allows plugins to perform any necessary setup before
    /// accepting workloads. The default implementation does nothing.
    ///
    /// # Returns
    /// Ok if the plugin started successfully.
    ///
    /// # Errors
    /// Returns an error if the plugin fails to initialize, which will
    /// prevent the host from starting.
    async fn start(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Called when a workload is binding to this plugin.
    ///
    /// This method is invoked when a workload is in the process of being bound to the plugin,
    /// allowing the plugin to perform any necessary setup or validation before the binding is finalized.
    /// The default implementation does nothing.
    ///
    /// # Arguments
    /// * `workload` - The unresolved workload that is being bound.
    /// * `interfaces` - The set of WIT interfaces that the workload requires from this plugin.
    ///
    /// # Returns
    /// Ok if the binding preparation succeeded.
    ///
    /// # Errors
    /// Returns an error if the plugin cannot support the requested binding.
    async fn on_workload_bind(
        &self,
        _workload: &UnresolvedWorkload,
        _interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Called when a [`WorkloadComponent`] or [`WorkloadService`] is being bound to this plugin.
    ///
    /// This method is called when a workload requires interfaces that this
    /// plugin provides. The plugin should configure the component's linker
    /// with the necessary implementations.
    ///
    /// # Arguments
    /// * `component` - The workload component to bind to this plugin
    /// * `interfaces` - The specific WIT interfaces the component requires
    ///
    /// # Returns
    /// Ok if binding succeeded.
    ///
    /// # Errors
    /// Returns an error if the plugin cannot bind to the component.
    async fn on_workload_item_bind<'a>(
        &self,
        _item: &mut WorkloadItem<'a>,
        _interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Called when a workload has been fully resolved and is ready for use.
    ///
    /// This optional callback allows plugins to perform actions after a workload
    /// has been successfully bound and resolved. The default implementation
    /// does nothing.
    ///
    /// # Arguments
    /// * `workload` - The fully resolved workload
    /// * `component_id` - The ID of the specific component within the workload
    ///
    /// # Returns
    /// Ok if the callback completed successfully.
    ///
    /// # Errors
    /// Returns an error if the plugin fails to handle the resolved workload.
    async fn on_workload_resolved(
        &self,
        _workload: &ResolvedWorkload,
        _component_id: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Called when a workload is being stopped or unbound from this plugin.
    ///
    /// This method allows plugins to clean up any resources associated with
    /// the workload. This can be called during binding failures (before resolution)
    /// or during normal workload shutdown (after resolution).
    ///
    /// Implementations must be idempotent, and must tolerate a workload they
    /// never finished binding: a failed [`HostPlugin::on_workload_bind`] or
    /// [`HostPlugin::on_workload_item_bind`] is rolled back by unbinding the
    /// plugin that failed, so this runs over whatever half of the bind managed
    /// to execute — and it may run again when the workload stops. Release what
    /// is there and treat what is not as already released, rather than
    /// erroring.
    ///
    /// The default implementation does nothing.
    ///
    /// # Arguments
    /// * `workload_id` - The ID of the workload being unbound
    /// * `interfaces` - The interfaces that were bound
    ///
    /// # Returns
    /// Ok if unbinding succeeded.
    ///
    /// # Errors
    /// Returns an error if cleanup fails.
    async fn on_workload_unbind(
        &self,
        _workload_id: &str,
        _interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Called when the plugin is being stopped during host shutdown.
    ///
    /// This method allows plugins to perform cleanup before the host stops.
    /// The default implementation does nothing.
    ///
    /// # Returns
    /// Ok if the plugin stopped successfully.
    ///
    /// # Errors
    /// Returns an error if cleanup fails (errors are logged but don't prevent shutdown).
    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// A tracker for workloads and their components, allowing storage of associated
/// data.
/// The tracker maintains a mapping of workload IDs to their data and
/// components, as well as a mapping of component IDs to their parent workload
/// IDs.
pub struct WorkloadTracker<T, Y> {
    pub workloads: HashMap<String, WorkloadTrackerItem<T, Y>>,
    pub components: HashMap<String, String>,
}

#[derive(Default)]
pub struct WorkloadTrackerItem<T, Y> {
    pub workload_data: Option<T>,
    pub components: HashMap<String, Y>,
}

impl<T, Y> Default for WorkloadTracker<T, Y> {
    fn default() -> Self {
        Self {
            workloads: HashMap::new(),
            components: HashMap::new(),
        }
    }
}

// TODO(lxf): remove once plugins have migrated to use this.
#[allow(dead_code)]
impl<T, Y> WorkloadTracker<T, Y> {
    pub fn add_unresolved_workload(&mut self, workload: &UnresolvedWorkload, data: T) {
        self.workloads.insert(
            workload.id().to_string(),
            WorkloadTrackerItem {
                workload_data: Some(data),
                components: HashMap::new(),
            },
        );
    }

    pub async fn remove_workload(&mut self, workload_id: &str) {
        if let Some(item) = self.workloads.remove(workload_id) {
            for component_id in item.components.keys() {
                self.components.remove(component_id);
            }
        }
    }

    pub async fn remove_workload_with_cleanup<
        FutW: Future<Output = ()>,
        FutC: Future<Output = ()>,
    >(
        &mut self,
        workload_id: &str,
        workload_cleanup: impl FnOnce(Option<T>) -> FutW,
        component_cleanup: impl Fn(Y) -> FutC,
    ) {
        if let Some(item) = self.workloads.remove(workload_id) {
            for (component_id, component_data) in item.components {
                component_cleanup(component_data).await;
                self.components.remove(&component_id);
            }
            workload_cleanup(item.workload_data).await;
        }
    }

    /// Track an item (a component or a long-lived service) by its metadata. Both
    /// `WorkloadComponent` and `WorkloadService` deref to `WorkloadMetadata`, so
    /// existing `&WorkloadComponent` callers coerce unchanged.
    pub fn add_component(&mut self, metadata: &WorkloadMetadata, data: Y) {
        let component_id = metadata.id();
        let workload_id = metadata.workload_id();
        let item = self
            .workloads
            .entry(workload_id.to_string())
            .or_insert_with(|| WorkloadTrackerItem {
                workload_data: None,
                components: HashMap::new(),
            });
        item.components.insert(component_id.to_string(), data);
        self.components
            .insert(component_id.to_string(), workload_id.to_string());
    }

    pub fn get_workload_data(&self, workload_id: &str) -> Option<&T> {
        let item = self.workloads.get(workload_id)?;
        item.workload_data.as_ref()
    }

    pub fn get_component_data(&self, component_id: &str) -> Option<&Y> {
        let workload_id = self.components.get(component_id)?;
        let item = self.workloads.get(workload_id)?;
        item.components.get(component_id)
    }

    pub fn get_component_data_mut(&mut self, component_id: &str) -> Option<&mut Y> {
        let workload_id = self.components.get(component_id)?;
        let item = self.workloads.get_mut(workload_id)?;
        item.components.get_mut(component_id)
    }
}

/// Locks an untrusted path to be within the given root directory.
///
/// Only the filesystem-backed plugins hand it untrusted names, so it compiles
/// with them.
#[cfg(any(feature = "wasi-blobstore", feature = "wasi-keyvalue"))]
pub(crate) fn lock_root(root: impl AsRef<Path>, untrusted: &str) -> Result<PathBuf, &'static str> {
    let path = Path::new(untrusted);

    // Reject absolute paths
    if path.is_absolute() {
        return Err("absolute paths not allowed");
    }

    // Reject paths with parent references
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => return Err("path traversal not allowed"),
            std::path::Component::Prefix(_) => return Err("windows prefixes not allowed"),
            _ => {}
        }
    }

    Ok(root.as_ref().join(path))
}

/// The multiplexed plugin set a host registers to serve `(implements ..)`
/// named imports.
///
/// Every single-backend plugin leaves [`HostPlugin::supports_named_instances`]
/// at its default of `false`, so a workload declaring two entries of one
/// namespace:package under distinct names fails to bind unless a multiplexed
/// plugin is registered alongside. That makes this list load-bearing rather
/// than optional, and it was previously written out by hand in every embedder:
/// `wash host`, `wash dev`, and downstream hosts each kept their own copy.
/// The copies drift — a downstream host lost `(implements ..)` support
/// entirely for three weeks after a refactor rebuilt its registration list and
/// omitted the block, with nothing at compile time to notice. One list here
/// means adding a multiplexed plugin reaches every embedder at once.
///
/// Postgres is deliberately absent: whether it registers at all, and whether
/// as `multiplex_only`, depends on the host's own configuration.
#[cfg(feature = "wasm_component_model_implements")]
pub fn multiplexed_plugins() -> Vec<std::sync::Arc<dyn HostPlugin>> {
    #[allow(unused_mut)]
    let mut plugins: Vec<std::sync::Arc<dyn HostPlugin>> = Vec::new();
    #[allow(unused_imports)]
    use std::sync::Arc;

    #[cfg(feature = "wasi-keyvalue")]
    {
        plugins.push(Arc::new(
            wasi_keyvalue::MultiplexedKeyValue::new()
                .with_provider(Arc::new(wasi_keyvalue::InMemoryProvider))
                .with_provider(Arc::new(wasi_keyvalue::RedisProvider))
                .with_provider(Arc::new(wasi_keyvalue::NatsProvider))
                .with_provider(Arc::new(wasi_keyvalue::FilesystemProvider)),
        ));
        plugins.push(Arc::new(
            wasi_keyvalue::MultiplexedAsyncKeyValue::new()
                .with_provider(Arc::new(wasi_keyvalue::InMemoryProvider))
                .with_provider(Arc::new(wasi_keyvalue::RedisProvider))
                .with_provider(Arc::new(wasi_keyvalue::NatsProvider))
                .with_provider(Arc::new(wasi_keyvalue::FilesystemProvider)),
        ));
    }

    #[cfg(feature = "wasi-blobstore")]
    {
        plugins.push(Arc::new(
            wasi_blobstore::MultiplexedBlobstore::new()
                .with_provider(Arc::new(wasi_blobstore::InMemoryProvider))
                .with_provider(Arc::new(wasi_blobstore::FilesystemProvider))
                .with_provider(Arc::new(wasi_blobstore::NatsBlobProvider)),
        ));
        plugins.push(Arc::new(
            wasi_blobstore::MultiplexedAsyncBlobstore::new()
                .with_provider(Arc::new(wasi_blobstore::InMemoryProvider))
                .with_provider(Arc::new(wasi_blobstore::FilesystemProvider))
                .with_provider(Arc::new(wasi_blobstore::NatsBlobProvider)),
        ));
    }

    plugins.push(Arc::new(
        wasmcloud_messaging::MultiplexedMessaging::new()
            .with_provider(Arc::new(wasmcloud_messaging::InMemoryMsgProvider))
            .with_provider(Arc::new(wasmcloud_messaging::NatsMsgProvider)),
    ));
    plugins.push(Arc::new(
        wasmcloud_messaging::MultiplexedAsyncMessaging::new()
            .with_provider(Arc::new(wasmcloud_messaging::InMemoryMsgProvider))
            .with_provider(Arc::new(wasmcloud_messaging::NatsMsgProvider)),
    ));

    plugins
}
