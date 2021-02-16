![Crates.io](https://img.shields.io/crates/v/wasmcloud-control-interface)
![Rust Build](https://img.shields.io/github/workflow/status/wasmcloud/wasmcloud/WASMCLOUD-CONTROL-INTERFACE/main)
[![Documentation](https://img.shields.io/badge/Docs-Documentation-blue)](https://wasmcloud.dev)
![Rustdocs](https://docs.rs/wasmcloud-host/badge.svg)

# wasmCloud Control Interface
This library is a convenient API for interacting with the lattice control interface.

The lattice control interface provides a way for clients to interact with the lattice to issue control commands and queries. This interface is a message broker protocol that supports functionality like starting and stopping actors and providers, declaring link definitions, performing function calls on actors, monitoring lattice events, holding auctions to determine scheduling compatibility, and much more.