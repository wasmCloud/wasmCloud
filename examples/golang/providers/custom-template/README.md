# Custom Capability Provider

This capability provider is a template for creating providers with custom capabilities. It uses the [provider-sdk-go](https://github.com/wasmCloud/provider-sdk-go) and implements the auto-generated Provider interface with an example handler that will persist the links that target the provider (target links) and links where the provider is the source and targets a component (source links).

The purpose of this example is to provide comprehensive comments on the usage of our wasmCloud provider SDK, from serving RPC exports to invoking component imports. The code is informative to read through and provides a base for extending wasmCloud with custom capabilities.

## Building

Prerequisites:

1. [Go toolchain](https://go.dev/doc/install)
1. [wit-bindgen-wrpc](https://github.com/wrpc/wit-bindgen-wrpc/tree/main) `cargo install wit-bindgen-wrpc-cli`
1. [wash](https://wasmcloud.com/docs/installation)

```bash
go generate ./...
go build .
```

Alternatively, you can generate, build and package this provider in one step:

```bash
wash build
```

You can build the included test component with `wash build -p ./component`.

## Running to test

Prerequisites:

1. [Go toolchain](https://go.dev/doc/install)
1. [nats-server](https://github.com/nats-io/nats-server)
1. [nats-cli](https://github.com/nats-io/natscli)

You can run this capability provider as a binary by passing a simple base64 encoded [HostData](https://pkg.go.dev/github.com/wasmCloud/provider-sdk-go#HostData) struct, in order to do basic testing. For example:

```bash
nats-server -js &
echo '{"lattice_rpc_url": "0.0.0.0:4222", "lattice_rpc_prefix": "default", "provider_key": "custom-template", "link_name": "default"}' | base64 | go run .
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

You can deploy this provider, along with a [prebuilt component](../component/) for testing, by deploying the [wadm.yaml](./wadm.yaml) application.

```bash
# Build the component
cd component
wash build

# Return to the provider directory
cd ..

# Launch wasmCloud in the background
wash up -d
# Deploy the application
wash app deploy ./wadm.yaml
```

## Customizing

Customizing this provider to meet your needs of a custom capability takes just a few steps.

1. Update the [wit/world.wit](./wit/world.wit) to include the data types and functions that model your custom capability. You can use the example as a base and the [component model WIT reference](https://component-model.bytecodealliance.org/design/wit.html) as a guide for types and keywords.
1. Implement any provider `export`s in [provider.go](./provider.go) as methods of your `Handler`.
1. Use any provider `import`s in [provider.go](./provider.go) to invoke linked components. Check out the `Call()` function for an example for how to invoke a component using RPC.

Have any questions? Please feel free to [file an issue](https://github.com/wasmCloud/wasmCloud/issues/new/choose) and/or join us on the [wasmCloud slack](https://slack.wasmcloud.com)!
