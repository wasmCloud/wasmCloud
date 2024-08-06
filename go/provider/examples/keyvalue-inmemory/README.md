# Keyvalue inmemory golang provider

This provider implements `wrpc:keyvalue/store@0.2.0-draft`.

## Notable files

- [main.go](./main.go) is a simple binary that sets up an errGroup to handle running the provider's primary requirements: executing as a standaline binary based on data received on stdin, handling RPC and connecting to a wasmCloud lattice.
- [keyvalue.go](./keyvalue.go) implements the required functions to conform to `wasi:keyvalue/store`. If the functions as specified in the [WIT](./wit/deps/keyvalue/store.wit) are not implemented, this provider will fail to build.
