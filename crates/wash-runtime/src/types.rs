//! Types used throughout the wasmcloud crate for workload management and host operations.
//!
//! This module contains two main categories of types:
//!
//! ## Public API Types (used in [`crate::host::HostApi`])
//! - Request/Response types: [`WorkloadStartRequest`], [`WorkloadStartResponse`],
//!   [`WorkloadStatusRequest`], [`WorkloadStatusResponse`],
//!   [`WorkloadStopRequest`], [`WorkloadStopResponse`]
//! - Host information: [`HostHeartbeat`]
//!
//! ## Core Workload Types (used internally)
//! - Workload definition: [`Workload`], [`WorkloadState`], [`WorkloadStatus`]
//! - Component configuration: [`Component`], [`Service`], [`LocalResources`]
//! - Volume management: [`Volume`], [`VolumeType`], [`VolumeMount`],
//!   [`EmptyDirVolume`], [`HostPathVolume`]

use bytes::Bytes;
use std::{collections::HashMap, sync::Arc};

use crate::wit::WitInterface;

/// Represents a deployable workload containing one or more WebAssembly components.
/// A workload defines the complete runtime configuration including components,
/// services, interfaces, and volumes.
#[derive(Debug, Clone, PartialEq)]
pub struct Workload {
    pub namespace: String,
    pub name: String,
    pub annotations: HashMap<String, String>,
    pub service: Option<Service>,
    pub components: Vec<Component>,
    pub host_interfaces: Vec<WitInterface>,
    pub volumes: Vec<Volume>,
}

/// The current state of a workload in its lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(i32)]
pub enum WorkloadState {
    Unspecified = 0,
    Starting = 1,
    Running = 2,
    Completed = 3,
    Stopping = 4,
    Error = 5,
    NotFound = 6,
}

/// Configuration for a long-running service component that handles requests.
/// Services can be restarted if they fail and have resource limits.
#[derive(Debug, Clone, PartialEq)]
pub struct Service {
    pub bytes: Bytes,
    pub digest: Option<String>,
    pub local_resources: LocalResources,
    pub max_restarts: u64,
}

/// A WebAssembly component that can be executed as part of a workload.
/// Components can be pooled for concurrent execution and have invocation limits.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Component {
    pub name: String,
    pub bytes: Bytes,
    pub digest: Option<String>,
    pub local_resources: LocalResources,
    pub pool_size: i32,
    pub max_invocations: i32,
}

/// Resource limits and configuration for a component or service.
/// Defines memory, CPU limits, configuration values, and volume mounts.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalResources {
    pub memory_limit_mb: i32,
    pub cpu_limit: i32,
    /// Opaque key-value configuration shared between operator + runtime + plugins.
    /// Allows passing arbitrary configuration values to influence implementation behavior for all component interfaces.
    /// Example: tracing=disable
    pub config: HashMap<String, String>,
    // wasi:cli/env variables, copied to WasiCtxBuilder
    pub environment: HashMap<String, String>,
    pub volume_mounts: Vec<VolumeMount>,
    pub allowed_hosts: Arc<[String]>,
}

impl Default for LocalResources {
    fn default() -> Self {
        Self {
            memory_limit_mb: -1,
            cpu_limit: -1,
            config: HashMap::new(),
            environment: HashMap::new(),
            volume_mounts: Vec::new(),
            allowed_hosts: Default::default(),
        }
    }
}

/// A named volume that can be mounted into components.
#[derive(Debug, Clone, PartialEq)]
pub struct Volume {
    pub name: String,
    pub volume_type: VolumeType,
}

/// The type of volume - either host path or empty directory.
#[derive(Debug, Clone, PartialEq)]
pub enum VolumeType {
    HostPath(HostPathVolume),
    EmptyDir(EmptyDirVolume),
}

/// Describes how a volume should be mounted into a component.
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeMount {
    pub name: String,
    pub mount_path: String,
    pub read_only: bool,
}

/// An ephemeral empty directory volume that exists for the lifetime of the workload.
#[derive(Debug, Clone, PartialEq)]
pub struct EmptyDirVolume {}

/// A volume that mounts a directory from the host filesystem.
#[derive(Debug, Clone, PartialEq)]
pub struct HostPathVolume {
    pub local_path: String,
}

/// Information about the host's current state and capabilities.
/// Returned by [`crate::host::HostApi::heartbeat`].
#[derive(Debug, Clone, PartialEq)]
pub struct HostHeartbeat {
    pub id: String,
    pub hostname: String,
    pub http_port: u16,
    pub friendly_name: String,
    pub version: String,
    pub labels: HashMap<String, String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub os_arch: String,
    pub os_name: String,
    pub os_kernel: String,
    /// System CPU usage in percent (0.0 - 100.0)
    pub system_cpu_usage: f32,
    /// System total memory in bytes
    pub system_memory_total: u64,
    /// System free memory in bytes
    pub system_memory_free: u64,
    pub component_count: u64,
    pub workload_count: u64,
    pub imports: Vec<WitInterface>,
    pub exports: Vec<WitInterface>,
}

/// Status information about a workload including its ID, state, and any messages.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadStatus {
    pub workload_id: String,
    pub workload_state: WorkloadState,
    pub message: String,
}

/// Request to start a new workload on the host.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadStartRequest {
    pub workload_id: String,
    pub workload: Workload,
}

/// Response after attempting to start a workload.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadStartResponse {
    pub workload_status: WorkloadStatus,
}

/// Request to get the status of a specific workload.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadStatusRequest {
    pub workload_id: String,
}

/// Response containing the status of a requested workload.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadStatusResponse {
    pub workload_status: WorkloadStatus,
}

/// Request to stop a running workload.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadStopRequest {
    pub workload_id: String,
}

/// Response after attempting to stop a workload.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadStopResponse {
    pub workload_status: WorkloadStatus,
}
