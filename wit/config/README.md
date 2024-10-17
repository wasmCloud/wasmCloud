# `wasmcloud:config`

Vendored API that is versioned for stability, based on [`wasi:config`](https://github.com/WebAssembly/wasi-config).

## Versioning

This vendored API is versioned with every revision. Patch updates for non-breaking changes, e.g. `0.1.1`, and minor releases for breaking changes (as this API is currently at major version 0).

Our intention is for this interface to be eventually deprecated in favor of `wasi:config` once it is stable with an interim deprecation period. We will support all versions of this API until deprecation of `wasmcloud:config`.

Note that the version of wasmcloud API's do not correlate to WASIP2 API's e.g. `wasi:config@0.2.0`
may not be the same as `wasmcloud:config@0.2.0`.
