# `http-hello-world`

This repository contains a WebAssembly component built using [`wstd`](https://github.com/bytecodealliance/wstd) (an async Rust standard library for Wasm components and WASI 0.2) to serve HTTP requests.

# Quickstart

To start a local development loop with [Wasm Shell](https://github.com/wasmCloud/wasmCloud):

```shell
wash dev
```

To build the component:

```shell
wash build
```

An OCI artifact of this Wasm component is available from [GitHub Packages](https://github.com/orgs/wasmCloud/packages/container/package/components%2Fhello-world) at `ghcr.io/wasmcloud/components/hello-world:0.1.0`.