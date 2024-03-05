//! # Control Interface Client
//!
//! This library provides a client API for consuming the wasmCloud control interface over a
//! NATS connection. This library can be used by multiple types of tools, and is also used
//! by the control interface capability provider and the wash CLI
//!
//! ## Usage
//! All of the [Client] functions are handled by a wasmCloud host running in the specified lattice.
//! Each function returns a Result<CtlResponse<T>> wrapper around the actual response type. The outer
//! result should be handled for protocol (timeouts, no hosts available) and deserialization errors (invalid response payload).
//! The inner result is the actual response from the host(s) and should be handled for application-level errors.

use serde::{Deserialize, Serialize};

mod broker;
mod otel;

pub mod client;
pub use client::{collect_sub_timeout, Client, ClientBuilder};

mod types;
pub use types::actor::*;
pub use types::ctl::*;
pub use types::host::*;
pub use types::link::InterfaceLinkDefinition;
pub use types::provider::*;
pub use types::registry::*;
pub use types::rpc::*;

/// Identifier of one or more entities on the lattice used for addressing. May take many forms, such as:
/// - actor public key
/// - provider public key
/// - opaque string
pub type LatticeTarget = String;

/// Identifier of a component which sends invocations on the lattice
pub type ComponentId = String;

/// Name of a link on the wasmCloud lattice
pub type LinkName = String;

/// WIT package for a given operation (ex. `keyvalue` in `wasi:keyvalue/readwrite.get`)
pub type WitPackage = String;

/// WIT namespace for a given operation (ex. `wasi` in `wasi:keyvalue/readwrite.get`)
pub type WitNamespace = String;

/// WIT interface for a given operation (ex. `readwrite` in `wasi:keyvalue/readwrite.get`)
pub type WitInterface = String;

/// The name of a known (possibly pre-created) configuration, normally used when creating
/// new interface links in order to configure one or both source/target
pub type KnownConfigName = String;

/// Generic result
type Result<T> = ::std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

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

/// Enumeration of all kinds of identifiers in a wasmCloud lattice
enum IdentifierKind {
    HostId,
    ComponentId,
    ActorRef,
    ProviderRef,
    LinkName,
}

//NOTE(ahmedtadde): For an initial implementation, we just want to make sure that the identifier is, at very least, not an empty string.
//This parser should be refined over time as needed.
fn parse_identifier<T: AsRef<str>>(kind: &IdentifierKind, value: T) -> Result<String> {
    let value = value.as_ref();
    match kind {
        IdentifierKind::HostId => assert_non_empty_string(value, "Host ID cannot be empty"),
        IdentifierKind::ComponentId => {
            assert_non_empty_string(value, "Component ID cannot be empty")
        }
        IdentifierKind::ActorRef => {
            assert_non_empty_string(value, "Actor OCI reference cannot be empty")
        }
        IdentifierKind::ProviderRef => {
            assert_non_empty_string(value, "Provider OCI reference cannot be empty")
        }
        IdentifierKind::LinkName => assert_non_empty_string(value, "Link Name cannot be empty"),
    }
}
