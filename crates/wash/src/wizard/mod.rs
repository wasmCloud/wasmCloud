//! The `wash wizard` picker: choose a workload architecture and scaffold it.
//! Architectures are not compiled in: [`index`] clones the templates repo into
//! the cache and derives every project's shape from its source on the spot.

pub mod builder;
pub mod generate;
pub mod index;
pub mod picker;
/// Generating a capability rather than a workload: a plugin plus a consumer.
pub mod plugin;
pub mod reverse;

/// Re-exported so every caller draws a topology the same way.
pub use wash_topology::{render, style};
pub mod spec;
pub mod stubs;

pub use generate::generate;
pub use index::{Index, Source};
pub use spec::{Capability, Linking, Spec, Trigger};
pub use wash_topology::diagram;
