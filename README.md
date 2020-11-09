# capability-providers

This repo contains first-party native capability providers for waSCC, and the tools to build and develop them.

### Supported platforms

Platforms are architecture/operating system combinations. All of the capability providers can be cross-compiled with the tooling in this repo:

* `arm-linux`
* `aarch64-linux`
* `x86_64-macos` (requires fork of [cross](https://github.com/ChrisRx/cross))
* `x86_64-linux`
* `x86_64-windows`

`x86_64-macos` requires installing a fork of cross until the PR [adding the x86_64-apple-darwin target](https://github.com/rust-embedded/cross/pull/480) is accepted. It can be installed to `$HOME/.cargo/bin` via cargo:

```sh
cargo install --git https://github.com/ChrisRx/cross --branch add-darwin-target --force
```

MIPS-based architectures are not cross-compiled by default because some capability providers have dependencies that do not currently support MIPS. In particular, anything using [rustls](https://github.com/ctz/rustls) will not compile for MIPS-based architectures until [ring](https://github.com/briansmith/ring), a cryptography library that rustls depends on, fully [supports MIPS](https://github.com/briansmith/ring/issues/562).

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
