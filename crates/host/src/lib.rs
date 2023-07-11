//! wasmCloud host library

#![warn(clippy::pedantic)]
#![warn(missing_docs)]
#![forbid(clippy::unwrap_used)]

/// local lattice
pub mod local;

/// wasmbus lattice
pub mod wasmbus;

pub use local::{Lattice as LocalLattice, LatticeConfig as LocalLatticeConfig};
pub use wasmbus::{Lattice as WasmbusLattice, LatticeConfig as WasmbusLatticeConfig};

pub use url;

use anyhow::Context as _;

#[cfg(unix)]
fn socket_pair() -> anyhow::Result<(tokio::net::UnixStream, tokio::net::UnixStream)> {
    tokio::net::UnixStream::pair().context("failed to create an unnamed unix socket pair")
}

#[cfg(windows)]
fn socket_pair() -> anyhow::Result<(tokio::io::DuplexStream, tokio::io::DuplexStream)> {
    Ok(tokio::io::duplex(8196))
}
