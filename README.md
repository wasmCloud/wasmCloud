
# wasmCloud Control Interface

This library is a convenient API for interacting with the lattice control interface.

The lattice control interface provides a way for clients to interact with the lattice to issue control commands and queries. This interface is a message broker protocol that supports functionality like starting and stopping actors and providers, declaring link definitions, monitoring lattice events, holding auctions to determine scheduling compatibility, and much more.

## Temporary location

This repository is a temporary location for this crate, which used to be
in
github.com/wasmcloud/wasmcloud/crates/wasmcloud-control-interface.

The plan is to move this crate back into the wasmcloud host repo once
version 0.50 is stabilized and merged.
