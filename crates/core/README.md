# `wasmcloud-core`

This repository contains the core data types, traits and reusable functionality to enable the [wasmCloud][wasmCloud] ecosystem.

This crate provides:

- Utilities for dealing with WIT types
- Types used across wasmCloud Hosts
- Types used in linking on wasmCloud lattices
- Logging types
- Utilities for interacting with NATS
- ... and more

This crate is meant to be used by programs, utilities and infrastructure targeting the wasmCloud platform.

[wasmCloud]: https://wasmcloud.com

## Installation

To use `wasmcloud-core` in your project, you can add it via `cargo add`:

```console
cargo add wasmcloud-core
```

Or include the following in your `Cargo.toml`:

```toml
wasmcloud-core = "0.7.0"
```

## Features

`wasmcloud-core` comes with the following features:

| Feature             | Default? | Description                                                                             |
|---------------------|----------|-----------------------------------------------------------------------------------------|
| hyper-rustls        | yes      | Enable [`hyper-rustls`][hyper-rustls] usage (see `tls` module)                          |
| oci                 | yes      | Enable [`oci-distribution`][oci-distribution] and [`oci-wasm`] usage (see `tls` module) |
| reqwest             | yes      | Enable [`reqwest`][request] extensions (see `tls` module)                               |
| rustls-native-certs | yes      | Enable [`rustls-native-certs`][rustls-native-certs] (see `tls` module)                  |
| webpki-roots        | yes      | Enable [`webpki-roots`][webpki-roots] (see `tls` module)                                |
| otel                | no       | Enable [OpenTelemetry][otel] module support                                             |

[hyper-rustls]: https://crates.io/crates/hyper-rustls
[oci-distribution]: https://crates.io/crates/oci-distribution
[reqwest]: https://crates.io/crates/reqwest
[rustls-native-certs]: https://crates.io/crates/rustls-native-certs
[webpki-roots]: https://crates.io/crates/webpki-roots
[otel]: https://opentelemetry.io/

## Using `wasmcloud-core`

`wasmcloud-core` does not provide a `prelude`, but instead exports types as needed under appropriate modules.

Import the needed types and traits as necessary from your project similarly to the following:

```rust
use wasmcloud_core::nats::convert_header_map_to_hashmap;
use wasmcloud_core::rpc::{health_subject, link_del_subject, link_put_subject, shutdown_subject};
use wasmcloud_core::{
    HealthCheckRequest, HealthCheckResponse, HostData, InterfaceLinkDefinition, LatticeTarget,
};
```

## Contributing

Have a change that belongs be in `wasmcloud-core`? Please feel free to [file an issue](https://github.com/wasmCloud/wasmCloud/issues/new/choose) and/or join us on the [wasmCloud slack](https://slack.wasmcloud.com)!
