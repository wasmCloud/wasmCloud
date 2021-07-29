<!--
badges go here when published
[![crates.io](https://img.shields.io/crates/v/wasmcloud-httpserver.svg)](https://crates.io/crates/wasmcloud-httpserver)&nbsp;
![Rust](https://github.com/wasmcloud/capability-providers/workflows/HTTPSERVER/badge.svg)
![license](https://img.shields.io/crates/l/wasmcloud-httpserver.svg)&nbsp;
[![documentation](https://docs.rs/wasmcloud-httpserver/badge.svg)](https://docs.rs/wasmcloud-httpserver)
-->

# wasmcloud HTTP Server Provider

This library is a _native capability provider_ for the `wasmcloud:httpserver` capability. Only actors signed with tokens containing this capability privilege will be allowed to use it. The actor that implements this contract must respond to the `HandleRequest" operation. **link to online interface docs**

It should be compiled as a native executable and its file system path provided to the wasmcloud host.

With the default settings, the server will bind to 127.0.0.1 and listen on port 8000. This defaults can be overridden by ... 
> TBD: do we link to documentation on manifest format, or how to specify settings in web UI, or will there be another way?
 
For more hands-on tutorials on building actors, including HTTP server actors, see the [wasmcloud.dev](https://wasmcloud.dev) website.

**NOTE**: If multiple actors on the same server or VM attempt to use the same IP interface and port, only the first actor link for that port will succeed, and the others will fail. During development, it is recommended to check ("tail") the wasmcloud host logs for success and error messages.