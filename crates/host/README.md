# wasmCloud Host

wasmCloud is an open source Cloud Native Computing Foundation (CNCF) project that enables teams to build, manage, and scale polyglot Wasm apps across any cloud, K8s, or edge.

⚠️ This crate is highly experimental and likely to experience breaking changes frequently. The host itself is relatively stable, but the APIs and public members of this crate are not guaranteed to be stable.

## Usage

This crate can be used to embed a wasmCloud host in a Rust application. You can refer to the [main.rs](https://github.com/wasmCloud/wasmCloud/blob/main/src/main.rs) file of the wasmCloud runtime for an example of this.

This library provides the host runtime for wasmCloud, which is a platform for running WebAssembly (Wasm) components and plugins.
It includes pluggable support for various extensions and integrations, allowing developers to customize and extend the host's functionality.

## Key Modules

- **[event](./src/event.rs)**: Provides the `crate::event::EventManager` trait for handling and dispatching events from the wasmCloud host
- **[metrics](./src/metrics.rs)**: Implements OpenTelemetry metrics for wasmCloud, primarily using the `crate::metrics` module for tracing and monitoring.
- **[nats](./src/nats.rs)**: Contains the NATS-based implementations for the wasmCloud host extension traits
- **[oci](./src/oci.rs)**: Offers configuration and utilities for fetching OCI (Open Container Initiative) artifacts
- **[policy](./src/policy.rs)**: Defines the `crate::policy::PolicyManager` trait for applying additional security policies on top of the wasmCloud host.
- **[registry](./src/registry.rs)**: Provides the `crate::registry::RegistryCredentialExt` extension trait for working with registry credentials and configurations.
- **[secrets](./src/secrets.rs)**: Contains the `crate::secrets::SecretsManager` trait for securely fetching secrets from a secret store.
- **[store](./src/store.rs)**: Defines the `crate::store::StoreManager` trait for managing configuration and data from a backing store.
- **[wasmbus](./src/wasmbus.rs)**: Contains the core implementation of the wasmCloud host functionality, including the `crate::wasmbus::Host` struct and related configurations.
- **[workload_identity](./src/workload_identity.rs)**: Experimental module for workload identity implementations, providing tools for identity management.

## Extending the Host

The top-level modules in this crate expose implementable extension traits that allow developers to extend the host's functionality. These traits can be supplied to an embedded host using the `crate::wasmbus::HostBuilder`.

For example, you can implement custom policies, secrets management, or registry configurations to tailor the host to your specific needs.

## Getting Started

To get started with wasmCloud, refer to [`wasmbus`](./src/wasmbus/mod.rs) for the core host functionality. From there, you can explore the other modules to add extensions and integrations as needed.

For more information, visit the [wasmCloud homepage](https://wasmcloud.com).
