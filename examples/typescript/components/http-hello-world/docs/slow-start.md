# Slow Start

Want to do more of the steps manually without using `wash dev`? You've come to the right place.

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

[wadm]: https://github.com/wasmCloud/wadm

## Send a request to the running component

To send a request to your component (via the HTTP server provider):

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
