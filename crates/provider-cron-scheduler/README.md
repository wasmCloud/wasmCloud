# Cron Job Capability Provider

This capability provider enables scheduled execution of tasks using cron expressions within the wasmCloud ecosystem. It uses the [wasmcloud-provider-sdk](https://crates.io/crates/wasmcloud-provider-sdk) and implements the [Provider](https://docs.rs/wasmcloud-provider-sdk/0.5.0/wasmcloud_provider_sdk/trait.Provider.html) trait to manage scheduled tasks based on cron expressions.

The provider maintains a registry of cron jobs for components, executes the jobs according to their schedules, and delivers payloads to the target components. It supports dynamic configuration through link definitions, allowing components to register and update their scheduled jobs at runtime.

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

You can run this cron capability provider as a binary by passing a simple base64 encoded [HostData](https://docs.rs/wasmcloud-core/0.6.0/wasmcloud_core/host/struct.HostData.html) struct, in order to do basic testing. For example:

```bash
nats-server -js &
echo '{"lattice_rpc_url": "0.0.0.0:4222", "lattice_rpc_prefix": "default", "provider_key": "wasmcloud:cron", "config": {"interval_seconds": "5"}, "env_values": {}, "link_definitions": [], "otel_config": {"enable_observability": false}}' | base64 | cargo run
```

And in another terminal, you can request the health of the provider using the NATS CLI

```bash
nats req "wasmbus.rpc.default.wasmcloud:scheduler.health" '{}'
```

## Running as an application

You can deploy this provider, along with a [component](./component/) for testing, by deploying the [wadm.yaml](./wadm.yaml) application. Make sure to build the component with `wash build`.

```bash
# Launch wasmCloud in the background
wash up -d
# Deploy the application
wash app deploy ./wadm.yaml
```

## Usage

Components can register cron jobs by linking to this provider with appropriate configuration. The link configuration should include cron expressions in the following format:

```
job_name=cron_expression:payload
```

For example:
```
daily_report=0 0 * * *:{"type":"generate_report"}
hourly_update=0 * * * *:{"action":"refresh_data"}
```

Multiple jobs can be specified by separating them with semicolons:
```
daily_report=0 0 * * *:{"type":"generate_report"};hourly_update=0 * * * *:{"action":"refresh_data"}
```

When a job is triggered, the provider will invoke the target component with the specified payload.

## Customizing

You can customize this cron job provider by:

1. Adjusting the minimum interval time between checks in the configuration.
2. Extending the job parsing logic to support additional formats or parameters.
3. Adding support for more complex scheduling patterns beyond standard cron expressions.
4. Implementing job history and status reporting capabilities.
5. Adding retry logic or error handling for failed job executions.

Have any questions? Please feel free to [file an issue](https://github.com/wasmCloud/wasmCloud/issues/new/choose) and/or join us on the [wasmCloud slack](https://slack.wasmcloud.com)!