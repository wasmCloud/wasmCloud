# Rust HTTP Password Checker

This repository contains a WebAssembly Component written in [Rust][rust], which:

- Implements a [`wasi:http`][wasi-http]-compliant HTTP handler
- Uses the [`httpserver` provider][httpserver-provider] to serve requests
- Provides APIs for checking a password from a secret store, or from the incoming request
- Can be declaratively provisioned with [`wadm`][wadm]

[wasi-http]: https://github.com/WebAssembly/wasi-http
[httpserver-provider]: https://github.com/wasmCloud/wasmCloud/tree/main/crates/providers/http-server
[httpserver-interface]: https://github.com/wasmCloud/interfaces/tree/main/httpserver
[wadm]: https://github.com/wasmCloud/wadm
[wasmcloud]: https://wasmcloud.com/docs/intro
[rust]: https://rust-lang.org

# Dependencies

This relies on the following installed software:

| Name    | Description                                                                                                 |
|---------|-------------------------------------------------------------------------------------------------------------|
| `cargo` | [Rust package installer][cargo] (part of the Rust toolchain)                                                |
| `wash`  | [Wasmcloud Shell][wash] controls your [wasmcloud][wasmcloud] host instances and enables building components |

[wash]: https://github.com/wasmCloud/wasmCloud/tree/main/crates/wash-cli
[cargo]: https://doc.rust-lang.org/cargo

# Quickstart

To get started developing this repository quickly, clone the repo and run `wash dev`:

```console
wash dev
```

`wash dev` does many things for you:

- Starts the [wasmCloud host][wasmcloud-host] that can run your WebAssembly component
- Builds this project
- Builds a declarative WADM manifest consisting of:
  - Your locally built component
  - A [HTTP server provider][httpserver-provider] which will receive requests from the outside world (on port 8000 by default)
  - Necessary links between providers and your component so your component can handle web traffic
- Deploys the built manifest (i.e all dependencies to run this application) locally
- Watches your code for changes and re-deploys when necessary.

[wasmcloud-host]: https://wasmcloud.com/docs/concepts/hosts

## Send a request to the running component

Once `wash dev` is serving your component, to send a request to the running component (via the HTTP server provider):

```console
curl localhost:8000/api/v1/check --data '{"value": "test"}'
```

You should see a JSON response like:

```json
{
  "status": "success",
  "data": {
    "strength": "very-weak",
    "length": 4,
    "contains": [
      "lowercase"
    ]
}
```

## Adding Capabilities

To learn how to extend this example with additional capabilities, see the [Adding Capabilities](https://wasmcloud.com/docs/tour/adding-capabilities?lang=rust) section of the wasmCloud documentation.

# Issues/ FAQ

<summary>
<description>

## `curl` produces a "failed to invoke" error

</description>

If `curl`ing produces

```
➜ curl localhost:8000
failed to invoke `wrpc:http/incoming-handler.handle`: failed to invoke `wrpc:http/incoming-handler@0.1.0.handle`: failed to shutdown synchronous parameter channel: not connected%
```

You *may* need to just wait a little bit -- the HTTP server takes a second or two to start up.

If the issue *persists*, you *may* have a lingering HTTP server provider running on your system. You can use `pgrep` to find it:

```console
❯ pgrep -la ghcr_io
4007604 /tmp/wasmcloudcache/NBCBQOZPJXTJEZDV2VNY32KGEMTLFVP2XJRZJ5FWEJJOXESJXXR2RO46/ghcr_io_wasmcloud_http_server_0_23_1
```

</summary>
