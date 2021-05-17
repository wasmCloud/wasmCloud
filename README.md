# capability-providers

This repo contains first-party native capability providers for wasmCloud, and the tools to build and develop them.

### Supported platforms

Platforms are architecture/operating system combinations. All of the capability providers can be cross-compiled with the tooling in this repo:

* `arm-linux`
* `aarch64-linux`
* `aarch64-apple-darwin`
* `x86_64-macos` (requires fork of [cross](https://github.com/ChrisRx/cross))
* `x86_64-linux`
* `x86_64-windows`

`x86_64-macos` requires installing a fork of cross until the PR [adding the x86_64-apple-darwin target](https://github.com/rust-embedded/cross/pull/480) is accepted. It can be installed to `$HOME/.cargo/bin` via cargo:

```sh
cargo install --git https://github.com/ChrisRx/cross --branch add-darwin-target --force
```

MIPS-based architectures are not cross-compiled by default because some capability providers have dependencies that do not currently support MIPS. In particular, anything using [rustls](https://github.com/ctz/rustls) will not compile for MIPS-based architectures until [ring](https://github.com/briansmith/ring), a cryptography library that rustls depends on, fully [supports MIPS](https://github.com/briansmith/ring/issues/562).

### Latest Versions
| Capability Provider | Crate | Provider Archive OCI Reference |
|---|---|---|
| FS (File system) | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-fs)](https://crates.io/crates/wasmcloud-fs) | wasmcloud.azurecr.io/fs |
| HTTP Client | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-httpclient)](https://crates.io/crates/wasmcloud-httpclient) | wasmcloud.azurecr.io/httpclient |
| HTTP Server | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-httpserver)](https://crates.io/crates/wasmcloud-httpserver) | wasmcloud.azurecr.io/httpserver |
| Logging | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-logging)](https://crates.io/crates/wasmcloud-logging) | wasmcloud.azurecr.io/logging |
| NATS | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-nats)](https://crates.io/crates/wasmcloud-nats) | wasmcloud.azurecr.io/nats |
| Redis | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-redis)](https://crates.io/crates/wasmcloud-redis) | wasmcloud.azurecr.io/redis |
| Redis Streams | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-redisstreams)](https://crates.io/crates/wasmcloud-redisstreams) | wasmcloud.azurecr.io/redisstreams |
| RedisGraph | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-redisgraph)](https://crates.io/crates/wasmcloud-redisgraph) | wasmcloud.azurecr.io/redisgraph |
| S3 | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-s3)](https://crates.io/crates/wasmcloud-s3) | wasmcloud.azurecr.io/s3 |
| Telnet | [![Crates.io](https://img.shields.io/crates/v/wasmcloud-telnet)](https://crates.io/crates/wasmcloud-telnet) | wasmcloud.azurecr.io/telnet |

OCI reference tags match up exactly to the `crates.io` version, without the prefixed `v`. For example, if the `Logging` provider shows a `crates.io` version of `v0.9.0`, you can access the `Logging` provider archive at `wasmcloud.azurecr.io/logging:0.9.0`.

### Getting started

You can cross-compile every provider running:

```sh
make all
```

or just a particular provider:

```sh
make http-client
```

If you want to try building a provider archive that includes MIPS-based architectures, run this from the directory of the specific provider:

```sh
make par-mips
```

This will build a provider archive that includes all supported platforms include ones that have MIPS-based architectures.

### Releasing Capability Providers

This repo contains `Actions` that allow for individual releases of capability providers. To release a capability provider as a maintainer (I'll use http-client in this example), follow these steps.

1. Make any necessary changes to capability provider
1. Update semver contained in that specific provider's `Cargo.toml`
1. Submit PR for review
1. Once PR is merged:
```
git checkout origin/main
git pull origin main
git tag -a http-client-v1.2.3 -m "http-client release v1.2.3" # Tag must be in the form of <provider>-vX.Y.Z
git push origin http-client-v1.2.3 # Kicks off the release action
```
