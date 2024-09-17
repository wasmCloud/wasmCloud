#![forbid(clippy::unwrap_used)]

pub mod logging;
pub mod nats;
pub mod tls;

pub mod host;
pub use host::*;

pub mod link;
pub use link::*;

pub mod otel;
pub use otel::*;

#[cfg(feature = "oci")]
pub mod oci;
#[cfg(feature = "oci")]
pub use oci::*;

pub mod par;
pub use par::*;

pub mod registry;
pub use registry::*;

pub mod rpc;
pub use rpc::*;

pub mod secrets;

pub mod wit;
pub use wit::*;

/// The 1.0 version of the wasmCloud control API, used in topic strings for the control API
pub const CTL_API_VERSION_1: &str = "v1";

/// Identifier of one or more entities on the lattice used for addressing. May take many forms, such as:
/// - component public key
/// - provider public key
/// - opaque string
pub type LatticeTarget = String;

/// Identifier of a component which sends invocations on the lattice
pub type ComponentId = String;

/// Name of a link on the wasmCloud lattice
pub type LinkName = String;

/// Public key (nkey) of a cluster issuer
pub type ClusterIssuerKey = String;

/// WIT package for a given operation (ex. `keyvalue` in `wasi:keyvalue/readwrite.get`)
pub type WitPackage = String;

/// WIT namespace for a given operation (ex. `wasi` in `wasi:keyvalue/readwrite.get`)
pub type WitNamespace = String;

/// WIT interface for a given operation (ex. `readwrite` in `wasi:keyvalue/readwrite.get`)
pub type WitInterface = String;

/// A WIT function (ex. `get` in `wasi:keyvalue/readwrite.get`)
pub type WitFunction = String;

/// The name of a known (possibly pre-created) configuration, normally used when creating
/// new interface links in order to configure one or both source/target
pub type KnownConfigName = String;

/// Trait describing types/entities that can be health-checked
pub trait HealthCheck {
    // This might not work with codegen and we'll have to impl
    fn health_request(&self) -> HealthCheckResponse;
}
