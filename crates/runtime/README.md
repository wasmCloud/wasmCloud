# wasmCloud Runtime

wasmCloud is an open source Cloud Native Computing Foundation (CNCF) project that enables teams to build, manage, and scale polyglot Wasm apps across any cloud, K8s, or edge.

⚠️ This crate is highly experimental and likely to experience breaking changes frequently. The runtime itself is relatively stable, but the APIs and public members of this crate are not guaranteed to be stable.

## Usage

This crate can be used to embed a wasmCloud runtime in a Rust application. You can refer to the [wasmcloud-host](https://crates.io/crates/wasmcloud-host) crate for an example of how to use the runtime, generally it's recommended to use the host crate instead for embedding in an application as this crate is lower level.
