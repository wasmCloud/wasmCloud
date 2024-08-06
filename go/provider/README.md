# wasmCloud Provider SDK

Providers are swappable [wasmCloud](https://wasmcloud) host plugins. They are executables (usually dedicated to longer-lived processes) that deliver common functionalities called capabilities. Providers are typically responsible for [capabilities](https://wasmcloud.com/docs/concepts/capabilities) that are not considered part of the core business logic of an application, such as...

- Sending notifications
- Fetching secret values
- Accessing databases
- Serving content over HTTP

This package is an SDK for creating capability providers in Golang. You can implement a capability that's already defined, or create a custom capability interface and use that in your wasmCloud application.

## Usage

An example can be found in [examples/keyvalue-inmemory](./examples/keyvalue-inmemory/) which implements the interface `wrpc:keyvalue/store@0.2.0-draft`.

Refer to the [custom template](https://github.com/wasmCloud/wasmCloud/tree/main/examples/golang/providers/custom-template#custom-capability-provider) for a comprehensive example of a custom provider.
