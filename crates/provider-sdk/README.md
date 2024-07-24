# wasmCloud Provider SDK

Providers are swappable [wasmCloud](https://wasmcloud) host plugins. They are executables (usually dedicated to longer-lived processes) that deliver common functionalities called [capabilities](https://wasmcloud.com/docs/concepts/capabilities). Providers are typically responsible for capabilities that are not considered part of the core business logic of an application, such as...

- Sending notifications
- Fetching secret values
- Accessing databases
- Serving content over HTTP

This crate is an SDK for creating capability providers in Rust. You can implement a capability that's already defined, or create a custom capability interface and use that in your wasmCloud application.

## Usage

Refer to the [custom template](https://github.com/wasmCloud/wasmCloud/tree/main/examples/rust/providers/custom-template#custom-capability-provider) for a comprehensive example of a custom provider.
