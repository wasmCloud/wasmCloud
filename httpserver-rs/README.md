# wasmcloud HTTP Server Provider

This library is a _native capability provider_ for the `wasmcloud:httpserver` 
capability. Only actors signed with this capability 
privilege will be allowed to use it. The actor that implements this contract 
must implement the [HandleRequest](https://docs.rs/wasmcloud-interface-httpserver/0.1.5/wasmcloud_interface_httpserver/trait.HttpServer.html#) operation

Run `make` to compile to a native executable and build the par file.
The par file is created in `build/httpserver.par.gz`.

Configuration settings for the httpserver provider are described in [settings](./settings.md). 
The default listen address is 127.0.0.1 port 8000.

Note: If multiple actors on the same server or VM attempt to use the same 
IP interface and port, only the first actor link for that port will succeed, 
and the others will fail. During development, 
it is recommended to check ("tail") the wasmcloud host logs for success and error messages.

For more hands-on tutorials on building actors, including HTTP server actors,
see the [wasmcloud.dev](https://wasmcloud.dev) website.
