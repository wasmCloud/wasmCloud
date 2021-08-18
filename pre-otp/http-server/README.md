[![crates.io](https://img.shields.io/crates/v/wasmcloud-httpserver.svg)](https://crates.io/crates/wasmcloud-httpserver)&nbsp;
![Rust](https://github.com/wasmcloud/capability-providers/workflows/HTTPSERVER/badge.svg)
![license](https://img.shields.io/crates/l/wasmcloud-httpserver.svg)&nbsp;
[![documentation](https://docs.rs/wasmcloud-httpserver/badge.svg)](https://docs.rs/wasmcloud-httpserver)

# wasmCloud HTTP Server Provider

This library is a _native capability provider_ for the `wasmcloud:httpserver` capability. Only actors signed with tokens containing this capability privilege will be allowed to use it. 

It should be compiled as a native shared object binary (linux `.so`, mac `.dylib`, windows `.dll`) and made available to the **wasmCloud** host runtime as a plugin. If you want to statically compile (embed) it into a custom wasmCloud host, then simply enable the `static_plugin` feature in your dependencies:

```
wasmcloud-httpserver = { version = "0.11.1", features = ["static_plugin"] }
```

To create an actor that makes use of this capability provider, make sure that a configuration is supplied at runtime and includes a `PORT` variable. This will enable the HTTP server and direct _all_ requests to your actor module, which you can handle by checking that a dispatched operation is equivalent to the constant `OP_HANDLE_REQUEST`. For more information on the various types available to HTTP-based actors, check out the [actor-interfaces](https://github.com/wasmcloud/actor-interfaces) repository.

For more hands-on tutorials on building actors, including HTTP server actors, see the [wasmcloud.dev](https://wasmcloud.dev) website.

**NOTE**: If multiple actors within the same host process request HTTP server configurations, 
each actor will get its own HTTP server. Be careful not to request the same HTTP port for multiple actors in the same host process, as this will cause the host process to reject the configuration/binding.
