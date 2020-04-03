[![crates.io](https://img.shields.io/crates/v/wascc-httpsrv.svg)](https://crates.io/crates/wascc-httpsrv)&nbsp;
![Rust](https://github.com/wascc/http-server-provider/workflows/Rust/badge.svg)
![license](https://img.shields.io/crates/l/wascc-httpsrv.svg)&nbsp;
[![documentation](https://docs.rs/wascc-httpsrv/badge.svg)](https://docs.rs/wascc-httpsrv)

# waSCC HTTP Server Provider

This library is a _native capability provider_ for the `wascc:http_server` capability. Only actors signed with tokens containing this capability privilege will be allowed to use it. 

It should be compiled as a native linux (`.so`) binary and made available to the **waSCC** host runtime as a plugin. If you want to statically compile (embed) it into a custom waSCC host, then simply enable the `static_plugin` feature in your dependencies:

```
wascc-httpsrv = { version = "??", features = ["static_plugin"] }
```

To create an actor that makes use of this capability provider, make sure that a configuration is supplied at runtime and includes a `PORT` variable. This will enable the HTTP server and direct _all_ requests to your actor module, which you can handle by checking that a dispatched operation is equivalent to the constant `OP_HANDLE_REQUEST`. For more information on the various types available to HTTP-based actors, check out the [wascc-codec documentation](https://docs.rs/wascc-codec).

For more hands-on tutorials on building actors, including HTTP server actors, see the [wascc.dev](https://wasc.dev) website.

**NOTE**: If multiple actors within the same host process request HTTP server configurations, multiple threads will be consumed and each actor will get its own HTTP server. Be careful not to request the same HTTP port for multiple actors in the same host process, as this will cause the host process to fail.
