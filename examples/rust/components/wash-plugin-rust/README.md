# WAsmcloud SHell (`wash`) plugin

Starting from version 0.28 of the [WAsmcloud SHell (`wash`)][wash], [plugins written in WebAssembly][docs-wash-plugins] can be used to extend wash with new functionality.

This folder contains an example WebAssembly plugin for `wash` that you can use as a starting point for your own projects.

[wash]: https://wasmcloud.com/docs/cli
[docs-wash-plugins]: https://wasmcloud.com/docs/ecosystem/wash/plugins

## Prerequisites

- `cargo` >=1.75
- [`wash`](https://wasmcloud.com/docs/installation) >=0.28.1

## Building

You can build your plugin using either `cargo` or `wash`:

```console
wash build
```

Your component will be built, signed and placed in the `build` folder.

You can inspect your component with `wash inspect`:

```console
wash inspect ./build/http_blobstore_s.wasm
```

```
$ wash inspect build/wash_plugin_rust_s.wasm


                        wash-plugin-rust - Component
  Account         AB7WCKG4XTUEN524UIQIV7SHUG4G3HH6FXHPZZTZVWJDXWUPH4IX2NL4
  Component       MA6Y3RQ5UYKQSPGNNMTR4QEJSORMAW2WRUX4QB5BPJIUXOIFSSWFS4U7
  Expires                                                            never
  Can Be Used                                                  immediately
  Version                                                        0.1.0 (0)
  Embedded WIT                                                        true
                                    Tags
  wasmcloud.com/experimental
```

You can also use `cargo` to build your component:

```console
cargo build
```

> [!WARNING]
> Since this project represents a WebAssembly component, `cargo build` must be used with a WebAssembly friendly target.
> This works without specifying `--target` to `cargo`due to the settings in `.cargo/config.toml`.
>
> `wash` is recommended for building this component -- `cargo` does not sign your WebAssembly component or take into account settings in `wasmcloud.toml`.

## Using your new plugin from wasmCloud

> ![NOTE]
> Consider reading [the documentation for wash plugins][docs-wash-plugins]

### 1. Install your newly built plugin with `wash`

`wash` plugins are stored on disk at a location you choose (by specifying `WASH_PLUGIN_DIR`) -- by default it is `~/.wash/plugins`.

You can use the `wash install` subcommand to install your newly built plugin:

```console
wash plugin install build/wash_plugin_rust_s.wasm
```

Alternatively, you can copy the WebAssembly binary into `WASH PLUGIN_DIR` manually.

You can confirm your plugin is installed with `wash plugin list`:

```
➜ wash plugin list


  Name           ID      Version   Author      Description
  Hello Plugin   hello   0.1.0     WasmCloud   A simple plugin that says hello and logs a bunch of things
```

### 2. Use your new plugin with `wash` 

Plugins are accessible as top level subcommands -- given a plugin with the ID `hello` you can call `wash hello` to trigger it.

By default the example plugin in this folder has some required args, so a complete call would look like this:

```console
wash hello --foo . wasmcloud.toml
```
