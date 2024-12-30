# wasmCloud Examples

This folder contains examples of projects you can run with wasmCloud.

## Dependencies

Before you can run these examples, you'll need the [wasmCloud Shell (`wash`)][wash] installed on your machine.

`wash` is used for both starting a new host (`wash up`) and performing actions on an existing host like starting a workload ([WebAssembly components][docs-components] or [capability providers][docs-providers]).

Most examples will use [wasmCloud Application Deployment Manager (`wadm`)][wadm], which is built into `wash` to declaratively launch workloads. Usually, you'll be running `wash app deploy path/to/wadm.yaml`.

[wash]: https://wasmcloud.com/docs/cli
[wadm]: https://github.com/wasmCloud/wadm

## Try out WebAssembly Components

wasmCloud runs [WebAssembly Components][docs-components]. Write business logic in your favorite `$LANGUAGE`, compile to WebAssembly, and deploy your code to hosts on a local or remote [wasmCloud lattices][docs-lattice].

**Want to write some code that responds to web requests, talks to a database and does some logging? That's a WebAssembly component.**

You can get started quickly by trying out of our example projects:

| Language                  | Where you should start                                                                 | OCI Artifact                                                    |
|---------------------------|----------------------------------------------------------------------------------------|-----------------------------------------------------------------|
| Golang ([TinyGo][tinygo]) | [`./golang/components/http-hello-world`](./golang/components/http-hello-world)         |                                                                 |
| Golang ([TinyGo][tinygo]) | [`./golang/components/http-echo-tinygo`](./golang/components/http-echo-tinygo)         |                                                                 |
| [Python][python]          | [`./python/components/http-hello-world`](./golang/components/http-hello-world)         |                                                                 |
| [Rust][rust]              | [`./rust/components/echo-messaging`](./rust/components/echo-messaging)               | `ghcr.io/wasmcloud/components/echo-messaging-rust:0.1.0`        |
| [Rust][rust]              | [`./rust/components/blobby`](./rust/components/blobby)                               | `ghcr.io/wasmcloud/components/blobby-rust:0.4.0`                |
| [Rust][rust]              | [`./rust/components/http-hello-world`](./rust/components/http-hello-world)           | `ghcr.io/wasmcloud/components/http-hello-world-rust:0.1.0`      |
| [Rust][rust]              | [`./rust/components/http-jsonify`](./rust/components/http-jsonify)                   | `ghcr.io/wasmcloud/components/http-jsonify-rust:0.1.1`          |
| [Rust][rust]              | [`./rust/components/http-keyvalue-counter`](./rust/components/http-keyvalue-counter) | `ghcr.io/wasmcloud/components/http-keyvalue-counter-rust:0.1.0` |
| [TypeScript][typescript]  | [`wasmCloud/typescript#examples/components/http-hello-world`](https://github.com/wasmCloud/typescript/tree/main/examples/components/http-hello-world) |  |

Start components with `wash`, either from file or OCI reference (if available):

```console
wash start component file:///path/to/examples/project/build/name_of_component_s.wasm component
wash start component ghcr.io/wasmcloud/components/http-jsonify-rust:0.1.1 http-jsonfiy
```

> [!WARNING]
> Not all WebAssembly component examples have officially maintained OCI artifacts.
>
> Want to use a component from your own registry? After `wash build`ing your component, `wash push` it to your registry of choice.

[docs-components]: https://wasmcloud.com/docs/concepts/components
[docs-providers]: https://wasmcloud.com/docs/concepts/providers
[docs-lattice]: https://wasmcloud.com/docs/concepts/lattice
[rust]: https://rust-lang.org
[tinygo]: https://tinygo.org
[python]: https://python.org
[typescript]: https://typescriptlang.org

## Try out Capability Providers

wasmCloud runs [capability providers][docs-providers] which are binaries that "provide" stateful and/or advanced functionality to the rest of the wasmCloud lattice. While components contain high level business logic and are quite light, providers normally contain more low level implementation that components rely on.

**Want to implement an interface for reading and writing key-value data that speaks to a real database? That's a wasmCloud provider.**

wasmCloud capability providers run as child processes on the wasmCloud host, and are mostly unrestricted, meaning you don't have to wait for WebAssembly ecosystem support for your code -- whatever advanced functionality (connecting to databases, AI/ML, raw hardware access, proprietary software, etc) you can build into a regular binary today can exposed for WebAssembly components running on your lattice to consume.

You can get started quickly by trying out of our example projects:

| Language     | Folder                                                               | OCI Artifact |
|--------------|----------------------------------------------------------------------|--------------|
| [Rust][rust] | [`./rust/providers/messaging-nats`](./rust/providers/messaging-nats) |              |

> [!WARNING]
> Most wasmCloud example providers do *not* have officially maintained OCI artifacts.
>
> Want to use a provider from your own registry? Build a Provider ARchive (PAR) file with `wash par` and `wash push` it to your registry of choice.
