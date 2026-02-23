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

use std::future::Future;
use std::path::PathBuf;
use std::{collections::HashMap, path::Path};

use crate::engine::workload::WorkloadItem;
use crate::{
    engine::workload::{ResolvedWorkload, UnresolvedWorkload, WorkloadComponent},
    wit::WitWorld,
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

pub mod wasmcloud_messaging;

#[cfg(all(feature = "wasi-webgpu", not(target_os = "windows")))]
pub mod wasi_webgpu;

#[cfg(feature = "wrpc")]
pub mod wrpc;

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

    /// Returns whether this plugin can dynamically handle the given interface.
    ///
    /// This is checked in addition to the static [`HostPlugin::world`] matching.
    /// Plugins that handle arbitrary interfaces (e.g., wrpc bridging) can override
    /// this to match based on interface configuration rather than a fixed world.
    ///
    /// # Returns
    /// `true` if this plugin can handle the interface, `false` otherwise.
    fn can_handle(&self, _interface: &crate::wit::WitInterface) -> bool {
        false
    }

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
        _interfaces: std::collections::HashSet<crate::wit::WitInterface>,
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
        _interfaces: std::collections::HashSet<crate::wit::WitInterface>,
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
        _interfaces: std::collections::HashSet<crate::wit::WitInterface>,
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

    pub fn add_component(&mut self, workload_component: &WorkloadComponent, data: Y) {
        let component_id = workload_component.id();
        let workload_id = workload_component.workload_id();
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
}

/// Locks an untrusted path to be within the given root directory.
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
