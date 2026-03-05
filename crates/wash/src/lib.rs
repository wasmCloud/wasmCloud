#![doc = include_str!("../../../README.md")]

/// The current version of the wash package, set at build time
pub const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Command line interface implementations for wash
pub mod cli;
/// Configuration management for wash
pub mod config;
/// Component inspection and analysis
pub mod inspect;
/// Create new wash projects
pub mod new;
/// Manage WebAssembly Interface Types (WIT) for wash components
pub(crate) mod wit;
