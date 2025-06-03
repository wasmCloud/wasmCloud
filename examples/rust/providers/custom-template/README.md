# Custom Capability Provider

This capability provider is a template for creating providers with custom capabilities. It uses the [wasmcloud-provider-sdk](https://crates.io/crates/wasmcloud-provider-sdk) and implements the [Provider](https://docs.rs/wasmcloud-provider-sdk/0.5.0/wasmcloud_provider_sdk/trait.Provider.html) trait with an example handler that will persist the links that target the provider (target links) and links where the provider is the source and targets a component (source links).

The purpose of this example is to provide comprehensive comments on the usage of our wasmCloud provider SDK, from serving RPC exports to invoking component imports. The code is informative to read through and provides a base for extending wasmCloud with custom capabilities.

## Building

Prerequisites:

1. [Rust toolchain](https://www.rust-lang.org/tools/install)
1. [wash](https://wasmcloud.com/docs/installation)

You can build this capability provider by running `wash build`. You can build the included test component with `wash build -p ./component`.

## Running to test

Prerequisites:

1. [Rust toolchain](https://www.rust-lang.org/tools/install)
1. [nats-server](https://github.com/nats-io/nats-server)
1. [nats-cli](https://github.com/nats-io/natscli)

You can run this capability provider as a binary by passing a simple base64 encoded [HostData](https://docs.rs/wasmcloud-core/0.6.0/wasmcloud_core/host/struct.HostData.html) struct, in order to do basic testing. For example:

```bash
nats-server -js &
echo '{"lattice_rpc_url": "0.0.0.0:4222", "lattice_rpc_prefix": "default", "provider_key": "custom-template", "config": {"foo": "bar"}, "env_values": {}, "link_definitions": [], "otel_config": {"enable_observability": false}}' | base64 | cargo run
```

And in another terminal, you can request the health of the provider using the NATS CLI

```bash
nats req "wasmbus.rpc.default.custom-template.health" '{}'
```

Additionally, you can invoke the provider directly which will send test data to each linked component

```bash
wash call custom-template wasmcloud:example/system-info.call
```

## Running as an application

You can deploy this provider, along with a [component](./component/) for testing, by deploying the [wadm.yaml](./wadm.yaml) application. Make sure to build the component with `wash build`.

```bash
# Launch wasmCloud in the background
wash up -d
# Deploy the application
wash app deploy ./wadm.yaml
```

## Customizing

Customizing this provider to meet your needs of a custom capability takes just a few steps.

1. Update the [wit/world.wit](./wit/world.wit) to include the data types and functions that model your custom capability. You can use the example as a base and the [component model WIT reference](https://component-model.bytecodealliance.org/design/wit.html) as a guide for types and keywords.
1. Implement any provider `export`s in [src/provider.rs](./src/provider.rs) inside of the `impl Handler {}` block.
1. Use the methods inside of the `impl Provider {}` block to handle invoking components. For inspiration, take a look at our other capability providers that implement various capabilities like HTTP, Messaging, Key-Value in the [crates/provider-\*](../../../../crates/) folder.

Have any questions? Please feel free to [file an issue](https://github.com/wasmCloud/wasmCloud/issues/new/choose) and/or join us on the [wasmCloud slack](https://slack.wasmcloud.com)!
