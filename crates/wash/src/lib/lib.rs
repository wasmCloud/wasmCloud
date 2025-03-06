//! A crate that implements the functionality behind the wasmCloud shell
//!
//! The `wash` command line interface <https://github.com/wasmCloud/wasmCloud/tree/main/crates/wash > is a great place
//! to find examples on how to fully utilize this library.
//!
//! This library contains a few feature flags, most enabled by default but optional in order to
//! allow consumers to omit some functionality. This is especially useful when considering compiling this
//! library to restrictive targets, e.g. `wasm32-unknown-unknown` or `wasm32-wasi`. Support for `wasm` targets
//! is a goal but has not been tested yet.
//!
//! | Feature Name | Default Enabled | Description |
//! | --- | --- | --- |
//! | start | true | Contains the [start] module, with utilities to start wasmCloud runtimes, NATS, and wadm |
//! | parser | true | Contains the [parser] module, with utilities to parse `wasmcloud.toml` files |
//! | cli | false | Contains the build, cli, and generate modules with additional trait derives for usage in building CLI applications |
//! | nats| true| Contains the [app], [component], [capture], [config], [context], [drain], [spier] and [wait] modules with a dependency on `async_nats` |

#[cfg(feature = "nats")]
pub mod app;
#[cfg(feature = "cli")]
pub mod build;
#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "cli")]
pub mod deps;
#[cfg(feature = "cli")]
pub mod generate;
#[cfg(feature = "parser")]
pub mod parser;
#[cfg(feature = "start")]
pub mod start;

#[cfg(feature = "nats")]
pub mod capture;
pub mod common;
#[cfg(feature = "nats")]
pub mod component;
#[cfg(feature = "nats")]
pub mod config;
#[cfg(feature = "nats")]
pub mod context;
#[cfg(feature = "nats")]
pub mod drain;
pub mod id;
pub mod keys;
pub mod registry;
#[cfg(feature = "nats")]
pub mod spier;
#[cfg(feature = "nats")]
pub mod wait;

#[cfg(feature = "plugin")]
pub mod plugin;
