//! A crate that implements the functionality behind the wasmCloud shell
//!
//! The `wash` command line interface <https://github.com/wasmcloud/wash> is a great place
//! to find examples on how to fully utilize this library.

#[cfg(feature = "start")]
pub mod start;

#[cfg(feature = "parser")]
pub mod parser;

#[cfg(feature = "cli")]
pub mod build;
#[cfg(feature = "cli")]
pub mod cli;

pub mod config;
pub mod context;
pub mod drain;
pub mod id;
pub mod keys;
pub mod registry;
