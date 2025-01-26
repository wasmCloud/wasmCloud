# 📡 WADM Status Receiver Example

This folder contains a WebAssembly component that makes use of:

- The [`wasmcloud:wadm/handler` WIT contract][contract]
- The [`wadm-provider`][provider] Capability Provider

[contract]: ./wit/deps/wasmcloud-wadm/package.wit
[provider]: ../../../../crates/provider-wadm

## 📦 Dependencies

- [`cargo`][cargo] (part of the Rust toolchain) for building this project
- [`wash`][wash] for building and running the components and [wasmCloud][wasmcloud] hosts

[cargo]: https://doc.rust-lang.org/cargo/
[wash]: https://github.com/wasmCloud/wash

## 👟 Quickstart

As with all other examples, you can get started quickly by using the [Wasmcloud SHell (`wash`)][wash].

Since `wash` supports declarative deployments (powered by [Wasmcloud Application Deployment Manager (`wadm`)][wadm]), you can get started quickly using the provided manifests:

### Build this component

```console
wash build
```

This will create a folder called `build` which contains `wadm_status_receiver_s.wasm`.

> [!NOTE]
> If you're using a local build of the provider (using `file://...` in `wadm.yaml`) this is a good time to ensure you've built the [provider archive `par.gz`][par] for your provider.

### Start a wasmCloud host with WADM

```console
wash up
```

> [!NOTE]
> `wash up` will run as long as the host is running (you can cancel it with `Ctrl-C`)

### Deploy the status receiver application

First, deploy our status receiver component that will listen for updates:

```console
wash app deploy local.wadm.yaml
```

### Deploy the example application to monitor

Now deploy the example application that our receiver will monitor:

```console
wash app deploy example.wadm.yaml
```

You should start seeing status updates in the logs as the example application deploys and its status changes.

To see everything running in the lattice:

```console
wash get inventory
```

To test status changes, you can:
1. Undeploy the example application:
   ```console
   wash app undeploy example-app
   ```
2. Redeploy it:
   ```console
   wash app deploy example.wadm.yaml
   ```

Each of these actions will generate status updates that our receiver will log.

## ⌨️ Code guide

With [wasmCloud][wasmcloud], you write only the important bits of your business logic, so the code for this component is short, with the important bits highlighted below:

```rust
impl Guest for StatusReceiver {
    fn handle_status_update(msg: StatusUpdate) -> Result<(), String> {
        wasi::logging::logging::log(
            wasi::logging::logging::Level::Info,
            "wadm-status",
            &format!(
                "Application '{}' v{} - Status: {:?}",
                msg.app, msg.status.version, msg.status.info.status_type
            ),
        );

        wasi::logging::logging::log(
            wasi::logging::logging::Level::Info,
            "wadm-status",
            &format!("Components found: {}", msg.status.components.len()),
        );

        for component in msg.status.components {
            wasi::logging::logging::log(
                wasi::logging::logging::Level::Info,
                "wadm-status",
                &format!(
                    "Component '{}' - Status: {:?}",
                    component.name, component.info.status_type
                ),
            );
        }
        Ok(())
    }
}
```

[wasmcloud]: https://wasmcloud.com/docs/intro
