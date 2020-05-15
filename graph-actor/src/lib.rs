#![doc(html_logo_url = "https://avatars0.githubusercontent.com/u/52050279?s=200&v=4")]
//! # waSCC Graph Database Actor API
//!
//! The WebAssembly Secure Capabilities Connector (waSCC) API for Graph Database actors
//! enables actors to communicate with graph capability providers in a secure, loosely-coupled
//! fashion.
//!
//! For examples and tutorials on using the actor APIs, check out [wascc.dev](https://wascc.dev).
extern crate wasccgraph_common as common;

pub use common::{FromTable, GraphResult};

pub mod graph;
