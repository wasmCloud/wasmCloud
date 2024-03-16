![Crates.io](https://img.shields.io/crates/v/wasmcloud-control-interface)
[![Documentation](https://img.shields.io/badge/Docs-Documentation-blue)](https://wasmcloud.dev)
[![Rustdocs](https://docs.rs/wasmcloud-control-interface/badge.svg)](https://docs.rs/wasmcloud-control-interface)

# wasmCloud Control Interface Client

This library is a convenient API for interacting with the lattice control interface. This is a Rust crate that implements the [lattice control protocol](https://wasmcloud.dev/reference/lattice-protocols/control-interface/) as described in the wasmCloud reference documentation. For a formal definition of the interface protocol, you can also look at the **Smithy** files in the [interface repository](https://github.com/wasmCloud/interfaces/blob/main/lattice-control/lattice-control-interface.smithy).

The lattice control interface provides a way for clients to interact with the lattice to issue control commands and queries. This interface is a message broker protocol that supports functionality like starting and stopping components and providers, declaring link definitions, monitoring lattice events, holding auctions to determine scheduling compatibility, and much more.
