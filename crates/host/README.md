# wasmCloud Host

wasmCloud is an open source Cloud Native Computing Foundation (CNCF) project that enables teams to build, manage, and scale polyglot Wasm apps across any cloud, K8s, or edge.

⚠️ This crate is highly experimental and likely to experience breaking changes frequently. The host itself is relatively stable, but the APIs and public members of this crate are not guaranteed to be stable.

## Usage

This crate can be used to embed a wasmCloud host in a Rust application. You can refer to the [wasmcloud-main-rs] file of the wasmCloud runtime for an example of this.

This library provides the host runtime for wasmCloud, which is a platform for running WebAssembly (Wasm) components and plugins.
It includes pluggable support for various extensions and integrations, allowing developers to customize and extend the host's functionality.

## Key Modules

- **[config]**: Implements the `crate::config::ConfigManager` trait for managing a configuration store that can be watched for updates. This is a supertrait of `crate::store::StoreManager` and is implemented by `crate::store::DefaultStore`.
- **[event]**: Provides the `crate::event::EventManager` trait for handling and dispatching events from the wasmCloud host
- **[metrics]**: Implements OpenTelemetry metrics for wasmCloud, primarily using the `crate::metrics` module for tracing and monitoring.
- **[nats]**: Contains the NATS-based implementations for the wasmCloud host extension traits
- **[oci]**: Offers configuration and utilities for fetching OCI (Open Container Initiative) artifacts
- **[policy]**: Defines the `crate::policy::PolicyManager` trait for applying additional security policies on top of the wasmCloud host.
- **[registry]**: Provides the `crate::registry::RegistryCredentialExt` extension trait for working with registry credentials and configurations.
- **[secrets]**: Contains the `crate::secrets::SecretsManager` trait for securely fetching secrets from a secret store.
- **[store]**: Defines the `crate::store::StoreManager` trait for managing configuration and data from a backing store.
- **[wasmbus]**: Contains the core implementation of the wasmCloud host functionality, including the `crate::wasmbus::Host` struct and related configurations.
- **[workload_identity]**: Experimental module for workload identity implementations, providing tools for identity management.

## Extending the Host

The top-level modules in this crate expose implementable extension traits that allow developers to extend the host's functionality. These traits can be supplied to an embedded host using the `crate::wasmbus::HostBuilder`.

For example, you can implement custom policies, secrets management, or registry configurations to tailor the host to your specific needs.

The wasmCloud [wasmcloud-main-rs] binary uses the implementations in [nats] to provide a NATS-based host runtime. This allows you to run wasmCloud components and plugins in a distributed environment, leveraging NATS for messaging and communication.

## Getting Started

To get started with wasmCloud, refer to [wasmbus] for the core host functionality. From there, you can explore the other modules to add extensions and integrations as needed.

For more information, visit the [wasmcloud-homepage].

<!-- Links used multiple times -->

[wasmcloud-main-rs]: https://github.com/wasmCloud/wasmCloud/blob/main/src/main.rs
[wasmcloud-homepage]: https://wasmcloud.com
