# HTTP KeyValue Watcher

This component demonstrates a Wasm-based system for monitoring and reacting to key-value store operations through HTTP interfaces. It integrates with [wasi-http](https://github.com/WebAssembly/wasi-http) for handling Outgoing HTTP responses and [wasi-keyvalue](https://github.com/WebAssembly/wasi-keyvalue) for key-value store interactions. The component establishes watch triggers based on link configuration and executes configurable reactions when events occur. At runtime, it [links](https://wasmcloud.com/docs/concepts/linking-components) to a Redis-backed implementation of the key-value store interface.


## Prerequisites

- `cargo` 1.75
- [`wash`](https://wasmcloud.com/docs/installation) 0.37.0

## Building

```bash
wash build
```

## Running with wasmCloud

You can build and deploy your component with all dependencies and a hot reload loop with `wash dev`.

```shell
wash dev
```

## Note
The `watcher` interface of the Redis key-value provider requires keyspace event notifications to be enabled for proper operation of the watch functionality. Enable this by running:
```bash
$ redis-cli CONFIG SET notify-keyspace-events Kg$
``` 
Without this configuration, the watcher component won't receive notifications about key-value store operations. Note that this setting persists until explicitly changed or Redis is restarted.

## Setting Watch Triggers
You can set triggers for key-value operations using link configuration by adding a `watch` field with a comma-separated string of watch patterns. Each pattern follows the format `OPERATION@key`
```bash
# wash config put <config-name> <values...>
wash config put custom-config foo=bar watch=SET@key,DEL@key
```
and load the custom-config while starting the provider
```bash
# wash start provider <component_ref> <component_id> --config <config-name>
wash start provider ghcr.io/wasmcloud/keyvalue-redis:0.28.1 rust-keyvalue-watcher --config custom-config

```
You can also add the watch parameters under the link config properties inside the wadm manifest
```yaml
# Under a link trait
config:
  - name: redis-custom
    properties:
        url: redis://0.0.0.0:6379
        watch: SET@key,DEL@key,.....
```
Note that only SET and DEL operations are supported in `wasi:keyvalue@0.2.0-draft`

## Customizing Trigger Reactions
The component currently sends HTTP requests to an alert server when watched operations occur. You can customize the reaction mechanism by modifying the on_set and on_delete functions in the `KvWatcherDemoGuest` implementation:

- `on_set`: Triggered when the watched key undergoes a SET operation (**Note**: the `value` parameter contains the latest SET value of the Key.)s

```rust
fn on_set(bucket: bindings::wasi::keyvalue::store::Bucket, key: String, value: Vec<u8>) {
    // Current implementation sends an HTTP request to localhost:3001/alert
    // You can modify this to implement different reactions
    // e.g., sending to different endpoints, formatting data differently,
    // or implementing entirely different reaction mechanisms
}
```
- `on_delete`: Triggered when the watched key undergoes a DEL operation

```rust
fn on_delete(bucket: bindings::wasi::keyvalue::store::Bucket, key: String) {
    // Current implementation sends an HTTP request to localhost:3001/alert
    // Customize this function to handle delete events differently
}
```

### Examples of alternative reaction mechanisms you could implement:

- Send notifications to different services
- Log to specific monitoring systems
- Trigger cascading operations in other storage systems
- Implement complex business logic based on the changes

## Adding Capabilities

To learn how to extend this example with additional capabilities, see the [Adding Capabilities](https://wasmcloud.com/docs/tour/adding-capabilities?lang=rust) section of the wasmCloud documentation.
