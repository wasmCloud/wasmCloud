# Couchbase Key Value provider

[Couchbase][cb] is a highly versatile, performant and scalablehighly versatile, performant and scalable award-winning distributed NoSQL cloud database -- and with this provider your WebAssembly components can have access to it.

This capability provider implements the [wasmcloud:keyvalue](https://github.com/wasmCloud/interfaces/tree/main/keyvalue) capability contract with a [Couchbase][cb] back-end.

## Getting started

The easiest way to use this provider is to pass `ghcr.io/wasmcloud/keyvalue-couchbase:0.1.0` as the OCI reference parameter to a `wash start provider` command:

```bash
wash start provider ghcr.io/wasmcloud/keyvalue-couchbase:0.1.0
```

[cb]: https://www.couchbase.com


## Link Definition Configuration Settings

This provider is multi-threaded and can handle concurrent requests from multiple components.

Each link definition declared for this provider will result in a single connection to a Couchbase instance managed on behalf of the linked component. Connections are maintained within the provider process, so multiple instances of this provider running in the same lattice will not share connections.

The following is a list of configuration settings available in the link definition.

| Property                 | Default | Example                                       | Description                                                                |
|--------------------------|---------|-----------------------------------------------|----------------------------------------------------------------------------|
| `URL`                    | N/A     | `couchbase://user:password@localhost/example` | Couchbase connection string                                                |
| `USERNAME`               | N/A     | `admin`                                       | Couchbase instance username                                                |
| `PASSWORD`               | N/A     | `password`                                    | Couchbase instance password                                                |
| `USE_DEFAULT_CONNECTION` | `false` | `true`                                        | Use the default connection (required for Couchbase servers v6.5 and above) |


## Development

> ![WARNING]
> Due to requirements on upstream [`couchbase-rs`][couchbase-rs], which uses the `libcouchbase` C binding, you
> must ensure you have everything required to build `libcouchbase` installed locally.


To build the provider locally, you can use `cargo`:

```console
cargo build
```

In addition to building `libcouchbase` and `couchbase-rs`, the build process will attempt to build the libraries for couchbase statically into the Rust provider binary.

[libcouchbase]: https://github.com/couchbase/libcouchbase
[couchbase-rs]: https://github.com/couchbaselabs/couchbase-rs
