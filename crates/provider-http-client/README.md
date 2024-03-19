# HTTP Client Capability Provider

This capability provider implements the `wasmcloud:httpclient` capability contract, and enables an component to make outgoing HTTP(s) requests. It is implemented in Rust using the [reqwest](https://docs.rs/reqwest) library.

This capability provider is multi-threaded and can handle concurrent requests from multiple components.

## Link Definition Values
This capability provider does not have any link definition configuration values.
