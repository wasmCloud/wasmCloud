# `http-hello-world-persistent-storage`

The Rust HTTP hello world component as it stands at the end of the [wasmcloud.com
quickstart's "Add persistent storage"][quickstart-persistent-storage] step:
the `templates/http-hello-world` starting point plus the `wasi:keyvalue` imports
in `wit/world.wit`, the `wit-bindgen` dependency in `Cargo.toml`, and the per-name
counter in `src/lib.rs`.

It exists as a build-tested checkpoint of that quickstart step. If a future
template change drifts from what the docs walk users through, this example's
build (run by the standard `examples` CI workflow) catches it.

[quickstart-persistent-storage]: https://wasmcloud.com/docs/quickstart/develop-a-webassembly-component#add-persistent-storage

## Run it

```shell
wash dev
```

```shell
curl 'http://localhost:8000/?name=Bailey'
# Hello x1, Bailey!
curl 'http://localhost:8000/?name=Bailey'
# Hello x2, Bailey!
```

The counter is persisted via `wasi:keyvalue/store` and `wasi:keyvalue/atomics` —
`wash dev` provides an in-memory implementation automatically.

## Build

```shell
wash build
```

## WIT

```wit
world hello {
  import wasi:keyvalue/store@0.2.0-draft;
  import wasi:keyvalue/atomics@0.2.0-draft;
}
```

Note: `wasi:http/incoming-handler` is exported via `wstd`'s `#[http_server]`
proc macro, so it does not appear explicitly in the world definition. Keeping
the export declared in `world.wit` while also calling `wit_bindgen::generate!`
makes the component encoder fail with *"failed to find export of interface
`wasi:http/incoming-handler@…` function `handle`"* — the macro and the manual
binding generation race over the same export symbol. (The
[wasmcloud.com quickstart prose][quickstart-persistent-storage] currently
shows the export staying in the world; that's a docs bug — tracked separately.)
