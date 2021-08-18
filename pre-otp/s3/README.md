[![crates.io](https://img.shields.io/crates/v/wasmcloud-s3.svg)](https://crates.io/crates/wasmcloud-s3)&nbsp;
![Rust](https://github.com/wasmcloud/capability-providers/workflows/S3/badge.svg)&nbsp;
![license](https://img.shields.io/crates/l/wasmcloud-s3.svg)&nbsp;
[![documentation](https://docs.rs/wasmcloud-s3/badge.svg)](https://docs.rs/wasmcloud-s3)

# wasmCloud Blobstore Provider (S3)

A native capability provider for wasmCloud that implements the `wasmcloud:blobstore` protocol for Amazon S3 and S3-compliant (e.g. `minio`) storage servers.

If you want to statically compile (embed) this plugin into a custom host, then enable the `static_plugin` feature in your dependencies:

```
wasmcloud-s3 = { version = "??", features = ["static_plugin"]}
```

## Configuration Values
| Value | Description |
| ----------- | ----------- |
| REGION | AWS region to use (default `us-east-1`) |
| ENDPOINT | AWS endpoint to use (default `s3.us-east-1.amazonaws.com` |
| AWS_ACCESS_KEY | AWS access key for authentication |
| AWS_SECRET_ACCESS_KEY | AWS secret access key for authentication |
| AWS_TOKEN | AWS token for authentication (can be omitted if not needed for auth) |
| TOKEN_VALID_FOR | AWS token lifetime (in seconds)|
| HTTP_PROXY | Proxy URL to use with the S3 client |