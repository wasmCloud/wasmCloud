// Adapted from
// https://github.com/wasmCloud/wasmcloud-otp/blob/5f13500646d9e077afa1fca67a3fe9c8df5f3381/host_core/native/hostcore_wasmcloud_native/src/oci.rs

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Configuration options for OCI operations.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Config {
    /// Additional CAs to include in the OCI client configuration
    pub additional_ca_paths: Vec<PathBuf>,
    /// Whether or not to allow downloading OCI artifacts with the tag `latest`
    pub allow_latest: bool,
    /// A list of OCI registries that are allowed to be accessed over HTTP
    pub allowed_insecure: Vec<String>,
    /// Used in tandem with `oci_user` and `oci_password` to override credentials for a specific OCI registry.
    pub oci_registry: Option<String>,
    /// Username for the OCI registry specified by `oci_registry`.
    pub oci_user: Option<String>,
    /// Password for the OCI registry specified by `oci_registry`.
    pub oci_password: Option<String>,
}
