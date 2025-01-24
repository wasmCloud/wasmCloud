# HTTP KeyValue Watcher

This component demonstrates a Wasm-based system for monitoring and reacting to key-value store operations through HTTP interfaces. It integrates with [wasi-http](https://github.com/WebAssembly/wasi-http) for handling HTTP requests and [wasi-keyvalue](https://github.com/WebAssembly/wasi-keyvalue) for key-value store interactions. The component establishes watch triggers based on incoming HTTP requests and executes configurable reactions when monitored events occur. At runtime, it [links](https://wasmcloud.com/docs/concepts/linking-components) to a Redis-backed implementation of the key-value store interface.


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

## Setting Watch Triggers
You can set triggers for key-value operations using HTTP requests with the following query parameters:

- action: Type of operation to watch for (on_set or on_delete)
- key: The key to watch for changes
- value: The value to watch for (only required for on_set actions)

### Watch for SET operations
```bash
curl "http://localhost:8000/action=on_set&key=foo&value=bar"
```

### Watch for DELETE operations
```bash
curl "http://localhost:8000/action=on_delete&key=foo"
```

The component will respond with confirmation messages like:

```bash
foo: Successfully created on_set trigger
foo: Successfully created on_delete trigger
```

## Customizing Trigger Reactions
The component currently sends HTTP requests to an alert server when watched operations occur. You can customize the reaction mechanism by modifying the on_set and on_delete functions in the `KvWatcherDemoGuest` implementation:

- `on_set`: Triggered when a watched key is set

```rust
fn on_set(bucket: bindings::wasi::keyvalue::store::Bucket, key: String, value: Vec<u8>) {
    // Current implementation sends an HTTP request to localhost:3001/alert
    // You can modify this to implement different reactions
    // e.g., sending to different endpoints, formatting data differently,
    // or implementing entirely different reaction mechanisms
}
```

- `on_delete`: Triggered when a watched key is deleted

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
