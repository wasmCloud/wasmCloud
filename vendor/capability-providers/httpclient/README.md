# HTTP Client Capability Provider

This capability provider implements the `wasmcloud:httpclient` capability contract, and enables an actor to make outgoing HTTP(s) requests. It is implemented in Rust using the [reqwest](https://docs.rs/reqwest) library.

This capability provider is multi-threaded and can handle concurrent requests from multiple actors.

Build with `make`. Test with `make test`.

## Link Definition Values
This capability provider does not have any link definition configuration values.

