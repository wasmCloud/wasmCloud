[![crates.io](https://img.shields.io/crates/v/wascc-logging.svg)](https://crates.io/crates/wascc-logging)&nbsp;
![Rust](https://github.com/wascc/logging-provider/workflows/Rust/badge.svg)&nbsp;
![license](https://img.shields.io/crates/l/wascc-logging.svg)&nbsp;
[![documentation](https://docs.rs/wascc-logging/badge.svg)](https://docs.rs/wascc-logging)

# waSCC Logging Provider

This library is a _native capability provider_ for the `wascc:logging` capability. Only actors signed with tokens containing this capability privilege will be allowed to use it.  It allows actors to use normal `log` macros (like `info!`, `warn!`, `error!`, etc, to write logs from within the actor.

It should be compiled as a native linux (`.so`) binary and made available to the **waSCC** host runtime as a plugin. 

If you want to statically link (embed) this capability provider into a custom host, then enable the `static_plugin` feature in your dependencies as follows:

```
wascc-logging = { version="??", features = ["static_plugin"] }
```

