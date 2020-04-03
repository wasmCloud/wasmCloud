
[![crates.io](https://img.shields.io/crates/v/wascc-s3.svg)](https://crates.io/crates/wascc-s3)&nbsp;
![Rust](https://github.com/wascc/s3-provider/workflows/Rust/badge.svg)&nbsp;
![license](https://img.shields.io/crates/l/wascc-s3.svg)&nbsp;
[![documentation](https://docs.rs/wascc-redis/badge.svg)](https://docs.rs/wascc-s3)

# waSCC S3 Capability Provider

A native capability provider for waSCC that implements the `wascc:blobstore` protocol for Amazon S3 and S3-compliant (e.g. `minio`) storage servers.

If you want to statically compile (embed) this plugin into a custom host, then enable the `static_plugin` feature in your dependencies:

```
wascc-s3 = { version = "??", features = ["static_plugin"]}
```
