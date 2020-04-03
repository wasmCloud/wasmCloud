
![Rust](https://github.com/wascc/s3-provider/workflows/Rust/badge.svg)

# S3 Provider

A native capability provider for waSCC that implements the `wascc:blobstore` protocol for Amazon S3 and S3-compliant (e.g. minio) storage servers.

If you want to statically compile (embed) this plugin into a custom host, then enable the `static_plugin` feature in your dependencies:

```
wascc-s3 = { version = "??", features = ["static_plugin"]}
```
