[![crates.io](https://img.shields.io/crates/v/wasmcloud-s3.svg)](https://crates.io/crates/wascc-s3)&nbsp;
![Rust](https://github.com/wasmcloud/capability-providers/workflows/S3/badge.svg)&nbsp;
![license](https://img.shields.io/crates/l/wasmcloud-s3.svg)&nbsp;
[![documentation](https://docs.rs/wasmcloud-s3/badge.svg)](https://docs.rs/wascc-s3)

# wasmCloud Blobstore Provider (S3)

A native capability provider for wasmCloud that implements the `wasmcloud:blobstore` protocol for Amazon S3 and S3-compliant (e.g. `minio`) storage servers.

If you want to statically compile (embed) this plugin into a custom host, then enable the `static_plugin` feature in your dependencies:

```
wasmcloud-s3 = { version = "??", features = ["static_plugin"]}
```
