# TinyGo HTTP Password Checker

This repository contains a WebAssembly Component written in [TinyGo][tinygo], which:

- Implements a [`wasi:http`][wasi-http]-compliant HTTP handler
- Uses the [`httpserver` provider][httpserver-provider] to serve requests
- Can be declaratively provisioned with [`wadm`][wadm]

[wasi-http]: https://github.com/WebAssembly/wasi-http
[httpserver-provider]: https://github.com/wasmCloud/wasmCloud/tree/main/crates/provider-http-server
[blobstore-provider]: https://github.com/wasmCloud/wasmCloud/tree/main/crates/provider-blobstore-fs
[wadm]: https://github.com/wasmCloud/wadm
[tinygo]: https://tinygo.org/getting-started/install/
[wash]:  https://wasmcloud.com/docs/ecosystem/wash/

# Dependencies

Before starting, ensure that you have the following installed:

- The [TinyGo toolchain][tinygo]
- [`wash`, the WAsmcloud SHell][wash] installed.

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
  - A [Blobstore provider][blobstore-provider] which will serve a file containing the 500 worst passwords
  - Necessary links between providers and your component so your component can handle web traffic
- Deploys the built manifest (i.e all dependencies to run this application) locally
- Watches your code for changes and re-deploys when necessary.

[wasmcloud-host]: https://wasmcloud.com/docs/concepts/hosts

## Send a request to the running component

Once `wash dev` is serving your component, to send a request to the running component (via the HTTP server provider):

```console
curl localhost:8000/check -d '{"value": "tes12345!"}'                                                                                         [23:01:58]
```

You should see a JSON response like:

```json
{
    "valid": false, "message": "insecure password, try including more special characters, using uppercase letters or using a longer password"

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
