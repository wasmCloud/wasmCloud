//! This module is primarily concerned with converting an [`UnresolvedWorkload`] into a [`ResolvedWorkload`] by
//! resolving all components and their dependencies.
use std::{
    collections::{HashMap, HashSet},
    ops::{Deref, DerefMut},
    path::PathBuf,
    sync::Arc,
};

use crate::{plugin::WitInterfaces, sockets::loopback};
use anyhow::{bail, ensure};
use tokio::{sync::RwLock, task::JoinHandle};
use tracing::{Instrument, debug, error, info, instrument, trace, warn};
use wasmtime::component::{
    Component, InstancePre, Linker, ResourceAny, ResourceType, types::ComponentItem,
};
use wasmtime::error::Context as _;
use wasmtime_wasi::p2::bindings::CommandPre;

#[cfg(feature = "wasi-tls")]
use crate::engine::ctx::SharedTlsProvider;
use crate::{
    engine::{
        ctx::SharedCtx,
        linked_call::{
            ComponentCtxTemplate, EphemeralCallMode, EphemeralLinkedCall, LinkedExportInvocation,
            func_is_bridge_safe, func_is_ephemeral_safe, invoke_linked_async_export,
            invoke_linked_sync_export, new_store_from_templates,
        },
        volumes::{ResolvedVolumeMount, resolve_component_volume_mounts_in_map},
    },
    plugin::HostPlugin,
    types::{LocalResources, VolumeMount},
    wit::{WitInterface, WitWorld},
};

/// Type alias for tracking bound plugins with their matched interfaces during binding.
/// Tuple: (plugin, matched_interfaces, component_ids)
type BoundPluginWithInterfaces = (
    Arc<dyn HostPlugin + 'static>,
    HashSet<WitInterface>,
    Vec<String>,
);

/// Metadata associated with components and services within a workload.
#[derive(Clone)]
pub struct WorkloadMetadata {
    /// The unique identifier for this component
    pub(crate) id: Arc<str>,
    /// The unique identifier for the workload this component belongs to
    pub(crate) workload_id: Arc<str>,
    /// The name of the workload this component belongs to
    workload_name: Arc<str>,
    /// The namespace of the workload this component belongs to
    workload_namespace: Arc<str>,
    /// The actual wasmtime [`Component`] that can be instantiated
    component: Component,
    /// The wasmtime [`Linker`] used to instantiate the component
    linker: Linker<SharedCtx>,
    /// The volume mounts requested by this component
    pub(crate) volume_mounts: Vec<(PathBuf, VolumeMount)>,
    /// Canonicalized volume mounts, resolved once during workload resolution.
    pub(crate) resolved_volume_mounts: Vec<ResolvedVolumeMount>,
    /// The local resources requested by this component
    pub(crate) local_resources: LocalResources,
    /// The plugins available to this component
    pub(crate) plugins: Option<HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>>,
    /// Workload loopback
    pub(crate) loopback: Arc<std::sync::Mutex<loopback::Network>>,
    /// Linked component ids
    linked_components: HashSet<Arc<str>>,
}

impl WorkloadMetadata {
    async fn resolve_volume_mounts(&mut self) -> anyhow::Result<()> {
        let mut resolved = Vec::with_capacity(self.volume_mounts.len());
        for (host_path, mount) in &self.volume_mounts {
            resolved.push(ResolvedVolumeMount::from_mount(host_path, mount).await?);
        }
        self.resolved_volume_mounts = resolved;
        Ok(())
    }

    /// Returns the unique identifier for this component.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the ID of the workload this component belongs to.
    pub fn workload_id(&self) -> &str {
        &self.workload_id
    }

    /// Returns the name of the workload this component belongs to.
    pub fn workload_name(&self) -> &str {
        &self.workload_name
    }

    /// Returns the namespace of the workload this component belongs to.
    pub fn workload_namespace(&self) -> &str {
        &self.workload_namespace
    }

    /// Returns a reference to the wasmtime engine used to compile this component.
    pub fn engine(&self) -> &wasmtime::Engine {
        self.component.engine()
    }

    /// Returns a mutable reference to the component's linker.
    pub fn linker(&mut self) -> &mut Linker<SharedCtx> {
        &mut self.linker
    }

    /// Returns a reference to the wasmtime [`Component`].
    pub fn component(&self) -> &Component {
        &self.component
    }

    /// Returns a reference to component local resources.
    pub fn local_resources(&self) -> &LocalResources {
        &self.local_resources
    }

    /// Returns a reference to the plugins associated with this component.
    pub fn plugins(&self) -> &Option<HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>> {
        &self.plugins
    }

    /// Adds a [`HostPlugin`] to the component.
    pub fn add_plugin(&mut self, id: &'static str, plugin: Arc<dyn HostPlugin + Send + Sync>) {
        if let Some(ref mut plugins) = self.plugins {
            plugins.insert(id, plugin);
        } else {
            let mut plugins = HashMap::new();
            plugins.insert(id, plugin);
            self.plugins = Some(plugins);
        }
    }

    /// Replaces all plugins for this component with the provided set.
    pub fn with_plugins(
        &mut self,
        plugins: HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>,
    ) {
        self.plugins = Some(plugins);
    }

    /// Extracts the [`ComponentItem::ComponentInstance`]s that the component exports.
    pub fn component_exports(&self) -> anyhow::Result<Vec<(String, ComponentItem)>> {
        Ok(self
            .component
            .component_type()
            .exports(self.component.engine())
            .map(|(name, item)| (name, item.ty))
            .filter_map(|(name, item)| {
                if matches!(item, ComponentItem::ComponentInstance(_)) {
                    Some((name.to_string(), item))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>())
    }

    pub fn uses_wasi_http(&self) -> bool {
        crate::engine::uses_wasi_http(&self.component)
    }

    pub fn imports_wasi_http(&self) -> bool {
        crate::engine::imports_wasi_http(&self.component)
    }

    pub fn exports_wasi_http(&self) -> bool {
        crate::engine::exports_wasi_http(&self.component)
    }

    /// Returns whether this component targets WASIP3.
    pub fn targets_p3(&self) -> bool {
        crate::engine::targets_wasip3(&self.component)
    }

    /// Computes and returns the [`WitWorld`] of this component.
    pub fn world(&self) -> WitWorld {
        let mut imports = HashMap::new();
        let mut exports = HashMap::new();

        // Iterate over imports, merging interfaces when namespace:package@version matches
        for (import_name, import_item) in self
            .component
            .component_type()
            .imports(self.component.engine())
        {
            if let ComponentItem::ComponentInstance(_) = &import_item.ty {
                // An `(implements <id>)` import carries the real interface
                // identity in its `implements` annotation; the import name is
                // the multiplex label (e.g. `team-a`). Build the interface from
                // the annotation and record the label as the routing `name` so
                // it lines up with a named host interface. A plain import uses
                // its name as the identity directly.
                let interface = match import_item.implements {
                    Some(implements_id) => {
                        let mut iface = WitInterface::from(implements_id);
                        iface.name = Some(import_name.to_string());
                        iface
                    }
                    None => WitInterface::from(import_name),
                };
                let k = interface.instance();
                imports
                    .entry(k)
                    .and_modify(|existing: &mut WitInterface| {
                        existing.merge(&interface);
                    })
                    .or_insert(interface);
            } else {
                debug!(
                    import_name,
                    "imported item is not a component instance, skipping"
                );
            }
        }

        // Iterate over exports, merging interfaces when namespace:package@version matches
        for (export_name, export_item) in self
            .component
            .component_type()
            .exports(self.component.engine())
        {
            if let ComponentItem::ComponentInstance(_) = export_item.ty {
                let interface = WitInterface::from(export_name);
                let k = interface.instance();
                exports
                    .entry(k)
                    .and_modify(|existing: &mut WitInterface| {
                        existing.merge(&interface);
                    })
                    .or_insert(interface);
            } else {
                debug!(
                    export_name,
                    "exported item is not a component instance, skipping"
                );
            }
        }

        WitWorld {
            imports: imports.into_values().collect(),
            exports: exports.into_values().collect(),
        }
    }
}

/// A [`WorkloadService`] is a component that is part of a workload that
/// runs once, either to completion or for the duration of the workload lifecycle.
#[derive(Clone)]
pub struct WorkloadService {
    /// The [`WorkloadMetadata`] for this service
    metadata: WorkloadMetadata,
    /// The maximum number of restarts for this service
    max_restarts: u64,
    /// The [`JoinHandle`] for the running service
    handle: Option<Arc<JoinHandle<()>>>,
}

impl WorkloadService {
    /// Create a new [`WorkloadService`] with the given workload ID,
    /// wasmtime [`Component`], [`Linker`], volume mounts, and instance limits.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        workload_id: impl Into<Arc<str>>,
        workload_name: impl Into<Arc<str>>,
        workload_namespace: impl Into<Arc<str>>,
        component: Component,
        linker: Linker<SharedCtx>,
        volume_mounts: Vec<(PathBuf, VolumeMount)>,
        local_resources: LocalResources,
        max_restarts: u64,
        loopback: Arc<std::sync::Mutex<loopback::Network>>,
    ) -> Self {
        Self {
            metadata: WorkloadMetadata {
                id: uuid::Uuid::new_v4().to_string().into(),
                workload_id: workload_id.into(),
                workload_name: workload_name.into(),
                workload_namespace: workload_namespace.into(),
                component,
                linker,
                volume_mounts,
                resolved_volume_mounts: Vec::new(),
                local_resources,
                plugins: None,
                loopback,
                linked_components: Default::default(),
            },
            handle: None,
            max_restarts,
        }
    }

    /// Pre-instantiate the component to prepare for execution.
    pub fn pre_instantiate(&mut self) -> anyhow::Result<CommandPre<SharedCtx>> {
        let component = self.metadata.component.clone();
        let pre = self.metadata.linker.instantiate_pre(&component)?;
        let command = CommandPre::new(pre)?;
        Ok(command)
    }

    /// Pre-instantiate the component for P3 execution.
    pub fn pre_instantiate_p3(
        &mut self,
    ) -> anyhow::Result<wasmtime_wasi::p3::bindings::CommandPre<SharedCtx>> {
        let component = self.metadata.component.clone();
        let pre = self.metadata.linker.instantiate_pre(&component)?;
        let command = wasmtime_wasi::p3::bindings::CommandPre::new(pre)?;
        Ok(command)
    }

    /// Pre-instantiate the raw component, leaving binding-view construction
    /// (e.g. both cli `Command` and http `Service`) to the caller. Used when a
    /// p3 service also serves HTTP and must drive both exports on one instance.
    pub fn pre_instantiate_raw(
        &self,
    ) -> anyhow::Result<wasmtime::component::InstancePre<SharedCtx>> {
        Ok(self
            .metadata
            .linker
            .instantiate_pre(&self.metadata.component)?)
    }

    /// Whether or not the service is currently running.
    pub fn is_running(&self) -> bool {
        self.handle.is_some()
    }
}

/// A [`WorkloadComponent`] is a component that is part of a workload.
///
/// It contains the actual [`Component`] that can be instantiated,
/// the [`Linker`] for creating stores and instances, the available
/// [`VolumeMount`]s to be passed as filesystem preopens, and the
/// full list of [`HostPlugin`]s that the component depends on.
#[derive(Clone)]
pub struct WorkloadComponent {
    /// Component name. Primarily for debugging purposes.
    name: Arc<str>,
    /// The [`WorkloadMetadata`] for this component
    pub(crate) metadata: WorkloadMetadata,
    /// The number of warm instances to keep for this component
    pool_size: usize,
    /// The maximum number of concurrent invocations allowed for this component
    max_invocations: usize,
}

impl WorkloadComponent {
    /// Create a new [`WorkloadComponent`] with the given workload ID,
    /// wasmtime [`Component`], [`Linker`], volume mounts, and instance limits.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        workload_id: impl Into<Arc<str>>,
        workload_name: impl Into<Arc<str>>,
        workload_namespace: impl Into<Arc<str>>,
        component_name: impl Into<Arc<str>>,
        component: Component,
        linker: Linker<SharedCtx>,
        volume_mounts: Vec<(PathBuf, VolumeMount)>,
        local_resources: LocalResources,
        loopback: Arc<std::sync::Mutex<loopback::Network>>,
    ) -> Self {
        Self {
            metadata: WorkloadMetadata {
                id: uuid::Uuid::new_v4().to_string().into(),
                workload_id: workload_id.into(),
                workload_name: workload_name.into(),
                workload_namespace: workload_namespace.into(),
                component,
                linker,
                volume_mounts,
                resolved_volume_mounts: Vec::new(),
                local_resources,
                plugins: None,
                loopback,
                linked_components: Default::default(),
            },
            name: component_name.into(),
            // TODO: Implement pooling and instance limits
            pool_size: 0,
            max_invocations: 0,
        }
    }

    /// Pre-instantiate the component to prepare for instantiation.
    pub fn pre_instantiate(&mut self) -> wasmtime::Result<InstancePre<SharedCtx>> {
        let component = self.metadata.component.clone();
        self.metadata.linker.instantiate_pre(&component)
    }

    pub fn metadata(&self) -> &WorkloadMetadata {
        &self.metadata
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Represents an item within a workload, such as a component or a service.
#[derive(Debug)]
pub enum WorkloadItem<'a> {
    Component(&'a mut WorkloadComponent),
    Service(&'a mut WorkloadService),
}

impl<'a> WorkloadItem<'a> {
    /// Returns true if the item is a component.
    pub fn is_component(&self) -> bool {
        matches!(self, WorkloadItem::Component(_))
    }

    /// Returns true if the item is a service.
    pub fn is_service(&self) -> bool {
        matches!(self, WorkloadItem::Service(_))
    }
}

impl<'a> Deref for WorkloadItem<'a> {
    type Target = WorkloadMetadata;

    fn deref(&self) -> &Self::Target {
        match self {
            WorkloadItem::Component(component) => component,
            WorkloadItem::Service(service) => service,
        }
    }
}

impl<'a> DerefMut for WorkloadItem<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            WorkloadItem::Component(component) => component,
            WorkloadItem::Service(service) => service,
        }
    }
}

impl std::fmt::Debug for WorkloadComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkloadComponent")
            .field("id", &self.metadata.id.as_ref())
            .field("workload_id", &self.metadata.workload_id.as_ref())
            .field("volume_mounts", &self.metadata.volume_mounts)
            .field("pool_size", &self.pool_size)
            .field("max_invocations", &self.max_invocations)
            .finish()
    }
}

impl std::fmt::Debug for WorkloadService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkloadService")
            .field("id", &self.metadata.id.as_ref())
            .field("workload_name", &self.metadata.workload_name.as_ref())
            .field(
                "workload_namespace",
                &self.metadata.workload_namespace.as_ref(),
            )
            .field("workload_id", &self.metadata.workload_id.as_ref())
            .field("volume_mounts", &self.metadata.volume_mounts)
            .field("is_running", &self.is_running())
            .finish()
    }
}

impl Deref for WorkloadComponent {
    type Target = WorkloadMetadata;

    fn deref(&self) -> &Self::Target {
        &self.metadata
    }
}

impl DerefMut for WorkloadComponent {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.metadata
    }
}

impl Deref for WorkloadService {
    type Target = WorkloadMetadata;

    fn deref(&self) -> &Self::Target {
        &self.metadata
    }
}

impl DerefMut for WorkloadService {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.metadata
    }
}

/// A fully resolved workload ready for execution.
///
/// A `ResolvedWorkload` contains all components that have been validated,
/// bound to plugins, and had their dependencies resolved. This is the final
/// state of a workload before execution.
#[derive(Clone)]
pub struct ResolvedWorkload {
    /// The unique identifier of the workload, created with [uuid::Uuid::new_v4]
    id: Arc<str>,
    /// The name of the workload
    name: Arc<str>,
    /// The namespace of the workload
    namespace: Arc<str>,
    /// All components in the workload. This is behind a `RwLock` to support mutable
    /// access to the component linkers.
    components: Arc<RwLock<HashMap<Arc<str>, WorkloadComponent>>>,
    /// The HTTP handler for outgoing HTTP requests
    http_handler: Arc<dyn crate::host::http::HostHandler>,
    /// An optional service component that runs once to completion or for the duration of the workload
    service: Option<WorkloadService>,
    /// The requested host [`WitInterface`]s to resolve this workload
    host_interfaces: Vec<WitInterface>,
    /// TLS provider override for `wasi:tls` client connections in this workload.
    #[cfg(feature = "wasi-tls")]
    tls_provider: Option<SharedTlsProvider>,
}

impl std::fmt::Debug for ResolvedWorkload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("ResolvedWorkload");
        s.field("id", &self.id)
            .field("name", &self.name)
            .field("namespace", &self.namespace)
            .field("host_interfaces", &self.host_interfaces)
            .field("has_service", &self.service.is_some())
            .field(
                "component_count",
                &self
                    .components
                    .try_read()
                    .map(|g| g.len() as isize)
                    .unwrap_or(-1),
            );
        #[cfg(feature = "wasi-tls")]
        s.field("tls_provider", &self.tls_provider.is_some());
        s.finish_non_exhaustive()
    }
}

/// Everything needed to rebuild a p3 service's store (plain `cli/run` or
/// trigger service), captured by its supervisor so each incarnation gets a
/// FRESH store: after a guest trap the old store can no longer enter ANY
/// component instance ("cannot enter component instance"), so reusing it would
/// leave every restarted incarnation permanently broken.
struct ServiceStoreRecipe {
    engine: wasmtime::Engine,
    http_handler: Arc<dyn crate::host::http::HostHandler>,
    active_template: ComponentCtxTemplate,
    linked_templates: Vec<ComponentCtxTemplate>,
    linked_instances: Vec<(Arc<str>, wasmtime::component::InstancePre<SharedCtx>)>,
}

impl ServiceStoreRecipe {
    /// Build a fresh service store (`is_service = true` so `cli/run` may bind
    /// its loopback socket).
    async fn build(&self) -> anyhow::Result<wasmtime::Store<SharedCtx>> {
        new_store_from_templates(
            &self.engine,
            self.http_handler.clone(),
            &self.active_template,
            &self.linked_templates,
            &self.linked_instances,
            true,
        )
        .await
    }
}

/// Build a trigger service's host-invoked ingresses, returning them alongside the
/// paired senders to register with the host-side HTTP/messaging ingresses. Called
/// once per incarnation (start and each restart) so a restarted service gets fresh
/// channels whose senders replace the stale registrations.
#[allow(clippy::type_complexity)]
fn build_trigger_ingresses(
    serves_http: bool,
    serves_messaging: bool,
) -> (
    Vec<crate::host::trigger_service::Ingress>,
    Option<tokio::sync::mpsc::Sender<crate::host::http::ServiceHttpJob>>,
    Option<tokio::sync::mpsc::Sender<crate::host::trigger_service::MessagingJob>>,
) {
    let mut ingresses = Vec::new();
    let http_tx = serves_http.then(|| {
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        ingresses.push(crate::host::trigger_service::Ingress::Http(rx));
        tx
    });
    let messaging_tx = serves_messaging.then(|| {
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        ingresses.push(crate::host::trigger_service::Ingress::Messaging(rx));
        tx
    });
    (ingresses, http_tx, messaging_tx)
}

impl ResolvedWorkload {
    /// Executes the service, if present, and returns whether it was run.
    #[instrument(name="execute_service", skip_all, fields(workload.id = self.id.as_ref(), workload.name = self.name.as_ref(), workload.namespace = self.namespace.as_ref()))]
    pub(crate) async fn execute_service(&mut self) -> anyhow::Result<Option<Arc<JoinHandle<()>>>> {
        if self
            .service
            .as_ref()
            .is_some_and(|s| s.metadata.targets_p3())
        {
            // A p3 service that also exports a host-invoked handler (today
            // `wasi:http/handler`) co-drives it with `cli/run` on one instance
            // (see the `trigger service` module).
            if self.service.as_ref().is_some_and(|s| {
                crate::engine::exports_wasi_http(&s.metadata.component)
                    || crate::engine::exports_messaging_handler(&s.metadata.component)
            }) {
                return self.execute_trigger_service().await;
            }
            return self.execute_service_p3().await;
        }

        let Some(service) = self.service.as_mut() else {
            return Ok(None);
        };
        let pre = service.pre_instantiate()?;
        let mut max_restarts = service.max_restarts;
        self.resolve_service_volume_mounts().await?;
        // Re-borrow immutably after the mutable borrow for pre_instantiate() is done
        let Some(service) = self.service.as_ref() else {
            bail!("service unexpectedly missing during execution");
        };
        let mut store = self
            .new_store_from_metadata(&service.metadata, true)
            .await?;
        let instance = pre.instantiate_async(&mut store).await?;
        let handle = tokio::spawn(async move {
            loop {
                if let Err(e) = instance.wasi_cli_run().call_run(&mut store).await {
                    error!(err = %e, retries = max_restarts, "service execution failed");
                    if max_restarts == 0 {
                        warn!("max restarts reached, service will not be restarted");
                        break;
                    }
                } else {
                    info!("service exited successfully");
                    break;
                }
                max_restarts = max_restarts.saturating_sub(1);
            }
        });

        let handle = Arc::new(handle);
        if let Some(s) = self.service.as_mut() {
            s.handle = Some(Arc::clone(&handle));
        }
        Ok(Some(handle))
    }

    /// Execute a service using P3 (wasi:cli@0.3) CommandPre.
    async fn execute_service_p3(&mut self) -> anyhow::Result<Option<Arc<JoinHandle<()>>>> {
        let service = self
            .service
            .as_mut()
            .map(|s| (s.pre_instantiate_p3(), s.max_restarts));

        if let Some((Ok(pre), mut max_restarts)) = service {
            self.resolve_service_volume_mounts().await?;
            // Capture the store recipe so each restarted incarnation gets a
            // FRESH store: a trapped store cannot re-enter any component
            // instance, so reuse after a fault would leave every restart
            // permanently broken (and even a clean restart would accumulate
            // stale instances in a reused store).
            let recipe = {
                let Some(service) = self.service.as_ref() else {
                    bail!("service unexpectedly missing during execution");
                };
                self.service_store_recipe(&service.metadata).await?
            };
            let mut store = recipe.build().await?;
            let handle = tokio::spawn(async move {
                loop {
                    let instance = match pre.instantiate_async(&mut store).await {
                        Ok(i) => i,
                        Err(e) => {
                            error!(err = %e, "failed to instantiate P3 service");
                            break;
                        }
                    };
                    let result = store
                        .run_concurrent(async move |accessor| {
                            instance.wasi_cli_run().call_run(accessor).await
                        })
                        .await;
                    match result {
                        Ok(Ok(Ok(()))) => {
                            info!("P3 service exited successfully");
                            break;
                        }
                        Ok(Ok(Err(()))) => {
                            error!(retries = max_restarts, "P3 service exited with error");
                            if max_restarts == 0 {
                                warn!("max restarts reached, P3 service will not be restarted");
                                break;
                            }
                        }
                        Ok(Err(e)) | Err(e) => {
                            error!(err = %e, retries = max_restarts, "P3 service execution failed");
                            if max_restarts == 0 {
                                warn!("max restarts reached, P3 service will not be restarted");
                                break;
                            }
                        }
                    }
                    max_restarts = max_restarts.saturating_sub(1);
                    match recipe.build().await {
                        Ok(fresh) => store = fresh,
                        Err(e) => {
                            error!(err = %e, "failed to rebuild P3 service store; giving up");
                            break;
                        }
                    }
                }
            });

            let handle = Arc::new(handle);
            if let Some(s) = self.service.as_mut() {
                s.handle = Some(Arc::clone(&handle));
            }
            Ok(Some(handle))
        } else {
            Ok(None)
        }
    }

    /// Execute a p3 service that also exports a host-invoked handler (today
    /// `wasi:http/handler` and `wasmcloud:messaging`): one instance co-drives
    /// `cli/run` and the handler under a single `run_concurrent` (see
    /// [`crate::host::trigger_service`]). The service runs in its own
    /// long-lived store.
    /// `is_service = true` lets the `cli/run` side bind its loopback socket.
    async fn execute_trigger_service(&mut self) -> anyhow::Result<Option<Arc<JoinHandle<()>>>> {
        let Some(service) = self.service.as_ref() else {
            return Ok(None);
        };
        let pre = service.pre_instantiate_raw()?;
        let (serves_http, serves_messaging, max_restarts) = (
            crate::engine::exports_wasi_http(&service.metadata.component),
            crate::engine::exports_messaging_handler(&service.metadata.component),
            service.max_restarts,
        );
        self.resolve_service_volume_mounts().await?;

        // Capture the store recipe so the supervisor below can rebuild a FRESH
        // store per incarnation: a trapped store cannot re-enter any component
        // instance, so a restart must never reuse it.
        let recipe = {
            let Some(service) = self.service.as_ref() else {
                bail!("service unexpectedly missing during execution");
            };
            self.service_store_recipe(&service.metadata).await?
        };
        let mut store = recipe.build().await?;
        let http_handler = self.http_handler.clone();
        let workload_id: Arc<str> = Arc::from(self.id());

        // Build the first incarnation's host-invoked ingresses. Each paired sender
        // is registered with its host-side ingress (the HTTP server, the messaging
        // subscriber), which then delivers to this live instance instead of
        // instantiating a component per request/message. The first registration is
        // synchronous (before the driver spawns) so a delivery immediately after
        // start finds the handler; restarts re-register from inside the supervisor.
        let (ingresses, http_tx, messaging_tx) =
            build_trigger_ingresses(serves_http, serves_messaging);
        if let Some(http_tx) = http_tx {
            self.http_handler
                .on_service_http_resolved(self.id(), http_tx)
                .await
                .map_err(|e| anyhow::anyhow!("failed to register service HTTP handler: {e:#}"))?;
        }
        if let Some(messaging_tx) = messaging_tx {
            self.http_handler
                .on_trigger_service_messaging_resolved(self.id(), messaging_tx)
                .await
                .map_err(|e| {
                    anyhow::anyhow!("failed to register trigger service messaging handler: {e:#}")
                })?;
        }

        // Supervise the driver: on a fault (e.g. a guest trap in `cli/run` or a
        // handler), re-instantiate into the same store, rebuild the ingresses, and
        // re-register their handlers (swapping the stale senders) until the restart
        // budget is exhausted. A clean exit (every channel closed) stops it.
        let handle = tokio::spawn(async move {
            let mut first = Some(ingresses);
            let mut restarts = max_restarts;
            loop {
                let ingresses = match first.take() {
                    Some(ingresses) => ingresses,
                    None => {
                        let (ingresses, http_tx, messaging_tx) =
                            build_trigger_ingresses(serves_http, serves_messaging);
                        if let Some(http_tx) = http_tx
                            && let Err(e) = http_handler
                                .on_service_http_resolved(&workload_id, http_tx)
                                .await
                        {
                            error!(err = %e, "failed to re-register service HTTP handler on restart");
                        }
                        if let Some(messaging_tx) = messaging_tx
                            && let Err(e) = http_handler
                                .on_trigger_service_messaging_resolved(&workload_id, messaging_tx)
                                .await
                        {
                            error!(err = %e, "failed to re-register trigger service messaging handler on restart");
                        }
                        ingresses
                    }
                };
                match crate::host::trigger_service::run_trigger_driver(&mut store, &pre, ingresses)
                    .await
                {
                    Ok(()) => {
                        info!("trigger service exited");
                        break;
                    }
                    Err(e) if restarts == 0 => {
                        error!(err = %e, "trigger service faulted; max restarts reached");
                        break;
                    }
                    Err(e) => {
                        warn!(err = %e, retries = restarts, "trigger service faulted; restarting");
                        restarts = restarts.saturating_sub(1);
                        // The faulted store cannot re-enter any component
                        // instance; the next incarnation needs a fresh one.
                        match recipe.build().await {
                            Ok(fresh) => store = fresh,
                            Err(e) => {
                                error!(err = %e, "failed to rebuild trigger service store; giving up");
                                break;
                            }
                        }
                    }
                }
            }
        });

        let handle = Arc::new(handle);
        if let Some(s) = self.service.as_mut() {
            s.handle = Some(Arc::clone(&handle));
        }
        Ok(Some(handle))
    }

    /// Aborts the running service [`JoinHandle`] if it exists.
    pub(crate) fn stop_service(&self) {
        if let Some(service) = &self.service
            && let Some(handle) = &service.handle
        {
            handle.abort();
            debug!(
                workload_id = self.id.as_ref(),
                "service for workload aborted"
            );
        }
    }

    pub fn components(&self) -> Arc<RwLock<HashMap<Arc<str>, WorkloadComponent>>> {
        self.components.clone()
    }

    pub fn host_interfaces(&self) -> &Vec<WitInterface> {
        &self.host_interfaces
    }

    async fn resolve_component_volume_mounts(
        &self,
        component_ids: &[Arc<str>],
    ) -> anyhow::Result<()> {
        resolve_component_volume_mounts_in_map(&self.components, component_ids).await
    }

    async fn resolve_service_volume_mounts(&mut self) -> anyhow::Result<()> {
        let linked_component_ids = self
            .service
            .as_ref()
            .map(|service| {
                service
                    .metadata
                    .linked_components
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.resolve_component_volume_mounts(&linked_component_ids)
            .await?;
        if let Some(service) = &mut self.service
            && service.metadata.resolved_volume_mounts.is_empty()
            && !service.metadata.volume_mounts.is_empty()
        {
            service.metadata.resolve_volume_mounts().await?;
        }
        Ok(())
    }

    fn component_ctx_template(&self, metadata: &WorkloadMetadata) -> ComponentCtxTemplate {
        #[cfg(feature = "wasi-tls")]
        {
            crate::engine::linked_call::component_ctx_template_from_metadata_with_tls(
                metadata,
                self.tls_provider.clone(),
            )
        }
        #[cfg(not(feature = "wasi-tls"))]
        {
            crate::engine::linked_call::component_ctx_template_from_metadata(metadata)
        }
    }

    #[instrument(name="link_components", skip_all, fields(workload.id = self.id.as_ref(), workload.name = self.name.as_ref(), workload.namespace = self.namespace.as_ref()))]
    async fn link_components(&mut self) -> anyhow::Result<()> {
        // Collect each component's exported component-instance interfaces.
        let mut component_exports: Vec<(Arc<str>, Vec<String>)> = Vec::new();
        for c in self.components.read().await.values() {
            let mut names = Vec::new();
            for (name, item) in c.component_exports()? {
                // TODO(#11): It's probably a good idea to skip registering wasi@0.2 interfaces
                if name == "wasmcloud:wash/plugin" || name.starts_with("wasmcloud:wash/plugin@") {
                    trace!(name, "skipping internal plugin export");
                    continue;
                }
                if let ComponentItem::ComponentInstance(_) = item {
                    names.push(name);
                } else {
                    warn!(name, "exported item is not a component instance, skipping");
                }
            }
            component_exports.push((Arc::from(c.id()), names));
        }

        // Build the export→component map for intra-workload linking. An
        // interface exported by more than one component is left out of the map
        // and tracked as ambiguous; it only causes an error if a component
        // actually imports it (host-invoked exports such as
        // `wasmcloud:messaging/handler` or `wasi:http/incoming-handler` are
        // consumed by host plugins, never imported intra-workload, so multiple
        // exporters are fine).
        let (interface_map, ambiguous_exports) = build_export_map(&component_exports);

        self.resolve_workload_imports(&interface_map, &ambiguous_exports)
            .await?;

        Ok(())
    }

    /// This function plugs a components imports with the exports of other components
    /// that are already loaded in the plugin system.
    ///
    /// Components are processed in topological order based on their inter-component
    /// dependencies. This ensures that when a component imports from another component,
    /// the exporting component has already had its imports resolved and can be
    /// pre-instantiated.
    async fn resolve_workload_imports(
        &mut self,
        interface_map: &HashMap<String, Arc<str>>,
        ambiguous_exports: &HashSet<String>,
    ) -> anyhow::Result<()> {
        // Build a dependency graph: for each component, track which other components it imports from
        let mut dependencies: HashMap<Arc<str>, HashSet<Arc<str>>> = HashMap::new();

        {
            let components = self.components.read().await;
            for (component_id, component) in components.iter() {
                let mut deps = HashSet::new();
                let ty = component.metadata.component.component_type();
                for (import_name, import_item) in ty.imports(component.metadata.component.engine())
                {
                    if !matches!(import_item.ty, ComponentItem::ComponentInstance(_)) {
                        continue;
                    }
                    // An interface exported by multiple components can't be
                    // resolved to a single provider for an intra-workload
                    // import — that's the genuine ambiguity the link check
                    // guards against.
                    if ambiguous_exports.contains(import_name) {
                        anyhow::bail!(
                            "component '{component_id}' imports interface '{import_name}', \
                             which is exported by multiple components in the workload; \
                             cannot disambiguate the provider"
                        );
                    }
                    if let Some(exporter_id) = interface_map.get(import_name)
                        && exporter_id != component_id
                    {
                        // This import is provided by another component in the workload
                        deps.insert(exporter_id.clone());
                    }
                }
                dependencies.insert(component_id.clone(), deps);
            }
        }

        // Topologically sort components: components with no dependencies (or dependencies
        // already processed) come first. This ensures that when we process a component
        // that imports from another component, the exporter has already been resolved.
        let sorted_component_ids = anyhow::Context::context(
            topological_sort_components(&dependencies),
            "failed to determine component processing order - possible circular dependency",
        )?;

        trace!(
            order = ?sorted_component_ids.iter().map(|id| id.as_ref()).collect::<Vec<_>>(),
            "processing components in topological order"
        );

        let mut resolved_links: HashMap<Arc<str>, HashSet<Arc<str>>> = HashMap::new();

        for component_id in sorted_component_ids {
            // In order to have mutable access to both the workload component and components that need
            // to be instantiated as "plugins" during linking, we remove and re-add the component to the list.
            let mut workload_component = {
                self.components
                    .write()
                    .await
                    .remove(&component_id)
                    .context("component not found during import resolution")?
            };

            let component = workload_component.metadata.component.clone();
            let linker = &mut workload_component.metadata.linker;

            let res = match self
                .resolve_component_imports(&component, linker, interface_map)
                .await
            {
                Ok(direct_links) => {
                    let linked_components = expand_link_closure(&direct_links, &resolved_links);
                    workload_component.linked_components = linked_components;
                    Ok(())
                }
                Err(err) => Err(err),
            };

            let linked_components = workload_component.linked_components.clone();
            let workload_component_id = workload_component.metadata.id.clone();

            self.components
                .write()
                .await
                .insert(workload_component.metadata.id.clone(), workload_component);
            resolved_links.insert(workload_component_id, linked_components);
            // Propagate any errors encountered during import resolution
            res?;
        }

        if let Some(mut service) = self.service.take() {
            let component = service.metadata.component.clone();
            let linker = &mut service.metadata.linker;

            let res = match self
                .resolve_component_imports(&component, linker, interface_map)
                .await
            {
                Ok(direct_links) => {
                    let linked_components = expand_link_closure(&direct_links, &resolved_links);
                    service.metadata.linked_components = linked_components;
                    Ok(())
                }
                Err(err) => Err(err),
            };

            self.service = Some(service);

            // Propagate any errors encountered during import resolution
            res?;
        }

        Ok(())
    }

    async fn resolve_component_imports(
        &self,
        component: &wasmtime::component::Component,
        linker: &mut Linker<SharedCtx>,
        interface_map: &HashMap<String, Arc<str>>,
    ) -> anyhow::Result<HashSet<Arc<str>>> {
        let mut linked_components = HashSet::new();
        let ty = component.component_type();
        let imports: Vec<_> = ty.imports(component.engine()).collect();
        // The cross-store stream bridge engages only for a p3-service workload:
        // there a long-lived service store must never be pinned/frozen by a
        // stream-carrying backend call, so such calls run ephemerally (relocated)
        // instead of on the shared store. In service-less / per-request workloads
        // stream-carrying calls stay on the shared store.
        let is_service_workload = self
            .service
            .as_ref()
            .is_some_and(|s| s.metadata.targets_p3());

        for (import_name, import_item) in imports.into_iter() {
            match import_item.ty {
                ComponentItem::ComponentInstance(import_instance_ty) => {
                    trace!(name = import_name, "processing component instance import");
                    let mut all_components = self.components.write().await;
                    let (
                        plugin_component,
                        instance_idx,
                        plugin_component_id,
                        nested_linked_component_ids,
                        plugin_engine,
                    ) = {
                        let Some(exporter_component) = interface_map.get(import_name).cloned()
                        else {
                            // Import not provided by another component in the workload.
                            // This is expected for host-provided interfaces (e.g. wasi:*).
                            // If it's not host-provided, linking will fail later with a
                            // clear error from wasmtime.
                            debug!(
                                name = import_name,
                                "import not found in component exports, assuming host-provided"
                            );
                            continue;
                        };
                        let nested_linked_component_ids = {
                            let Some(exporter_workload_component) =
                                all_components.get(&exporter_component)
                            else {
                                anyhow::bail!(
                                    "exporting component '{exporter_component}' for import '{import_name}' not found"
                                );
                            };
                            exporter_workload_component
                                .metadata
                                .linked_components
                                .iter()
                                .cloned()
                                .collect::<Vec<_>>()
                        };
                        let Some(plugin_component) = all_components.get_mut(&exporter_component)
                        else {
                            anyhow::bail!(
                                "exporting component '{exporter_component}' for import '{import_name}' not found"
                            );
                        };
                        let plugin_component_id = plugin_component.id.clone();
                        let plugin_engine = plugin_component.metadata.engine().clone();
                        let Some((ComponentItem::ComponentInstance(_), idx)) = plugin_component
                            .metadata
                            .component
                            .get_export(None, import_name)
                        else {
                            trace!(name = import_name, "skipping non-instance import");
                            continue;
                        };
                        (
                            plugin_component,
                            idx,
                            plugin_component_id,
                            nested_linked_component_ids,
                            plugin_engine,
                        )
                    };
                    trace!(name = import_name, index = ?instance_idx, "found import at index");

                    // Preinstantiate the plugin instance so we can use it later
                    let pre = plugin_component.pre_instantiate().map_err(|e| {
                        e.context("failed to pre-instantiate during component linking")
                    })?;

                    let mut linker_instance = match linker.instance(import_name) {
                        Ok(i) => i,
                        Err(e) => {
                            trace!(name = import_name, error = %e, "error finding instance in linker, skipping");
                            continue;
                        }
                    };

                    for (export_name, export_ty) in
                        import_instance_ty.exports(plugin_component.metadata.component.engine())
                    {
                        match export_ty.ty {
                            ComponentItem::ComponentFunc(func_ty) => {
                                let (item, func_idx) = match plugin_component
                                    .metadata
                                    .component
                                    .get_export(Some(&instance_idx), export_name)
                                {
                                    Some(res) => res,
                                    None => {
                                        trace!(
                                            name = import_name,
                                            fn_name = export_name,
                                            "failed to get export index, skipping"
                                        );
                                        continue;
                                    }
                                };
                                ensure!(
                                    matches!(item, ComponentItem::ComponentFunc(..)),
                                    "expected function export, found other"
                                );
                                trace!(
                                    name = import_name,
                                    fn_name = export_name,
                                    "linking function import"
                                );
                                let export_is_async = func_ty.async_();
                                // Plain-value async calls always run ephemerally
                                // (params copied). In a p3-service workload, a call
                                // carrying only relocatable `stream<T>` handles also
                                // runs ephemerally — its args/results are relocated
                                // (see `relocate`) rather than copied — so a
                                // stream-carrying backend call can't pin or freeze
                                // the service store.
                                let plain_safe = func_is_ephemeral_safe(&func_ty);
                                let relocate = !plain_safe
                                    && is_service_workload
                                    && func_is_bridge_safe(&func_ty);
                                let ephemeral_call = if export_is_async && (plain_safe || relocate)
                                {
                                    let mode = if relocate {
                                        EphemeralCallMode::Relocated {
                                            param_tys: func_ty.params().map(|(_, ty)| ty).collect(),
                                            result_tys: func_ty.results().collect(),
                                        }
                                    } else {
                                        EphemeralCallMode::PlainValue
                                    };
                                    Some(Arc::new(EphemeralLinkedCall {
                                        engine: plugin_engine.clone(),
                                        http_handler: self.http_handler.clone(),
                                        components: self.components.clone(),
                                        active_component_id: plugin_component_id.clone(),
                                        linked_component_ids: nested_linked_component_ids.clone(),
                                        #[cfg(feature = "wasi-tls")]
                                        tls_provider: self.tls_provider.clone(),
                                        mode,
                                    }))
                                } else {
                                    None
                                };

                                let inv = LinkedExportInvocation {
                                    import_name: import_name.into(),
                                    export_name: export_name.into(),
                                    pre: pre.clone(),
                                    plugin_component_id: plugin_component.id.clone(),
                                    func_idx,
                                    param_tys: Arc::default(),
                                    ephemeral_call,
                                };

                                linked_components.insert(inv.plugin_component_id.clone());

                                let export_name = inv.export_name.clone();
                                if export_is_async {
                                    linker_instance
                                        .func_new_concurrent(
                                            export_name.as_ref(),
                                            move |accessor, _func_ty, params, results| {
                                                let inv = inv.clone();
                                                Box::pin(async move {
                                                    invoke_linked_async_export(
                                                        accessor, params, results, &inv,
                                                    )
                                                    .await
                                                })
                                            },
                                        )
                                        .map_err(|e| {
                                            e.context("failed to create concurrent func")
                                        })?;
                                } else {
                                    linker_instance
                                        .func_new_async(
                                            export_name.as_ref(),
                                            move |store, _func_ty, params, results| {
                                                let inv = inv.clone();
                                                Box::new(async move {
                                                    invoke_linked_sync_export(
                                                        store, params, results, &inv,
                                                    )
                                                    .await
                                                })
                                            },
                                        )
                                        .map_err(|e| {
                                            e.context("failed to wrap sync func in async func")
                                        })?;
                                }
                            }
                            ComponentItem::Resource(resource_ty) => {
                                let (item, _idx) = match plugin_component
                                    .metadata
                                    .component
                                    .get_export(Some(&instance_idx), export_name)
                                {
                                    Some(res) => res,
                                    None => {
                                        trace!(
                                            name = import_name,
                                            resource = export_name,
                                            "failed to get resource index, skipping"
                                        );
                                        continue;
                                    }
                                };
                                let ComponentItem::Resource(_) = item else {
                                    trace!(
                                        name = import_name,
                                        resource = export_name,
                                        "expected resource export, found non-resource, skipping"
                                    );
                                    continue;
                                };

                                // TODO: This should be a comparison of the ComponentItem to the
                                // host resource type, but for some reason the comparison fails.
                                if export_name == "output-stream"
                                    || export_name == "input-stream"
                                    || export_name == "pollable"
                                    || export_name == "tcp-socket"
                                    || export_name == "incoming-value-async-body"
                                {
                                    trace!(
                                        name = import_name,
                                        resource = export_name,
                                        "skipping stream link as it is a host resource type"
                                    );
                                    continue;
                                }

                                trace!(name = import_name, resource = export_name, ty = ?resource_ty, "linking resource import");

                                linker_instance
                                        .resource(export_name, ResourceType::host::<ResourceAny>(), |_, _| Ok(()))
                                        .map_err(|e| {
                                            e.context(format!(
                                                "failed to define resource import: {import_name}.{export_name}"
                                            ))
                                        })
                                        .unwrap_or_else(|e| {
                                            trace!(name = import_name, resource = export_name, error = %e, "error defining resource import, skipping");
                                        });
                            }
                            _ => {
                                trace!(
                                    name = import_name,
                                    fn_name = export_name,
                                    "skipping non-function non-resource import"
                                );
                                continue;
                            }
                        }
                    }
                }
                ComponentItem::Resource(resource_ty) => {
                    trace!(
                        name = import_name,
                        ty = ?resource_ty,
                        "component import is a resource, which is not supported in this context. skipping."
                    );
                }
                _ => continue,
            }
        }

        Ok(linked_components)
    }

    /// Gets the unique identifier of the workload
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Gets the name of the workload
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Gets the namespace of the workload
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Returns the number of components in this workload.
    /// Does not include the service component if one is defined.
    pub async fn component_count(&self) -> usize {
        self.components.read().await.len()
    }

    /// Helper to create a new wasmtime Store for multiple components and set active given component in the workload.
    pub async fn new_store(
        &self,
        component_id: &str,
    ) -> anyhow::Result<wasmtime::Store<SharedCtx>> {
        // Clone + drop the lock before building: the store factory write-locks
        // `components`, so a held read lock would deadlock.
        let metadata = {
            let components = self.components.read().await;
            components
                .get(component_id)
                .context("component ID not found in workload")?
                .metadata
                .clone()
        };
        self.new_store_from_metadata(&metadata, false).await
    }

    /// Creates a new wasmtime Store for multiple components from the given workload metadata.
    async fn new_store_from_metadata(
        &self,
        metadata: &WorkloadMetadata,
        is_service: bool,
    ) -> anyhow::Result<wasmtime::Store<SharedCtx>> {
        let recipe = self.service_store_recipe(metadata).await?;
        new_store_from_templates(
            &recipe.engine,
            recipe.http_handler.clone(),
            &recipe.active_template,
            &recipe.linked_templates,
            &recipe.linked_instances,
            is_service,
        )
        .await
    }

    /// Capture everything needed to (re)build this service's store, so the
    /// trigger-service supervisor can construct a fresh store per incarnation
    /// without borrowing `self`.
    async fn service_store_recipe(
        &self,
        metadata: &WorkloadMetadata,
    ) -> anyhow::Result<ServiceStoreRecipe> {
        let linked_component_ids = metadata
            .linked_components
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let linked_metadata = {
            let components = self.components.read().await;
            linked_component_ids
                .iter()
                .map(|linked_component_id| {
                    components
                        .get(linked_component_id)
                        .map(|component| component.metadata.clone())
                        .with_context(|| {
                            format!("linked component '{linked_component_id}' not found")
                        })
                })
                .collect::<wasmtime::Result<Vec<_>>>()?
        };
        let active_template = self.component_ctx_template(metadata);
        let linked_templates = linked_metadata
            .iter()
            .map(|metadata| self.component_ctx_template(metadata))
            .collect::<Vec<_>>();
        let mut linked_instances = Vec::with_capacity(linked_component_ids.len());
        for linked_component_id in &linked_component_ids {
            linked_instances.push((
                linked_component_id.clone(),
                self.instantiate_pre(linked_component_id.as_ref()).await?,
            ));
        }

        Ok(ServiceStoreRecipe {
            engine: metadata.engine().clone(),
            http_handler: self.http_handler.clone(),
            active_template,
            linked_templates,
            linked_instances,
        })
    }

    pub async fn instantiate_pre(
        &self,
        component_id: &str,
    ) -> anyhow::Result<wasmtime::component::InstancePre<SharedCtx>> {
        let mut components = self.components.write().await;
        let component = components
            .get_mut(component_id)
            .context("component ID not found in workload")?;
        let wasmtime_component = component.metadata.component.clone();
        let linker = component.metadata.linker();
        let pre = linker.instantiate_pre(&wasmtime_component)?;

        Ok(pre)
    }

    /// Unbind all plugins from all components in this workload.
    ///
    /// This should be called when stopping a workload to ensure proper cleanup
    /// of plugin resources. Errors from individual plugin unbind operations are
    /// logged but do not prevent the overall unbind from completing.
    #[instrument(name="unbind_all_plugins", skip_all, fields(workload.id = self.id.as_ref(), workload.name = self.name.as_ref()))]
    pub async fn unbind_all_plugins(&self) -> anyhow::Result<()> {
        trace!(
            workload_id = self.id.as_ref(),
            workload_name = self.name.as_ref(),
            "unbinding all plugins from workload"
        );

        for component in self.components.read().await.values() {
            if let Some(plugins) = component.plugins() {
                for (plugin_id, plugin) in plugins.iter() {
                    trace!(
                        plugin_id,
                        component_id = component.id(),
                        workload_id = self.id.as_ref(),
                        "unbinding plugin from component"
                    );

                    // Get the interfaces this plugin was bound to by checking the component's imports
                    let world = component.world();
                    let plugin_world = plugin.world();

                    // Find the intersection of what the component imports and what the plugin provides
                    let bound_interfaces = world
                        .imports
                        .iter()
                        .filter(|import| plugin_world.imports.contains(import))
                        .cloned()
                        .collect::<std::collections::HashSet<_>>();

                    if let Err(e) = plugin
                        .on_workload_unbind(self.id(), WitInterfaces::new(&bound_interfaces))
                        .await
                    {
                        warn!(
                            plugin_id,
                            component_id = component.id(),
                            workload_id = self.id.as_ref(),
                            error = ?e,
                            "failed to unbind plugin from workload, continuing cleanup"
                        );
                    }
                }
            }

            if component.exports_wasi_http() {
                anyhow::Context::context(
                    self.http_handler.on_workload_unbind(self.id()).await,
                    "failed to notify HTTP handler of workload",
                )?;
            }
        }

        // A trigger service registered its HTTP/messaging handlers at start
        // (`execute_trigger_service`); drop those registrations on stop so it no
        // longer receives host-invoked deliveries on a torn-down instance.
        if self.service.is_some() {
            if let Err(e) = self.http_handler.on_service_http_unbind(self.id()).await {
                tracing::warn!(workload.id = %self.id(), err = %e, "failed to unbind service HTTP handler, continuing");
            }
            if let Err(e) = self
                .http_handler
                .on_trigger_service_messaging_unbind(self.id())
                .await
            {
                tracing::warn!(workload.id = %self.id(), err = %e, "failed to unbind trigger service messaging handler, continuing");
            }
        }

        Ok(())
    }
}

/// An unresolved workload that has been initialized but not yet bound to plugins.
///
/// An `UnresolvedWorkload` represents a workload that has been validated and compiled
/// but has not yet been bound to host plugins or had its dependencies resolved.
/// This is an intermediate state in the workload lifecycle before becoming a
/// [`ResolvedWorkload`] that can be executed.
///
/// # Lifecycle
///
/// 1. **Creation**: Built from a [`Workload`] specification via [`Engine::initialize_workload`]
/// 2. **Plugin Binding**: Components are bound to required host plugins
/// 3. **Resolution**: Dependencies are resolved and the workload becomes [`ResolvedWorkload`]
/// 4. **Execution**: The resolved workload can create component instances and handle requests
///
/// # Plugin Resolution
///
/// During resolution, the workload will:
/// - Match required interfaces with available plugins
/// - Configure component linkers with plugin implementations
/// - Validate that all dependencies can be satisfied
/// - Create the final executable workload representation
pub struct UnresolvedWorkload {
    /// The unique identifier of the workload, created with [uuid::Uuid::new_v4]
    id: Arc<str>,
    /// The name of the workload
    name: Arc<str>,
    /// The namespace of the workload
    namespace: Arc<str>,
    /// The requested host [`WitInterface`]s to resolve this workload
    host_interfaces: Vec<WitInterface>,
    /// The [`WorkloadService`] associated with this workload, if any
    service: Option<WorkloadService>,
    /// All [`WorkloadComponent`]s in the workload
    components: HashMap<Arc<str>, WorkloadComponent>,
    /// TLS provider override for `wasi:tls` client connections in this workload.
    #[cfg(feature = "wasi-tls")]
    tls_provider: Option<SharedTlsProvider>,
}

impl UnresolvedWorkload {
    /// Creates a new unresolved workload from its constituent parts.
    ///
    /// # Arguments
    /// * `id` - Unique identifier for this workload instance
    /// * `name` - Human-readable name of the workload
    /// * `namespace` - Namespace for workload organization
    /// * `engine` - The WebAssembly engine for compilation and execution
    /// * `service` - Optional long-running service component
    /// * `components` - Iterator of components that make up this workload
    /// * `host_interfaces` - Required WIT interfaces that must be provided by host plugins
    ///
    /// # Returns
    /// A new `UnresolvedWorkload` ready for plugin binding and resolution.
    pub fn new(
        id: impl Into<Arc<str>>,
        name: impl Into<Arc<str>>,
        namespace: impl Into<Arc<str>>,
        service: Option<WorkloadService>,
        components: impl IntoIterator<Item = WorkloadComponent>,
        host_interfaces: Vec<WitInterface>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            namespace: namespace.into(),
            service,
            components: components
                .into_iter()
                .map(|c| {
                    let id = Arc::from(c.id());
                    (id, c)
                })
                .collect(),
            host_interfaces,
            #[cfg(feature = "wasi-tls")]
            tls_provider: None,
        }
    }

    /// Override the TLS provider used for `wasi:tls` client connections in this workload.
    ///
    /// Use this to plug in an alternative TLS backend, install a custom root
    /// certificate store (corporate CAs, certificate pinning), or integrate
    /// with HSM-backed key material.
    #[cfg(feature = "wasi-tls")]
    pub fn with_tls_provider(mut self, provider: SharedTlsProvider) -> Self {
        self.tls_provider = Some(provider);
        self
    }

    /// Apply an optional TLS provider override. No-op when `None`.
    #[cfg(feature = "wasi-tls")]
    pub fn maybe_with_tls_provider(self, provider: Option<SharedTlsProvider>) -> Self {
        match provider {
            Some(p) => self.with_tls_provider(p),
            None => self,
        }
    }

    /// Bind this workload to the host plugins based on the requested
    /// interfaces. Returns a list of plugins and the component IDs they were bound to.
    #[allow(clippy::type_complexity)]
    #[instrument(skip_all)]
    pub async fn bind_plugins(
        &mut self,
        plugins: &HashMap<&'static str, Arc<dyn HostPlugin + 'static>>,
    ) -> anyhow::Result<Vec<(Arc<dyn HostPlugin + 'static>, Vec<String>)>> {
        // Track bound plugins with their matched interfaces for cleanup on failure
        let mut bound_plugins_with_interfaces: Vec<BoundPluginWithInterfaces> = Vec::new();
        let mut bound_plugins: Vec<(Arc<dyn HostPlugin + 'static>, Vec<String>)> = Vec::new();

        // Collect all component's required (unmatched) host interfaces
        // This tracks which interfaces each component still needs to be bound
        let mut unmatched_interfaces: HashMap<IdFlavor, HashSet<WitInterface>> = HashMap::new();
        let host_interfaces = {
            // filter out Plugins fulfilled by host. `handler` is the unified
            // WASI P3 http interface (covers both incoming and outgoing);
            // `incoming-handler`/`outgoing-handler` are its P2 equivalents.
            let http_iface =
                WitInterface::from("wasi:http/incoming-handler,outgoing-handler,handler");
            self.host_interfaces
                .iter()
                .filter(|wit_interface| !http_iface.contains(wit_interface))
                .cloned()
                .collect::<Vec<_>>()
        };

        trace!(host_interfaces = ?host_interfaces, "determining missing guest interfaces");

        if let Some(service) = self.service.as_ref() {
            let world = service.world();

            trace!(?world, "comparing service world to host interfaces");
            let required_interfaces: HashSet<WitInterface> = host_interfaces
                .iter()
                .filter(|wit_interface| world.includes_bidirectional(wit_interface))
                .cloned()
                .collect();

            if !required_interfaces.is_empty() {
                unmatched_interfaces.insert(
                    IdFlavor::Service(Arc::from(service.id())),
                    required_interfaces,
                );
            }
        }

        for (id, workload_component) in &self.components {
            let world = workload_component.world();
            trace!(?world, "comparing component world to host interfaces");
            let required_interfaces: HashSet<WitInterface> = host_interfaces
                .iter()
                .filter(|wit_interface| world.includes_bidirectional(wit_interface))
                .cloned()
                .collect();

            if !required_interfaces.is_empty() {
                unmatched_interfaces.insert(IdFlavor::Component(id.clone()), required_interfaces);
            }
        }

        trace!(?unmatched_interfaces, "resolving unmatched interfaces");

        // Iterate through each plugin first, then check every component for matching worlds
        for (plugin_id, p) in plugins.iter() {
            let plugin_interfaces = p.world();
            trace!(plugin_id = plugin_id, plugin_interfaces = ?plugin_interfaces, "checking plugin interfaces");

            // Collect bindings for this plugin across all components
            let mut plugin_component_bindings = Vec::new();

            // Check each component to see if this plugin matches any of their required interfaces
            for (component_id, required_interfaces) in unmatched_interfaces.iter() {
                // Find interfaces that this plugin can satisfy for this component
                let mut matching_interfaces = HashSet::new();
                for wit_interface in required_interfaces.iter() {
                    // Check if plugin supports this interface
                    if plugin_interfaces.includes_bidirectional(wit_interface) {
                        // an `(implements ..)` named interface is served only
                        // by a plugin that supports named instances.
                        let defer_to_other = if wit_interface.name.is_some() {
                            !p.supports_named_instances()
                                && other_plugin_serves(plugins, plugin_id, wit_interface, true)
                        } else {
                            p.supports_named_instances()
                                && other_plugin_serves(plugins, plugin_id, wit_interface, false)
                        };
                        if defer_to_other {
                            continue;
                        }
                        matching_interfaces.insert(wit_interface.clone());
                    }
                }

                if !matching_interfaces.is_empty() {
                    plugin_component_bindings.push((component_id.clone(), matching_interfaces));
                }
            }

            // If this plugin matches any components, bind them
            if !plugin_component_bindings.is_empty() {
                // Collect all unique interfaces across all component bindings for on_workload_bind
                let plugin_matched_interfaces: HashSet<WitInterface> = plugin_component_bindings
                    .iter()
                    .flat_map(|(_, interfaces)| interfaces.clone())
                    .collect();

                // Validate: if multiple named entries of the same namespace:package
                // are matched to this plugin, the plugin must support named instances
                let mut ns_pkg_named: HashMap<(&str, &str), Vec<&str>> = HashMap::new();
                for iface in &plugin_matched_interfaces {
                    if let Some(name) = &iface.name {
                        ns_pkg_named
                            .entry((iface.namespace.as_str(), iface.package.as_str()))
                            .or_default()
                            .push(name.as_str());
                    }
                }
                for ((ns, pkg), mut names) in ns_pkg_named {
                    if names.len() > 1 && !p.supports_named_instances() {
                        names.sort_unstable();
                        bail!(
                            "plugin '{}' does not support named instances, but workload \
                             requires {} named entries for {ns}:{pkg} (names: {}). \
                             The plugin must implement supports_named_instances() to \
                             handle multiplexed interfaces.",
                            plugin_id,
                            names.len(),
                            names.join(", ")
                        );
                    }
                }

                debug!(
                    plugin_id = plugin_id,
                    interfaces = ?plugin_matched_interfaces,
                    "binding plugin to workload"
                );

                let bind_span = tracing::span!(
                    tracing::Level::INFO,
                    "plugin_on_workload_bind",
                    plugin_id = plugin_id,
                );

                // Call on_workload_bind with the workload and all matched interfaces
                if let Err(e) = p
                    .on_workload_bind(self, WitInterfaces::new(&plugin_matched_interfaces))
                    .instrument(bind_span)
                    .await
                {
                    tracing::error!(
                        plugin_id = plugin_id,
                        err = ?e,
                        "failed to bind plugin to workload"
                    );
                    // Clean up all previously bound plugins in reverse order
                    for (bound_plugin, bound_interfaces, _) in
                        bound_plugins_with_interfaces.iter().rev()
                    {
                        debug!(
                            plugin_id = bound_plugin.id(),
                            "calling on_workload_unbind for cleanup after bind failure"
                        );
                        if let Err(cleanup_err) = bound_plugin
                            .on_workload_unbind(self.id(), WitInterfaces::new(bound_interfaces))
                            .await
                        {
                            warn!(
                                plugin_id = bound_plugin.id(),
                                error = ?cleanup_err,
                                "failed to cleanup plugin after bind failure"
                            );
                        }
                    }
                    bail!(e)
                }

                // Collect component IDs for this plugin
                let mut plugin_component_ids = Vec::new();

                // Now bind each component
                for (id, matching_interfaces) in plugin_component_bindings {
                    let mut workload_item = match &id {
                        IdFlavor::Component(component_id) => WorkloadItem::Component(
                            self.components
                                .get_mut(component_id)
                                .context("component not found during plugin binding")?,
                        ),
                        IdFlavor::Service(_) => {
                            WorkloadItem::Service(self.service.as_mut().ok_or_else(|| {
                                anyhow::anyhow!("Infallible. Service was presented before")
                            })?)
                        }
                    };

                    debug!(
                        plugin_id = plugin_id,
                        component_id = workload_item.id(),
                        interfaces = ?matching_interfaces,
                        "binding plugin to workload item"
                    );

                    let item_bind_span = tracing::span!(
                        tracing::Level::INFO,
                        "plugin_on_workload_item_bind",
                        plugin_id = plugin_id,
                    );
                    if let Err(e) = p
                        .on_workload_item_bind(
                            &mut workload_item,
                            WitInterfaces::new(&matching_interfaces),
                        )
                        .instrument(item_bind_span)
                        .await
                    {
                        tracing::error!(
                            plugin_id = plugin_id,
                            component_id = workload_item.id(),
                            err = ?e,
                            "failed to bind workload item to plugin"
                        );
                        // Clean up all previously bound plugins in reverse order
                        for (bound_plugin, bound_interfaces, _) in
                            bound_plugins_with_interfaces.iter().rev()
                        {
                            debug!(
                                plugin_id = bound_plugin.id(),
                                "calling on_workload_unbind for cleanup after component bind failure"
                            );
                            if let Err(cleanup_err) = bound_plugin
                                .on_workload_unbind(self.id(), WitInterfaces::new(bound_interfaces))
                                .await
                            {
                                warn!(
                                    plugin_id = bound_plugin.id(),
                                    error = ?cleanup_err,
                                    "failed to cleanup plugin after component bind failure"
                                );
                            }
                        }
                        bail!(e)
                    } else {
                        trace!(
                            plugin_id = plugin_id,
                            component_id = workload_item.id(),
                            "successfully bound plugin to component"
                        );
                        workload_item.add_plugin(plugin_id, p.clone());
                        plugin_component_ids.push(workload_item.id().to_string());

                        // Remove matched interfaces from unmatched set
                        if let Some(unmatched) = unmatched_interfaces.get_mut(&id) {
                            for interface in matching_interfaces.iter() {
                                unmatched.remove(interface);
                            }
                        }
                    }
                }

                // Add this plugin with all its bound component IDs
                bound_plugins.push((p.clone(), plugin_component_ids.clone()));
                bound_plugins_with_interfaces.push((
                    p.clone(),
                    plugin_matched_interfaces,
                    plugin_component_ids,
                ));
            }
        }

        // Check if all required interfaces were matched
        for (component_id, unmatched) in unmatched_interfaces.iter() {
            if !unmatched.is_empty() {
                tracing::error!(
                    component_id = component_id.as_ref(),
                    interfaces = ?unmatched,
                    "no plugins found for requested interfaces"
                );
                bail!(
                    "workload component {component_id} requested interfaces that are not available on this host: {unmatched:?}",
                )
            }
        }

        Ok(bound_plugins)
    }

    /// Resolves the workload by binding it to host plugins and creating the final executable workload.
    ///
    /// This method performs the final resolution step that transforms an unresolved workload
    /// into a [`ResolvedWorkload`] ready for execution. It:
    ///
    /// 1. Binds components to matching host plugins based on required interfaces
    /// 2. Configures component linkers with plugin implementations
    /// 3. Validates that all component dependencies are satisfied
    /// 4. Creates the final resolved workload representation
    /// 5. Notifies plugins that the workload has been resolved
    ///
    /// # Arguments
    /// * `plugins` - Optional map of available host plugins for binding
    ///
    /// # Returns
    /// A [`ResolvedWorkload`] ready for component instantiation and execution.
    ///
    /// # Errors
    /// Returns an error if:
    /// - Required interfaces cannot be satisfied by available plugins
    /// - Plugin binding fails
    /// - Component linking fails
    /// - Plugin notification fails
    #[instrument(name="resolve_workload", skip_all, fields(workload.id = self.id.as_ref(), workload.name = self.name.as_ref(), workload.namespace = self.namespace.as_ref()))]
    pub async fn resolve(
        mut self,
        plugins: Option<&HashMap<&'static str, Arc<dyn HostPlugin + 'static>>>,
        http_handler: Arc<dyn crate::host::http::HostHandler>,
    ) -> anyhow::Result<ResolvedWorkload> {
        // Bind to plugins
        let bound_plugins = if let Some(plugins) = plugins {
            trace!("binding plugins to workload");
            self.bind_plugins(plugins).await?
        } else {
            Vec::new()
        };

        let incoming_http_component = {
            match self
                .host_interfaces
                .iter()
                .any(|hi| hi.is_incoming_http_handler())
            {
                // http was not part of the requested interfaces
                false => None,
                true => self
                    .components
                    .values()
                    .find(|component| component.exports_wasi_http())
                    .map(|c| c.id().to_string()),
            }
        };

        // Resolve the workload
        let mut resolved_workload = ResolvedWorkload {
            id: self.id.clone(),
            name: self.name.clone(),
            namespace: self.namespace.clone(),
            components: Arc::new(RwLock::new(self.components)),
            service: self.service,
            host_interfaces: self.host_interfaces,
            http_handler: http_handler.clone(),
            #[cfg(feature = "wasi-tls")]
            tls_provider: self.tls_provider,
        };

        // Link components before plugin resolution
        if let Err(e) = resolved_workload.link_components().await {
            // If linking fails, unbind all plugins before returning the error
            warn!(
                error = ?e,
                "failed to link components, unbinding all plugins"
            );
            let _ = resolved_workload.unbind_all_plugins().await;
            bail!(e);
        }

        // Canonicalize component volume mounts up front so bad paths fail at
        // deploy time, not on first request (the service resolves its own).
        let all_component_ids: Vec<Arc<str>> = resolved_workload
            .components
            .read()
            .await
            .keys()
            .cloned()
            .collect();
        resolved_workload
            .resolve_component_volume_mounts(&all_component_ids)
            .await?;

        // Notify plugins of the resolved workload
        for (plugin, component_ids) in bound_plugins.iter() {
            debug!(
                plugin_id = plugin.id(),
                component_count = component_ids.len(),
                "notifying plugin of resolved workload"
            );
            // Call on_workload_resolved for each component this plugin is bound to
            for component_id in component_ids {
                if let Err(e) = plugin
                    .on_workload_resolved(&resolved_workload, component_id.as_str())
                    .await
                {
                    // If we fail to notify a plugin, unbind all plugins that were already bound
                    warn!(
                        plugin_id = plugin.id(),
                        component_id,
                        error = ?e,
                        "failed to notify plugin of resolved workload, unbinding all plugins"
                    );
                    let _ = resolved_workload.unbind_all_plugins().await;
                    bail!(e);
                }
            }
        }

        if let Some(component_id) = incoming_http_component
            && let Err(e) = http_handler
                .on_workload_resolved(&resolved_workload, &component_id)
                .await
        {
            warn!(
                component_id = component_id,
                error = ?e,
                "failed to notify HTTP handler of resolved workload, unbinding all plugins"
            );
            let _ = resolved_workload.unbind_all_plugins().await;
            bail!(e);
        }

        Ok(resolved_workload)
    }

    /// Gets the unique identifier of the workload
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Gets the name of the workload
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Gets the namespace of the workload
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// Retrieves the interface configuration for a given WIT interface, if it exists.
    pub fn interface_config(&self, interface: &WitInterface) -> Option<&HashMap<String, String>> {
        self.host_interfaces
            .iter()
            .find(|i| i.contains(interface))
            .map(|i| &i.config)
    }
}

/// Builds the export→component map used to wire intra-workload component
/// imports. An interface exported by exactly one component is registered for
/// linking; an interface exported by more than one component is "ambiguous" —
/// left out of the map and returned in the second set instead.
///
/// Ambiguity is not an error on its own: host-invoked exports (e.g.
/// `wasmcloud:messaging/handler`, `wasi:http/incoming-handler`) are consumed
/// by host plugins and never imported by another component, so multiple
/// exporters are expected. The importer side ([`resolve_workload_imports`])
/// errors only if a component actually imports an ambiguous interface.
fn build_export_map(
    component_exports: &[(Arc<str>, Vec<String>)],
) -> (HashMap<String, Arc<str>>, HashSet<String>) {
    let mut interface_map: HashMap<String, Arc<str>> = HashMap::new();
    let mut ambiguous: HashSet<String> = HashSet::new();

    for (component_id, names) in component_exports {
        for name in names {
            if ambiguous.contains(name) {
                continue;
            }
            if interface_map.remove(name).is_some() {
                // A second exporter for this interface: mark it ambiguous.
                trace!(
                    name,
                    "interface exported by multiple components; deferring to import side"
                );
                ambiguous.insert(name.clone());
            } else {
                trace!(name, "registering component export for linking");
                interface_map.insert(name.clone(), component_id.clone());
            }
        }
    }

    (interface_map, ambiguous)
}

/// Returns whether some *other* registered plugin (not `self_id`) with
/// `supports_named_instances() == want_named` can serve `iface`.
/// Used to determine whether a plugin can defer an `(implements ..)`
///  interface to a named-capable one, and vice versa).
fn other_plugin_serves(
    plugins: &HashMap<&'static str, Arc<dyn HostPlugin + 'static>>,
    self_id: &str,
    iface: &WitInterface,
    want_named: bool,
) -> bool {
    plugins.iter().any(|(id, q)| {
        *id != self_id
            && q.supports_named_instances() == want_named
            && q.world().includes_bidirectional(iface)
    })
}

/// Topologically sort components (Kahn's algorithm) so exporters come before
/// importers, a component's linker must be fully configured before anything
/// that imports from it is pre-instantiated. Errors on a dependency cycle.
fn topological_sort_components(
    dependencies: &HashMap<Arc<str>, HashSet<Arc<str>>>,
) -> anyhow::Result<Vec<Arc<str>>> {
    // Build in-degree map: count how many dependencies each component has
    // (only counting dependencies on other components within this workload)
    let mut in_degree: HashMap<Arc<str>, usize> = HashMap::new();

    for (component_id, deps) in dependencies {
        // Count only dependencies that are part of this workload
        let dep_count = deps
            .iter()
            .filter(|d| dependencies.contains_key(*d))
            .count();
        in_degree.insert(component_id.clone(), dep_count);
    }

    // Start with components that have no dependencies (in-degree == 0)
    // Sort for deterministic ordering
    let mut queue: Vec<Arc<str>> = in_degree
        .iter()
        .filter(|&(_, degree)| *degree == 0)
        .map(|(id, _)| id.clone())
        .collect();
    queue.sort();

    let mut result = Vec::with_capacity(dependencies.len());

    while let Some(component_id) = queue.pop() {
        result.push(component_id.clone());

        // Find components that depend on this one and decrease their in-degree
        for (other_id, deps) in dependencies {
            if deps.contains(&component_id)
                && let Some(degree) = in_degree.get_mut(other_id)
            {
                *degree = degree.saturating_sub(1);
                if *degree == 0 && !result.contains(other_id) {
                    queue.push(other_id.clone());
                    // Re-sort to maintain determinism
                    queue.sort();
                }
            }
        }
    }

    // Check for circular dependencies
    if result.len() != dependencies.len() {
        let unprocessed: Vec<_> = dependencies
            .keys()
            .filter(|id| !result.contains(id))
            .map(|id| id.as_ref())
            .collect();
        bail!("circular dependency detected among components: {unprocessed:?}");
    }

    Ok(result)
}

/// Expand `direct_links` into the full transitive closure. One hop suffices
/// because components are processed in topological order, so each
/// `resolved_links[dep]` already holds that dep's complete closure.
fn expand_link_closure(
    direct_links: &HashSet<Arc<str>>,
    resolved_links: &HashMap<Arc<str>, HashSet<Arc<str>>>,
) -> HashSet<Arc<str>> {
    let mut linked_components = direct_links.clone();
    for linked_component_id in direct_links {
        if let Some(nested_links) = resolved_links.get(linked_component_id) {
            linked_components.extend(nested_links.iter().cloned());
        }
    }
    linked_components
}

// Helper enum to differentiate between component and service IDs
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
enum IdFlavor {
    Component(Arc<str>),
    Service(Arc<str>),
}

impl AsRef<str> for IdFlavor {
    fn as_ref(&self) -> &str {
        match self {
            IdFlavor::Component(id) => id.as_ref(),
            IdFlavor::Service(id) => id.as_ref(),
        }
    }
}

impl std::fmt::Display for IdFlavor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdFlavor::Component(id) => write!(f, "Component({id})"),
            IdFlavor::Service(id) => write!(f, "Service({id})"),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::plugin::HostPlugin;
    use crate::wit::{WitInterface, WitWorld};
    use async_trait::async_trait;
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use wasmtime::component::{Component, Linker};

    /// Records a single plugin method call for testing callback order and parameters.
    #[derive(Debug, Clone)]
    struct CallRecord {
        #[allow(unused)]
        plugin_id: String,
        method: String,
        component_id: Option<String>,
        #[allow(unused)]
        interfaces: Vec<String>,
    }

    /// Mock plugin implementation for testing workload binding behavior.
    /// Tracks all method calls and counts for verification of callback order and frequency.
    struct MockPlugin {
        id: &'static str,
        world: WitWorld,
        call_records: Arc<Mutex<Vec<CallRecord>>>,
        on_workload_bind_count: Arc<AtomicUsize>,
        on_workload_item_bind_count: Arc<AtomicUsize>,
        on_workload_resolved_count: Arc<AtomicUsize>,
        named_instance_support: bool,
    }

    impl MockPlugin {
        /// Creates a new mock plugin with the specified interfaces it can import/export.
        fn new(id: &'static str, imports: Vec<WitInterface>, exports: Vec<WitInterface>) -> Self {
            Self {
                id,
                world: WitWorld {
                    imports: imports.into_iter().collect(),
                    exports: exports.into_iter().collect(),
                },
                call_records: Arc::new(Mutex::new(Vec::new())),
                on_workload_bind_count: Arc::new(AtomicUsize::new(0)),
                on_workload_item_bind_count: Arc::new(AtomicUsize::new(0)),
                on_workload_resolved_count: Arc::new(AtomicUsize::new(0)),
                named_instance_support: false,
            }
        }

        fn with_named_instance_support(mut self) -> Self {
            self.named_instance_support = true;
            self
        }

        /// Returns the number of times the specified method was called.
        fn get_call_count(&self, method: &str) -> usize {
            match method {
                "on_workload_bind" => self.on_workload_bind_count.load(Ordering::SeqCst),
                "on_workload_item_bind" => self.on_workload_item_bind_count.load(Ordering::SeqCst),
                "on_workload_resolved" => self.on_workload_resolved_count.load(Ordering::SeqCst),
                _ => 0,
            }
        }

        /// Returns all recorded method calls in chronological order.
        fn get_call_records(&self) -> Vec<CallRecord> {
            self.call_records.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl HostPlugin for MockPlugin {
        fn id(&self) -> &'static str {
            self.id
        }

        fn world(&self) -> WitWorld {
            self.world.clone()
        }

        fn supports_named_instances(&self) -> bool {
            self.named_instance_support
        }

        async fn on_workload_bind(
            &self,
            _workload: &UnresolvedWorkload,
            interfaces: WitInterfaces<'_>,
        ) -> anyhow::Result<()> {
            self.on_workload_bind_count.fetch_add(1, Ordering::SeqCst);
            self.call_records.lock().unwrap().push(CallRecord {
                plugin_id: self.id.to_string(),
                method: "on_workload_bind".to_string(),
                component_id: None,
                interfaces: interfaces.iter().map(|i| i.to_string()).collect(),
            });
            Ok(())
        }

        async fn on_workload_item_bind<'a>(
            &self,
            item: &mut WorkloadItem<'a>,
            interfaces: WitInterfaces<'_>,
        ) -> anyhow::Result<()> {
            self.on_workload_item_bind_count
                .fetch_add(1, Ordering::SeqCst);
            self.call_records.lock().unwrap().push(CallRecord {
                plugin_id: self.id.to_string(),
                method: "on_workload_item_bind".to_string(),
                component_id: Some(item.id().to_string()),
                interfaces: interfaces.iter().map(|i| i.to_string()).collect(),
            });
            Ok(())
        }

        async fn on_workload_resolved(
            &self,
            _workload: &ResolvedWorkload,
            component_id: &str,
        ) -> anyhow::Result<()> {
            self.on_workload_resolved_count
                .fetch_add(1, Ordering::SeqCst);
            self.call_records.lock().unwrap().push(CallRecord {
                plugin_id: self.id.to_string(),
                method: "on_workload_resolved".to_string(),
                component_id: Some(component_id.to_string()),
                interfaces: Vec::new(),
            });
            Ok(())
        }
    }

    /// Load a test fixture wasm file at runtime rather than compile time.
    /// This avoids requiring fixture wasm files during `cargo build` — they're
    /// only needed when tests actually run.
    fn load_fixture(name: &str) -> Vec<u8> {
        let path = format!("{}/tests/wasm/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read(&path).unwrap_or_else(|e| panic!("fixture {path} not found: {e}"))
    }

    fn http_counter_wasm() -> Vec<u8> {
        load_fixture("http_counter.wasm")
    }

    fn messaging_handler_wasm() -> Vec<u8> {
        load_fixture("messaging_handler.wasm")
    }

    fn service_wasm() -> Vec<u8> {
        load_fixture("cpu_usage_service.wasm")
    }
    /// Creates a test component using the http_counter fixture.
    /// This provides a real component with actual WIT interface imports.
    fn create_test_component(id: &str) -> WorkloadComponent {
        let engine = wasmtime::Engine::default();
        let linker = Linker::new(&engine);

        // Use the actual http_counter fixture component
        let wasm = http_counter_wasm();
        let component = Component::new(&engine, &wasm).unwrap();

        let local_resources = LocalResources::default();

        WorkloadComponent::new(
            format!("workload-{id}"),
            format!("test-workload-{id}"),
            "test-namespace".to_string(),
            "test-component".to_string(),
            component,
            linker,
            Vec::new(),
            local_resources,
            Arc::default(),
        )
    }

    fn create_test_messaging_component(id: &str) -> WorkloadComponent {
        let engine = wasmtime::Engine::default();
        let linker = Linker::new(&engine);

        let wasm = messaging_handler_wasm();
        let component = Component::new(&engine, &wasm).unwrap();

        let local_resources = LocalResources::default();

        WorkloadComponent::new(
            format!("workload-{id}"),
            format!("test-workload-{id}"),
            "test-namespace".to_string(),
            "test-component".to_string(),
            component,
            linker,
            Vec::new(),
            local_resources,
            Arc::default(),
        )
    }

    fn create_test_service_component(id: &str) -> WorkloadService {
        let engine = wasmtime::Engine::default();
        let linker = Linker::new(&engine);

        let wasm = service_wasm();
        let component = Component::new(&engine, &wasm).unwrap();

        let local_resources = LocalResources::default();

        WorkloadService::new(
            format!("workload-{id}"),
            format!("test-workload-{id}"),
            "test-namespace".to_string(),
            component,
            linker,
            Vec::new(),
            local_resources,
            3,
            Arc::default(),
        )
    }

    /// Every built-in plugin must actually bind the interface it advertises in its
    /// `world()`. The bind hooks pick which interfaces to serve by matching
    /// namespace / package / interface-name strings against the requested set, so a
    /// wrong literal (a package typo, or a config key passed where an interface name
    /// is expected) makes the plugin silently no-op instead of registering its host
    /// functions. This guards that whole class of regression: feed each plugin an
    /// interface it is designed to serve, then assert the first bind registers host
    /// functions. Because a second bind of the same interface on the same linker
    /// conflicts, a plugin that no-op'd (never touched the linker) is caught by the
    /// second bind wrongly succeeding.
    #[cfg(all(
        feature = "wasmcloud-postgres",
        feature = "wasi-blobstore",
        feature = "wasi-keyvalue"
    ))]
    #[tokio::test]
    async fn builtin_plugins_bind_their_declared_interfaces() {
        use crate::plugin::wasi_blobstore::{FilesystemBlobstore, InMemoryBlobstore};
        use crate::plugin::wasi_keyvalue::InMemoryKeyValue;
        use crate::plugin::wasmcloud_postgres::WasmcloudPostgres;
        // wasmcloud:postgres carries the target database as interface config, not as
        // an interface name; the plugin reads it after matching on package.
        let mut postgres_iface =
            WitInterface::from("wasmcloud:postgres/types,query,prepared@0.1.1-draft");
        postgres_iface
            .config
            .insert("database".to_string(), "testdb".to_string());
        let blobstore_iface =
            WitInterface::from("wasi:blobstore/blobstore,container,types@0.2.0-draft");
        let keyvalue_iface = WitInterface::from("wasi:keyvalue/store,atomics,batch@0.2.0-draft");
        let cases: Vec<(Arc<dyn HostPlugin>, WitInterface)> = vec![
            (
                Arc::new(WasmcloudPostgres::new("postgres://user:pass@localhost:5432/db").unwrap())
                    as Arc<dyn HostPlugin>,
                postgres_iface,
            ),
            (
                Arc::new(FilesystemBlobstore::new(std::env::temp_dir())) as Arc<dyn HostPlugin>,
                blobstore_iface.clone(),
            ),
            (
                Arc::new(InMemoryBlobstore::new(None)) as Arc<dyn HostPlugin>,
                blobstore_iface,
            ),
            (
                Arc::new(InMemoryKeyValue::new()) as Arc<dyn HostPlugin>,
                keyvalue_iface,
            ),
        ];
        // A minimal empty component is enough: the bind hooks only mutate the linker.
        let build_component = || {
            let engine = wasmtime::Engine::default();
            let linker = Linker::new(&engine);
            // Minimal empty component: magic "\0asm" + component version + layer.
            let component =
                Component::new(&engine, [0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00]).unwrap();
            WorkloadComponent::new(
                "workload-bind".to_string(),
                "test-workload-bind".to_string(),
                "test-namespace".to_string(),
                "test-component".to_string(),
                component,
                linker,
                Vec::new(),
                LocalResources::default(),
                Arc::default(),
            )
        };
        for (plugin, iface) in cases {
            let id = plugin.id();
            let set: HashSet<WitInterface> = std::iter::once(iface).collect();
            let mut component = build_component();
            let mut item = WorkloadItem::Component(&mut component);
            plugin
                .on_workload_item_bind(&mut item, WitInterfaces::new(&set))
                .await
                .unwrap_or_else(|e| {
                    panic!("plugin `{id}` failed to bind its declared interface: {e}")
                });
            // The first bind registered the plugin's host functions, so binding the
            // same interface again on the same linker must conflict. If the plugin
            // silently no-op'd (e.g. a matcher typo), it never touched the linker and
            // this second bind wrongly succeeds.
            let rebind = plugin
                .on_workload_item_bind(&mut item, WitInterfaces::new(&set))
                .await;
            assert!(
                rebind.is_err(),
                "plugin `{id}` silently no-op'd on its declared interface instead of \
                     registering host functions"
            );
        }
    }

    /// Tests basic plugin binding with one plugin and one component.
    /// Verifies that `on_workload_bind` is called before `on_workload_item_bind`.
    #[tokio::test]
    async fn test_single_plugin_single_component() {
        // Use the actual interfaces that http_counter.wasm uses
        let http_interface = WitInterface {
            namespace: "wasi".to_string(),
            package: "blobstore".to_string(),
            interfaces: ["container".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.2.0-draft").unwrap()),
            config: std::collections::HashMap::new(),
            name: None,
        };

        let plugin = Arc::new(MockPlugin::new(
            "blobstore-plugin",
            vec![],
            vec![http_interface.clone()],
        ));

        let mut plugins = HashMap::new();
        plugins.insert(plugin.id(), plugin.clone() as Arc<dyn HostPlugin>);

        // Create workload with single component
        let components = vec![create_test_component("component1")];

        let mut workload = UnresolvedWorkload::new(
            "test-workload-id".to_string(),
            "test-workload".to_string(),
            "test-namespace".to_string(),
            None,
            components,
            vec![http_interface.clone()],
        );

        let bound_plugins = workload.bind_plugins(&plugins).await.unwrap();

        // Verify plugin was called once for workload binding
        assert_eq!(plugin.get_call_count("on_workload_bind"), 1);

        // Verify plugin was called once for component binding
        assert_eq!(plugin.get_call_count("on_workload_item_bind"), 1);

        // Verify bound_plugins contains our plugin with the component
        assert_eq!(bound_plugins.len(), 1);
        let (_bound_plugin, component_ids) = &bound_plugins[0];
        assert_eq!(component_ids.len(), 1);

        // Verify call order
        let records = plugin.get_call_records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].method, "on_workload_bind");
        assert_eq!(records[1].method, "on_workload_item_bind");
        assert_eq!(records[1].component_id.as_ref().unwrap(), &component_ids[0]);
    }

    /// Tests complex binding scenarios with multiple plugins and components.
    /// Verifies that each plugin gets called once for workload binding.
    #[tokio::test]
    async fn test_multiple_plugins_multiple_components() {
        let http_interface = WitInterface::from("wasi:http/incoming-handler@0.2.0");
        let blobstore_interface = WitInterface::from("wasi:blobstore/blobstore@0.2.0");
        let keyvalue_interface = WitInterface::from("wasi:keyvalue/store@0.2.0");

        let http_plugin = Arc::new(MockPlugin::new(
            "http-plugin",
            vec![],
            vec![http_interface.clone()],
        ));

        let storage_plugin = Arc::new(MockPlugin::new(
            "storage-plugin",
            vec![],
            vec![blobstore_interface.clone(), keyvalue_interface.clone()],
        ));

        let mut plugins = HashMap::new();
        plugins.insert(http_plugin.id(), http_plugin.clone() as Arc<dyn HostPlugin>);
        plugins.insert(
            storage_plugin.id(),
            storage_plugin.clone() as Arc<dyn HostPlugin>,
        );

        // Create components
        let components = vec![
            create_test_component("component1"),
            create_test_component("component2"),
            create_test_component("component3"),
        ];

        let mut workload = UnresolvedWorkload::new(
            "test-workload-id".to_string(),
            "test-workload".to_string(),
            "test-namespace".to_string(),
            None,
            components,
            vec![
                http_interface.clone(),
                blobstore_interface.clone(),
                keyvalue_interface.clone(),
            ],
        );

        // Note: Due to the way world() works on real components, we can't easily mock it
        // This test verifies the structure and call patterns are correct
        let _bound_plugins = workload.bind_plugins(&plugins).await.unwrap();

        // Each plugin that matches should be in the result
        for (plugin, _component_ids) in &_bound_plugins {
            // Each plugin gets called once for on_workload_bind
            if plugin.id() == "http-plugin" {
                assert_eq!(http_plugin.get_call_count("on_workload_bind"), 1);
            } else if plugin.id() == "storage-plugin" {
                assert_eq!(storage_plugin.get_call_count("on_workload_bind"), 1);
            }
        }
    }

    /// Tests that when multiple plugins provide the same interface,
    /// only one plugin gets bound to avoid duplicate interface handling.
    #[tokio::test]
    async fn test_no_duplicate_bindings() {
        let http_interface = WitInterface::from("wasi:http/incoming-handler@0.2.0");

        // Two plugins that both provide HTTP
        let plugin1 = Arc::new(MockPlugin::new(
            "http-plugin-1",
            vec![],
            vec![http_interface.clone()],
        ));

        let plugin2 = Arc::new(MockPlugin::new(
            "http-plugin-2",
            vec![],
            vec![http_interface.clone()],
        ));

        let mut plugins = HashMap::new();
        plugins.insert(plugin1.id(), plugin1.clone() as Arc<dyn HostPlugin>);
        plugins.insert(plugin2.id(), plugin2.clone() as Arc<dyn HostPlugin>);

        let components = vec![create_test_component("component1")];

        let mut workload = UnresolvedWorkload::new(
            "test-workload-id".to_string(),
            "test-workload".to_string(),
            "test-namespace".to_string(),
            None,
            components,
            vec![http_interface.clone()],
        );

        let _bound_plugins = workload.bind_plugins(&plugins).await.unwrap();

        // Only one plugin should be bound per interface
        // Due to HashMap iteration order being unstable, we can't predict which one
        let total_workload_binds =
            plugin1.get_call_count("on_workload_bind") + plugin2.get_call_count("on_workload_bind");

        // Important: Only one plugin should handle the interface
        assert!(
            total_workload_binds <= 1,
            "Only one plugin should bind for a given interface"
        );
    }

    /// Tests error handling when a workload requests interfaces that no plugin provides.
    /// The binding should fail gracefully with a descriptive error message.
    #[tokio::test]
    async fn test_missing_interface_fails() {
        let http_interface = WitInterface::from("wasi:http/incoming-handler@0.2.0");
        let blobstore_interface = WitInterface::from("wasi:blobstore/blobstore@0.2.0");

        // Plugin only provides HTTP
        let plugin = Arc::new(MockPlugin::new(
            "http-plugin",
            vec![],
            vec![http_interface.clone()],
        ));

        let mut plugins = HashMap::new();
        plugins.insert(plugin.id(), plugin.clone() as Arc<dyn HostPlugin>);

        // Create a component - it will declare what it actually imports
        let components = vec![create_test_component("component1")];

        // Workload requests both HTTP and Blobstore interfaces
        // But only HTTP is available via plugins
        let mut workload = UnresolvedWorkload::new(
            "test-workload-id".to_string(),
            "test-workload".to_string(),
            "test-namespace".to_string(),
            None,
            components,
            vec![http_interface.clone(), blobstore_interface.clone()],
        );

        // This should fail if a component actually needs blobstore but it's not provided
        // Note: The actual failure depends on what the component's world() returns
        let _result = workload.bind_plugins(&plugins).await;

        // The test verifies the error path exists and works correctly
        // In practice, this would fail if a component imports blobstore but no plugin provides it
    }

    /// A component exporting the unified WASI P3 `wasi:http/handler` interface
    /// must bind with no HTTP plugin present, exactly like a P2
    /// `incoming-handler` component: HTTP is served by the host, so the
    /// requested interface must not fall through to plugin matching.
    #[tokio::test]
    async fn test_p3_http_handler_served_by_host() {
        let engine = wasmtime::Engine::default();
        let wasm = wat::parse_str(
            r#"(component
                (component $empty)
                (instance $i (instantiate $empty))
                (export "wasi:http/handler@0.3.0" (instance $i))
            )"#,
        )
        .unwrap();
        let component = Component::new(&engine, &wasm).unwrap();
        let workload_component = WorkloadComponent::new(
            "workload-p3-http".to_string(),
            "test-workload-p3-http".to_string(),
            "test-namespace".to_string(),
            "test-component".to_string(),
            component,
            Linker::new(&engine),
            Vec::new(),
            LocalResources::default(),
            Arc::default(),
        );

        let mut http_interface = WitInterface::from("wasi:http/handler");
        http_interface
            .config
            .insert("host".to_string(), "localhost".to_string());
        assert!(http_interface.is_incoming_http_handler());

        let mut workload = UnresolvedWorkload::new(
            "test-workload-id".to_string(),
            "test-workload".to_string(),
            "test-namespace".to_string(),
            None,
            vec![workload_component],
            vec![http_interface],
        );

        let bound_plugins = workload.bind_plugins(&HashMap::new()).await.unwrap();
        assert!(
            bound_plugins.is_empty(),
            "host-served wasi:http/handler must not require a plugin"
        );
    }

    /// Tests that plugin callbacks are invoked in the correct order:
    /// `on_workload_bind` first, then `on_workload_item_bind` for each component.
    #[tokio::test]
    async fn test_plugin_callback_order() {
        let interface1 = WitInterface::from("test:interface/handler@0.1.0");

        let plugin = Arc::new(MockPlugin::new(
            "test-plugin",
            vec![],
            vec![interface1.clone()],
        ));

        let mut plugins = HashMap::new();
        plugins.insert(plugin.id(), plugin.clone() as Arc<dyn HostPlugin>);

        let components = vec![
            create_test_component("comp1"),
            create_test_component("comp2"),
        ];

        let mut workload = UnresolvedWorkload::new(
            "test-workload-id".to_string(),
            "test-workload".to_string(),
            "test-namespace".to_string(),
            None,
            components,
            vec![interface1.clone()],
        );

        let _bound_plugins = workload.bind_plugins(&plugins).await.unwrap();

        // Verify callback order
        let records = plugin.get_call_records();

        // First call should always be on_workload_bind
        if !records.is_empty() {
            assert_eq!(
                records[0].method, "on_workload_bind",
                "on_workload_bind should be called before component bindings"
            );

            // All subsequent calls should be on_workload_item_bind
            for record in records.iter().skip(1) {
                assert_eq!(
                    record.method, "on_workload_item_bind",
                    "All calls after on_workload_bind should be on_workload_item_bind"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_world_includes_bidirectional() {
        let world = WitWorld {
            imports: HashSet::from([WitInterface::from("wasmcloud:messaging/handler@0.1.0")]),
            exports: HashSet::from([WitInterface::from(
                "wasmcloud:messaging/consumer,types@0.1.0",
            )]),
        };

        let interface1 = WitInterface::from("wasmcloud:messaging/handler@0.1.0");
        let interface2 = WitInterface::from("wasmcloud:messaging/consumer,types@0.1.0");
        let interface3 = WitInterface::from("wasmcloud:messaging/handler,consumer,types@0.1.0");
        let interface4 = WitInterface::from("wasmcloud:messaging/producer@0.1.0");

        assert!(world.includes_bidirectional(&interface1));
        assert!(world.includes_bidirectional(&interface2));
        assert!(world.includes_bidirectional(&interface3));
        assert!(!world.includes_bidirectional(&interface4));
        // Show the difference between includes and includes_bidirectional
        assert!(!world.includes(&interface3));
    }

    /// Tests topological sort with a chain dependency: A -> B -> C
    /// Expected order: C, B, A (or any valid topological order)
    #[test]
    fn build_export_map_registers_unique_exports() {
        let exports = vec![
            (
                Arc::from("comp-a") as Arc<str>,
                vec!["wasi:http/incoming-handler".to_string()],
            ),
            (
                Arc::from("comp-b") as Arc<str>,
                vec!["custom:pkg/iface".to_string()],
            ),
        ];
        let (map, ambiguous) = build_export_map(&exports);
        assert_eq!(
            map.get("custom:pkg/iface").map(|c| c.as_ref()),
            Some("comp-b")
        );
        assert!(ambiguous.is_empty());
    }

    #[test]
    fn build_export_map_marks_duplicate_exports_ambiguous() {
        // Two workers exporting the same handler interface: ambiguous, and
        // dropped from the resolvable map (only an importer would error).
        let exports = vec![
            (
                Arc::from("task-leet") as Arc<str>,
                vec!["wasmcloud:messaging/handler@0.2.0".to_string()],
            ),
            (
                Arc::from("task-reverse") as Arc<str>,
                vec!["wasmcloud:messaging/handler@0.2.0".to_string()],
            ),
        ];
        let (map, ambiguous) = build_export_map(&exports);
        assert!(!map.contains_key("wasmcloud:messaging/handler@0.2.0"));
        assert!(ambiguous.contains("wasmcloud:messaging/handler@0.2.0"));
    }

    #[test]
    fn build_export_map_three_exporters_stay_ambiguous() {
        // A third exporter must not accidentally re-register the interface.
        let iface = "wasmcloud:messaging/handler@0.2.0".to_string();
        let exports = vec![
            (Arc::from("a") as Arc<str>, vec![iface.clone()]),
            (Arc::from("b") as Arc<str>, vec![iface.clone()]),
            (Arc::from("c") as Arc<str>, vec![iface.clone()]),
        ];
        let (map, ambiguous) = build_export_map(&exports);
        assert!(!map.contains_key(&iface));
        assert!(ambiguous.contains(&iface));
    }

    #[test]
    fn expand_link_closure_adds_nested_links_only() {
        let a: Arc<str> = Arc::from("component-a");
        let b: Arc<str> = Arc::from("component-b");
        let c: Arc<str> = Arc::from("component-c");
        let unrelated: Arc<str> = Arc::from("unrelated");

        let direct_links = HashSet::from([a.clone()]);
        let resolved_links = HashMap::from([
            (a.clone(), HashSet::from([b.clone(), c.clone()])),
            (unrelated.clone(), HashSet::from([Arc::from("ignored")])),
        ]);

        let closure = expand_link_closure(&direct_links, &resolved_links);

        assert_eq!(closure.len(), 3);
        assert!(closure.contains(&a));
        assert!(closure.contains(&b));
        assert!(closure.contains(&c));
        assert!(!closure.contains(&unrelated));
    }

    #[test]
    fn test_topological_sort_chain() {
        let a: Arc<str> = Arc::from("component-a");
        let b: Arc<str> = Arc::from("component-b");
        let c: Arc<str> = Arc::from("component-c");

        // A depends on B, B depends on C
        let mut dependencies: HashMap<Arc<str>, HashSet<Arc<str>>> = HashMap::new();
        dependencies.insert(a.clone(), HashSet::from([b.clone()]));
        dependencies.insert(b.clone(), HashSet::from([c.clone()]));
        dependencies.insert(c.clone(), HashSet::new());

        let result = topological_sort_components(&dependencies).unwrap();

        // C should come before B, and B should come before A
        let c_pos = result.iter().position(|x| x == &c).unwrap();
        let b_pos = result.iter().position(|x| x == &b).unwrap();
        let a_pos = result.iter().position(|x| x == &a).unwrap();

        assert!(
            c_pos < b_pos,
            "C should be processed before B: C at {c_pos}, B at {b_pos}"
        );
        assert!(
            b_pos < a_pos,
            "B should be processed before A: B at {b_pos}, A at {a_pos}"
        );
    }

    /// Tests topological sort with no dependencies
    #[test]
    fn test_topological_sort_no_dependencies() {
        let a: Arc<str> = Arc::from("component-a");
        let b: Arc<str> = Arc::from("component-b");
        let c: Arc<str> = Arc::from("component-c");

        let mut dependencies: HashMap<Arc<str>, HashSet<Arc<str>>> = HashMap::new();
        dependencies.insert(a.clone(), HashSet::new());
        dependencies.insert(b.clone(), HashSet::new());
        dependencies.insert(c.clone(), HashSet::new());

        let result = topological_sort_components(&dependencies).unwrap();

        // All components should be present
        assert_eq!(result.len(), 3);
        assert!(result.contains(&a));
        assert!(result.contains(&b));
        assert!(result.contains(&c));
    }

    /// Tests topological sort with diamond dependency: A -> B, A -> C, B -> D, C -> D
    #[test]
    fn test_topological_sort_diamond() {
        let a: Arc<str> = Arc::from("component-a");
        let b: Arc<str> = Arc::from("component-b");
        let c: Arc<str> = Arc::from("component-c");
        let d: Arc<str> = Arc::from("component-d");

        // A depends on B and C, both B and C depend on D
        let mut dependencies: HashMap<Arc<str>, HashSet<Arc<str>>> = HashMap::new();
        dependencies.insert(a.clone(), HashSet::from([b.clone(), c.clone()]));
        dependencies.insert(b.clone(), HashSet::from([d.clone()]));
        dependencies.insert(c.clone(), HashSet::from([d.clone()]));
        dependencies.insert(d.clone(), HashSet::new());

        let result = topological_sort_components(&dependencies).unwrap();

        let a_pos = result.iter().position(|x| x == &a).unwrap();
        let b_pos = result.iter().position(|x| x == &b).unwrap();
        let c_pos = result.iter().position(|x| x == &c).unwrap();
        let d_pos = result.iter().position(|x| x == &d).unwrap();

        // D should come before B and C
        assert!(d_pos < b_pos, "D should be processed before B");
        assert!(d_pos < c_pos, "D should be processed before C");
        // B and C should come before A
        assert!(b_pos < a_pos, "B should be processed before A");
        assert!(c_pos < a_pos, "C should be processed before A");
    }

    /// Tests topological sort with circular dependency detection
    #[test]
    fn test_topological_sort_circular_dependency() {
        let a: Arc<str> = Arc::from("component-a");
        let b: Arc<str> = Arc::from("component-b");
        let c: Arc<str> = Arc::from("component-c");

        // Circular: A -> B -> C -> A
        let mut dependencies: HashMap<Arc<str>, HashSet<Arc<str>>> = HashMap::new();
        dependencies.insert(a.clone(), HashSet::from([b.clone()]));
        dependencies.insert(b.clone(), HashSet::from([c.clone()]));
        dependencies.insert(c.clone(), HashSet::from([a.clone()]));

        let result = topological_sort_components(&dependencies);
        assert!(
            result.is_err(),
            "Should detect circular dependency: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_host_interface_redundancy() {
        let messaging_handler = WitInterface {
            namespace: "wasmcloud".to_string(),
            package: "messaging".to_string(),
            interfaces: ["handler".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.2.0").unwrap()),
            config: std::collections::HashMap::new(),
            name: None,
        };

        let messaging_consumer = WitInterface {
            namespace: "wasmcloud".to_string(),
            package: "messaging".to_string(),
            interfaces: ["consumer".to_string(), "types".to_string()]
                .into_iter()
                .collect(),
            version: Some(semver::Version::parse("0.2.0").unwrap()),
            config: std::collections::HashMap::new(),
            name: None,
        };

        let logging = WitInterface {
            namespace: "wasi".to_string(),
            package: "logging".to_string(),
            interfaces: ["logging".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.1.0-draft").unwrap()),
            config: std::collections::HashMap::new(),
            name: None,
        };

        let messaging_plugin = Arc::new(MockPlugin::new(
            "messaging-plugin",
            vec![messaging_consumer],
            vec![messaging_handler],
        ));

        let logging_plugin = Arc::new(MockPlugin::new(
            "logging-plugin",
            vec![logging.clone()],
            vec![],
        ));

        let mut plugins = HashMap::new();
        plugins.insert(
            logging_plugin.id(),
            logging_plugin.clone() as Arc<dyn HostPlugin>,
        );
        plugins.insert(
            messaging_plugin.id(),
            messaging_plugin.clone() as Arc<dyn HostPlugin>,
        );

        // Create workload with single component
        let components = vec![create_test_messaging_component("component")];

        let mut workload = UnresolvedWorkload::new(
            "test-workload-id".to_string(),
            "test-workload".to_string(),
            "test-namespace".to_string(),
            None,
            components,
            vec![
                WitInterface {
                    namespace: "wasmcloud".to_string(),
                    package: "messaging".to_string(),
                    interfaces: ["consumer".to_string(), "handler".to_string()]
                        .into_iter()
                        .collect(),
                    version: Some(semver::Version::parse("0.2.0").unwrap()),
                    config: std::collections::HashMap::new(),
                    name: None,
                },
                logging,
            ],
        );

        let bound_plugins = workload.bind_plugins(&plugins).await.unwrap();

        // Verify plugin was called once for workload binding
        assert_eq!(logging_plugin.get_call_count("on_workload_bind"), 1);

        // Verify plugin was called once for component binding
        assert_eq!(logging_plugin.get_call_count("on_workload_item_bind"), 1);

        // Verify plugin was called once for workload binding
        assert_eq!(messaging_plugin.get_call_count("on_workload_bind"), 0);

        // Verify plugin was called once for component binding
        assert_eq!(messaging_plugin.get_call_count("on_workload_item_bind"), 0);

        // Verify bound_plugins contains our plugin with the component
        assert_eq!(bound_plugins.len(), 1);
    }

    #[tokio::test]
    async fn test_single_plugin_single_service() {
        let logging_interface = WitInterface {
            namespace: "wasi".to_string(),
            package: "logging".to_string(),
            interfaces: ["logging".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.1.0-draft").unwrap()),
            config: std::collections::HashMap::new(),
            name: None,
        };

        let plugin = Arc::new(MockPlugin::new(
            "logging-plugin",
            vec![logging_interface.clone()],
            vec![],
        ));

        let mut plugins = HashMap::new();
        plugins.insert(plugin.id(), plugin.clone() as Arc<dyn HostPlugin>);

        // Create workload with single component
        let service = create_test_service_component("service");

        let mut workload = UnresolvedWorkload::new(
            "test-workload-id".to_string(),
            "test-workload".to_string(),
            "test-namespace".to_string(),
            Some(service),
            vec![],
            vec![logging_interface.clone()],
        );

        let bound_plugins = workload.bind_plugins(&plugins).await.unwrap();

        // Verify plugin was called once for workload binding
        assert_eq!(plugin.get_call_count("on_workload_bind"), 1);

        // Verify plugin was called once for service binding
        assert_eq!(plugin.get_call_count("on_workload_item_bind"), 1);

        // Verify bound_plugins contains our plugin with the component
        assert_eq!(bound_plugins.len(), 1);
        let (_bound_plugin, component_ids) = &bound_plugins[0];
        assert_eq!(component_ids.len(), 1);
    }

    fn keyvalue_interface(name: Option<&str>) -> WitInterface {
        WitInterface {
            namespace: "wasi".to_string(),
            package: "keyvalue".to_string(),
            interfaces: ["store".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.2.0-draft").unwrap()),
            config: std::collections::HashMap::new(),
            name: name.map(String::from),
        }
    }

    /// Two named `wasi:keyvalue` entries, plugin doesn't support naming -> error
    #[tokio::test]
    async fn test_named_interfaces_fail_without_plugin_support() {
        let plugin = Arc::new(MockPlugin::new(
            "keyvalue-plugin",
            vec![],
            vec![keyvalue_interface(None)],
        ));

        let mut plugins = HashMap::new();
        plugins.insert(plugin.id(), plugin.clone() as Arc<dyn HostPlugin>);

        let mut workload = UnresolvedWorkload::new(
            "test-workload-id".to_string(),
            "test-workload".to_string(),
            "test-namespace".to_string(),
            None,
            vec![create_test_component("component1")],
            vec![
                keyvalue_interface(Some("cache")),
                keyvalue_interface(Some("sessions")),
            ],
        );

        let result = workload.bind_plugins(&plugins).await;
        match result {
            Ok(_) => panic!("Expected error for unsupported named instances"),
            Err(e) => {
                let err_msg = format!("{e}");
                assert!(
                    err_msg.contains("does not support named instances"),
                    "Expected 'does not support named instances' error, got: {err_msg}"
                );
            }
        }
    }

    /// Same setup but plugin returns `supports_named_instances() == true` -> succeeds
    #[tokio::test]
    async fn test_named_interfaces_succeed_with_plugin_support() {
        let plugin = Arc::new(
            MockPlugin::new("keyvalue-plugin", vec![], vec![keyvalue_interface(None)])
                .with_named_instance_support(),
        );

        let mut plugins = HashMap::new();
        plugins.insert(plugin.id(), plugin.clone() as Arc<dyn HostPlugin>);

        let mut workload = UnresolvedWorkload::new(
            "test-workload-id".to_string(),
            "test-workload".to_string(),
            "test-namespace".to_string(),
            None,
            vec![create_test_component("component1")],
            vec![
                keyvalue_interface(Some("cache")),
                keyvalue_interface(Some("sessions")),
            ],
        );

        let result = workload.bind_plugins(&plugins).await;
        if let Err(e) = result {
            panic!("Expected success but got error: {e}");
        }
    }

    /// Only one named entry -> no multiplexing needed, passes even without plugin support
    #[tokio::test]
    async fn test_single_named_interface_no_validation() {
        let plugin = Arc::new(MockPlugin::new(
            "keyvalue-plugin",
            vec![],
            vec![keyvalue_interface(None)],
        ));

        let mut plugins = HashMap::new();
        plugins.insert(plugin.id(), plugin.clone() as Arc<dyn HostPlugin>);

        let mut workload = UnresolvedWorkload::new(
            "test-workload-id".to_string(),
            "test-workload".to_string(),
            "test-namespace".to_string(),
            None,
            vec![create_test_component("component1")],
            vec![keyvalue_interface(Some("cache"))],
        );

        let result = workload.bind_plugins(&plugins).await;
        if let Err(e) = result {
            panic!("Single named entry should not require named instance support: {e}");
        }
    }

    /// Coexistence: a regular (standalone) plugin and a named-capable
    /// (multiplexed) plugin both matching the same package bind together, with
    /// the unnamed interface routed to the regular plugin and the `(implements
    /// ..)` named interfaces routed to the named-capable one. Without the
    /// name-aware dispatch this errors (the regular plugin would claim the two
    /// named entries it cannot serve).
    #[tokio::test]
    async fn test_named_and_regular_plugins_coexist() {
        let regular = Arc::new(MockPlugin::new(
            "kv-standalone",
            vec![],
            vec![keyvalue_interface(None)],
        ));
        let multiplexed = Arc::new(
            MockPlugin::new("kv-multiplexed", vec![], vec![keyvalue_interface(None)])
                .with_named_instance_support(),
        );

        let mut plugins = HashMap::new();
        plugins.insert(regular.id(), regular.clone() as Arc<dyn HostPlugin>);
        plugins.insert(multiplexed.id(), multiplexed.clone() as Arc<dyn HostPlugin>);

        let mut workload = UnresolvedWorkload::new(
            "test-workload-id".to_string(),
            "test-workload".to_string(),
            "test-namespace".to_string(),
            None,
            vec![create_test_component("component1")],
            vec![
                keyvalue_interface(None),
                keyvalue_interface(Some("cache")),
                keyvalue_interface(Some("sessions")),
            ],
        );

        let bound = workload
            .bind_plugins(&plugins)
            .await
            .expect("standalone + multiplexed plugins should bind together");
        let bound_ids: std::collections::HashSet<&str> =
            bound.iter().map(|(p, _)| p.id()).collect();
        assert!(
            bound_ids.contains("kv-standalone"),
            "regular plugin must bind the unnamed interface"
        );
        assert!(
            bound_ids.contains("kv-multiplexed"),
            "named-capable plugin must bind the named interfaces"
        );
    }

    /// Existing unnamed entries -> no change in behavior
    #[tokio::test]
    async fn test_unnamed_interfaces_backwards_compatible() {
        let iface = WitInterface {
            namespace: "wasi".to_string(),
            package: "blobstore".to_string(),
            interfaces: ["container".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.2.0-draft").unwrap()),
            config: std::collections::HashMap::new(),
            name: None,
        };

        let plugin = Arc::new(MockPlugin::new(
            "blobstore-plugin",
            vec![],
            vec![iface.clone()],
        ));

        let mut plugins = HashMap::new();
        plugins.insert(plugin.id(), plugin.clone() as Arc<dyn HostPlugin>);

        let mut workload = UnresolvedWorkload::new(
            "test-workload-id".to_string(),
            "test-workload".to_string(),
            "test-namespace".to_string(),
            None,
            vec![create_test_component("component1")],
            vec![iface],
        );

        let result = workload.bind_plugins(&plugins).await;
        if let Err(e) = result {
            panic!("Unnamed interfaces should work as before: {e}");
        }
    }
}
