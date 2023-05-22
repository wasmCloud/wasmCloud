# wasmCloud Wit Interfaces
This directory contains wasmCloud's well-known capability interfaces, expressed in the `wit` syntax. The contents of this directory provide a single source of truth for wasmCloud interface definitions and provide a place to track their progress as they converge to WASI cloud standards.

🏆 The ultimate goal of these interfaces is to allow someone to simply shop around for the characteristics of the runtime(s) they want, and not have to worry about C FFI or bizarre interop scenarios or worse, porting from one language to another. WebAssembly components should eventually fade into the background as a boring implementation detail.

## Phase 1
Phase one of the interface migration involves migrating the service and type definitions from Smithy to the `wit` syntax. 

Interfaces in this phase are to stay as close to the original wasmCloud interfaces as possible, with certain exceptions where there are large gaps between wasmCloud and WASI standards (e.g. `messaging`).

## Phase 2
In phase 2, the wit syntax stored here will be dropped wherever possible, and replaced entirely with the use of "off the shelf" WASI cloud standards. Only in the case where there is no corresponding WASI standard or the underlying WASI standard cannot meet our needs will we retain a wasmCloud-specific interface.

## Interface List
The following is a list of interfaces that we plan to migrate to `wit` syntax.

In the following list, the `Phase` refers to which phase of _our_ migration and implementation the interface is in. This shouldn't be confused with a WASI standards acceptance phase.

### Core Interfaces
The following are considered core or "first party" supported interfaces.

| wasmCloud Interface | Phase | WASI Std | Description |
|:-:|:-:|--|--|
| N/A | 1 | [wasi-logging](https://github.com/WebAssembly/wasi-logging) | Standard level-oriented logging |
| `messaging` | 1 | [wasi-messaging](https://github.com/WebAssembly/wasi-messaging) | Interact with message brokers like NATS or RabbitMQ |
| `blobstore` | 1 | [wasi-blob-store](https://github.com/WebAssembly/wasi-blob-store) | Interact with blob stores, which could abstract over a file system |
| N/A | N/A | [wasi-filesystem](https://github.com/WebAssembly/wasi-filesystem) | File system abstraction. We will likely not have a high-level interface for this, it should come "for free" with wasmtime |


### Additional Interfaces
The following are additional interfaces less frequently used in simple applications or interfaces that have no correlation to WASI cloud specifications.


| Interface | Phase | Description |
|--|:-:|--|
| `lattice-control` | _Not Started_ | Interact with the wasmCloud control interface |
| `ml` | _Not Started_ | Perform machine learning functions |
| `sensors` | _Not Started_ | Receive streaming data from sensors |
| `config-service` | _Not Started_ | Interact with a wasmCloud configuration service |

### Community Interfaces
Community interfaces will be tracked outside of this `wit` directory.
