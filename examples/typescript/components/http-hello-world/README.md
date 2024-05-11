# Typescript HTTP Hello World

This repository contains a hello world HTTP component, written in [Typescript][ts].

This component:

- Uses Typescript for it's implementation
- Uses the [`wasi:http`][wasi-http] standard WIT definitions
- Relies on the [`httpserver` capability provider][httpserver-provider] (which exposes the [`wasmcloud:httpserver` interface][httpserver-interface])
- Return `"hello from Typescript"` to all HTTP requests
- Can be declaratively provisioned with [`wadm`][wadm]

[ts]: https://www.typescriptlang.org/
[wasi-http]: https://github.com/WebAssembly/wasi-http
[httpserver-provider]: https://github.com/wasmCloud/wasmCloud/tree/main/crates/providers/http-server
[httpserver-interface]: https://github.com/wasmCloud/interfaces/tree/main/httpserver
[wadm]: https://github.com/wasmCloud/wadm

# Dependencies

This relies on the following installed software:

| Name   | Description                                                                                                 |
| ------ | ----------------------------------------------------------------------------------------------------------- |
| `wash` | [Wasmcloud Shell][wash] controls your [wasmcloud][wasmcloud] host instances and enables building components |
| `npm`  | [Node Package Manager (NPM)][npm] which manages packages for for the NodeJS ecosystem                       |
| `node` | [NodeJS runtime][nodejs] (see `.nvmrc` for version)                                                         |

[wash]: https://github.com/wasmCloud/wasmCloud/tree/main/crates/wash-cli
[node]: https://nodejs.org
[npm]: https://github.com/npm/cli

# Get started

## Install NodeJS dependencies

If you have the basic dependencies installed, you can install NodeJS-level dependcies:

```console
npm install
```

## Start a wasmcloud host

To start a wasmcloud host you can use `wash`:

```console
wash up
```

This command won't return (as it's the running host process), but you can view the output of the host.

## Build the component

To build the [component][wasmcloud-component], we can use `wash`:

```console
wash build
```

This will build and sign the component and place a signed [WebAssembly component][wasm-component] at `build/index_s.wasm`.

`build` performs many substeps (see `package.json` for details):

- (`build:tsc`) transpiles Typescript code into Javascript code
- (`build:js`) builds a javascript module runnable in NodeJS from a [WebAssembly component][wasm-component] using the [`jco` toolchain][jco]
- (`build:component`) build and sign a WebAssembly component for this component using `wash`

[wasmcloud-component]: https://wasmcloud.com/docs/concepts/webassembly-components
[wasm-component]: https://component-model.bytecodealliance.org/
[jco]: https://github.com/bytecodealliance/jco

## Start the component along with the HTTP server provider

To start the component, HTTP server provider and everything we need to run:

```console
npm run wadm:start
```

This command will deploy the application to your running wasmcloud host, using [`wadm`][wadm], a declarative WebAssembly orchestrator.

## Send a request to the running component

To send a request to the running component (via the HTTP server provider):

```console
curl localhost:8081
```

> [!NOTE]
> Confused as to why it is port 8081?
>
> See `typescript-http-hello-world.wadm.yaml` for more information on the pieces of the architecture;
> components, providers, and link definitions.

## (Optional) reload on code change

To quickly reload your application after changing the code in `index.ts`:

```console
npm run reload
```

## Adding Capabilities

To learn how to extend this example with additional capabilities, see the [Adding Capabilities](https://wasmcloud.com/docs/tour/adding-capabilities?lang=typescript) section of the wasmCloud documentation.
