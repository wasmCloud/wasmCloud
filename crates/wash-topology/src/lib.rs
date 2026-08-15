//! Workload topology: the schema, the derivation that produces one from a
//! project on disk, and the renderer that draws it. Shared by `wash` and
//! `xtask`; topologies are always derived, never stored in a project.

pub mod catalog;
pub mod derive;
pub mod render;
pub mod schema;
pub mod style;

pub use derive::derive;
pub use render::diagram;
pub use schema::*;
