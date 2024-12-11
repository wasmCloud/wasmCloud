# `wasmcloud-test-util`

This repository contains utilities for writing tests for the [wasmCloud][wasmCloud] ecosystem.

This crate provides utilities for:

- Manipulating a wasmCloud host programmatically
- Starting/stopping workloads (i.e. providers and components)
- Dealing with OS-level dependencies (ex. random ports)
- ... and more

This crate is meant to be used by programs, utilities and infrastructure targeting the wasmCloud platform.

[wasmCloud]: https://wasmcloud.com

## Installation

To use `wasmcloud-test-util` in your project, you can add it via `cargo add` as a test (development) dependency:

```console
cargo add --dev wasmcloud-test-util
```

Or include the following in your `Cargo.toml`:

```toml
wasmcloud-test-util = "0.13.0"
```

## Features

`wasmcloud-test-util` comes with the following features:

| Feature             | Default? | Description                                                                 |
|---------------------|----------|-----------------------------------------------------------------------------|
| http                | no       | Enable HTTP related test utilities                                               |
| os                  | no       | Enable OS-level test utilities
| testcontainers      | no       | Enable [`testcontainers`][testcontainers]-related extensions |

[testcontainers]: https://crates.io/crates/testcontainers

## Using `wasmcloud-test-util`

`wasmcloud-test-util` does not provide a `prelude`, but instead exports types as needed under appropriate modules.

```rust
use tokio::time::Duration;

use wasmcloud_test_util::control_interface::ClientBuilder;
use wasmcloud_test_util::lattice::link::{assert_advertise_link, assert_remove_link};
use wasmcloud_test_util::nats::wait_for_nats_connection;
use wasmcloud_test_util::provider::StartProviderArgs;
use wasmcloud_test_util::testcontainers::{AsyncRunner as _, ImageExt, NatsServer};
use wasmcloud_test_util::{
    assert_config_put, assert_scale_component, assert_start_provider, WasmCloudTestHost,
};

#[tokio::test]
async fn example_test() -> anyhow::Result<()> {
    // Start NATS
    let nats_container = NatsServer::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("failed to start nats-server container");
    let nats_port = nats_container
        .get_host_port_ipv4(4222)
        .await
        .expect("should be able to find the NATS port");
    let nats_url = format!("nats://127.0.0.1:{nats_port}");

    // Build a wasmCloud host (assuming you have a local NATS server running)
    let lattice = "default";
    let host = WasmCloudTestHost::start(nats_url, lattice).await?;

    // Once you have a host (AKA a single-member wasmCloud lattice), you'll want a NATS client
    // which you can use to control the host and the lattice:
    let nats_client = async_nats::connect(nats_url).await?;
    let ctl_client = ClientBuilder::new(nats_client)
        .lattice(host.lattice_name().to_string())
        .build();

    // Now that you have a control client, you can use the `assert_*` functions to perform actions on your host:
    assert_config_put(
        &ctl_client,
        "test-config",
        [("EXAMPLE_KEY".to_string(), "EXAMPLE_VALUE".to_string())],
    )
    .await?;

    assert_scale_component(
        &ctl_client,
        &host.host_key(),
        "ghcr.io/wasmcloud/components/http-jsonify-rust:0.1.1",
        "example-component",
        None,
        1,
        Vec::new(),
        Duration::from_secs(10),
    )
    .await?;

    // ... your test logic goes here ...

    Ok(())
}
```

## Contributing

Have a change that belongs be in `wasmcloud-test-util`? Please feel free to [file an issue](https://github.com/wasmCloud/wasmCloud/issues/new/choose) and/or join us on the [wasmCloud slack](https://slack.wasmcloud.com)!
