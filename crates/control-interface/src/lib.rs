//! # Control Interface Client
//!
//! This library provides a client API for consuming the [wasmCloud control interface][docs-control-interface]
//! over a NATS connection.
//!
//! This library can be used by multiple types of tools, and is also used
//! by the control interface capability provider and the [`wash` CLI][wash].
//!
//! ## Usage
//!
//! All of the [`Client`] functions are handled by a wasmCloud host running in the specified lattice.
//!
//! Each function returns a `Result<CtlResponse<T>>` wrapper around the actual response type. The outer
//! result should be handled for protocol (timeouts, no hosts available) and deserialization errors (invalid response payload).
//! The inner result is the actual response from the host(s) and should be handled for application-level errors.
//!
//! [docs-control-interface]: <https://wasmcloud.com/docs/hosts/lattice-protocols/control-interface>
//! [wash]: <https://wasmcloud.com/docs/ecosystem/wash>

use serde::{Deserialize, Serialize};

mod broker;
mod otel;

pub mod client;
pub use client::{Client, ClientBuilder};

mod types;
pub use types::component::*;
pub use types::ctl::*;
pub use types::host::*;
pub use types::link::*;
pub use types::provider::*;
pub use types::registry::*;
pub use types::rpc::*;

// NOTE(brooksmtownsend): These are included to avoid a major breaking change
// in this crate by removing the public type aliases. They should be removed
// when we release 3.0.0 of this crate.
#[deprecated(
    since = "2.3.0",
    note = "String type aliases are deprecated, use Strings instead"
)]
#[allow(dead_code)]
mod aliases {
    type ComponentId = String;
    type KnownConfigName = String;
    type LatticeTarget = String;
    type LinkName = String;
    type WitInterface = String;
    type WitNamespace = String;
    type WitPackage = String;
}
#[allow(unused_imports)]
#[allow(deprecated)]
pub use aliases::*;

/// Generic result
type Result<T> = ::core::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Enumeration of all kinds of identifiers in a wasmCloud lattice
#[allow(unused)]
pub(crate) enum IdentifierKind {
    /// Identifiers that are host IDs
    HostId,
    /// Identifiers that are component IDs
    ComponentId,
    /// Identifiers that are component references
    ComponentRefs,
    /// Identifiers that are provider reverences
    ProviderRef,
    /// Identifiers that are link names
    LinkName,
}

impl IdentifierKind {
    /// Ensure an identifier is a valid as a host ID
    fn is_host_id(value: impl AsRef<str>) -> Result<String> {
        assert_non_empty_string(value, "Host ID cannot be empty")
    }

    /// Ensure an identifier is a valid as a component ID
    fn is_component_id(value: impl AsRef<str>) -> Result<String> {
        assert_non_empty_string(value, "Component ID cannot be empty")
    }

    /// Ensure an identifier is a valid as a component reference
    fn is_component_ref(value: impl AsRef<str>) -> Result<String> {
        assert_non_empty_string(value, "Component OCI reference cannot be empty")
    }

    /// Ensure an identifier is a valid as a provider reference
    fn is_provider_ref(value: impl AsRef<str>) -> Result<String> {
        assert_non_empty_string(value, "Provider OCI reference cannot be empty")
    }

    /// Ensure an identifier is a valid as a provider reference
    fn is_provider_id(value: impl AsRef<str>) -> Result<String> {
        assert_non_empty_string(value, "Provider ID cannot be empty")
    }

    /// Ensure an identifier is a valid as a link name
    fn is_link_name(value: impl AsRef<str>) -> Result<String> {
        assert_non_empty_string(value, "Link Name cannot be empty")
    }
}

/// Helper function that serializes the data and maps the error
pub(crate) fn json_serialize<T>(item: T) -> Result<Vec<u8>>
where
    T: Serialize,
{
    serde_json::to_vec(&item).map_err(|e| format!("JSON serialization failure: {e}").into())
}

/// Helper function that deserializes the data and maps the error
pub(crate) fn json_deserialize<'de, T: Deserialize<'de>>(buf: &'de [u8]) -> Result<T> {
    serde_json::from_slice(buf).map_err(|e| format!("JSON deserialization failure: {e}").into())
}

/// Check that a likely user-provided string is non empty
fn assert_non_empty_string(input: impl AsRef<str>, message: impl AsRef<str>) -> Result<String> {
    let input = input.as_ref();
    let message = message.as_ref();
    if input.trim().is_empty() {
        Err(message.into())
    } else {
        Ok(input.trim().to_string())
    }
}
