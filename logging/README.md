[![crates.io](https://img.shields.io/crates/v/wasmcloud-logging.svg)](https://crates.io/crates/wasmcloud-logging)&nbsp;
![Rust](https://github.com/wasmcloud/logging-provider/workflows/Rust/badge.svg)&nbsp;
![license](https://img.shields.io/crates/l/wasmcloud-logging.svg)&nbsp;
[![documentation](https://docs.rs/wasmcloud-logging/badge.svg)](https://docs.rs/wasmcloud-logging)

# wasmCloud Logging Provider

This library is a _native capability provider_ for the `wasmcloud:logging` capability. Only actors signed with tokens containing this capability privilege will be allowed to use it. It allows actors to use normal `log` macros (like `info!`, `warn!`, `error!`, etc, to write logs from within the actor.

It should be compiled as a native binary (linux: `.so`, mac: `.dylib`, windows: `dll`, etc) and made available to the **wasmCloud** host runtime as a plugin. This is commonly done by creating a [provider-archive](https://github.com/wasmCloud/provider-archive)

If you want to statically link (embed) this capability provider into a custom host, then enable the `static_plugin` feature in your dependencies as follows:

```
wasmcloud-logging = { version="??", features = ["static_plugin"] }
```
