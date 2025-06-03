# 🚌 `wasmcloud:bus` WIT interface

This folder contains [WIT][wit] definitions for `wasmcloud:bus`, an interface for interacting with advanced
wasmCloud features like communicating over the lattice by setting [link names][docs-links].

[wit]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md
[docs-links]: https://wasmcloud.com/docs/concepts/linking-components/linking-at-runtime#defining-and-using-link-names

## 👟 Using this WIT interface

`wasmcloud:bus` is implemented by wasmCloud host, so this WIT interface may be imported by components that need to communicate over the lattice.

### 🏗️ Building WebAssembly components

These definitions are meant to be used while *creating* WebAssembly components, with whatever language toolchain is available to you.

| Language   | Toolchain                            |
|------------|--------------------------------------|
| Javascript | [`jco`][jco]  |
| Rust       | [`cargo-component`][cargo-component] |
| Rust       | [`wit-bindgen`][wit-bindgen-rust]    |
| Python     | [`componentize-py`][compnentize-py]  |
| Golang     | [`wit-bindgen`][wit-bindgen-go]      |

Depending on which language and toolchain you use, the specifics differ, but you should end up with a project that contains a `wit` folder.

[cargo-component]: https://github.com/bytecodealliance/cargo-component
[compnentize-py]: https://github.com/bytecodealliance/componentize-py
[jco]: https://github.com/bytecodealliance/jco
[wit-bindgen-go]: https://github.com/bytecodealliance/wit-bindgen?tab=readme-ov-file#guest-tinygo
[wit-bindgen-rust]: https://github.com/bytecodealliance/wit-bindgen

### ⬇️ Downloading this WIT

While ecosystem tooling for pulling and using WIT-manifest-only components develops, the easiest way to get started is to use [`wit-deps`][wit-deps], a dependency manager for WIT files.

In your project, include the following `wit/deps.toml`:

```yaml
bus = "https://github.com/wasmCloud/wasmCloud/releases/download/wit-wasmcloud-bus-v1.0.0/wit-wasmcloud-bus-0.1.0.tar.gz"
```

From your project root (the folder above `wit/`), you should be able to run `wit-deps`:

```console
wit-deps
```

This will populate a `wit/deps` folder and create `wit/deps.lock`.

[wit-deps]: https://github.com/bytecodealliance/wit-deps

### 🚀 Using interfaces in the `wasmcloud:bus` WIT package

Using interfaces in the `wasmcloud:bus` WIT package from your language of choice depends primarily on your language, toolchain, and the WebAssembly runtime you're using -- here are some examples for reference.

#### Guest: Rust

If using the Rust ecosystem with `wit-bindgen`, you might have a WIT `world` that looks like the following:

```wit
package wasmcloud:examples;

/// Invoke a component and receive string output. Similar to wasi:cli/command.run, without args
///
/// This enables the component to be used with `wash call`
interface invoke {
    /// Invoke a component
    call: func() -> string;
}

world component {
  import wasmcloud:bus/lattice@1.0.0;
  import wasi:keyvalue/store@0.2.0-draft;
  export invoke;
}
```

To build a WebAssembly component that satisfies that `world`, you might write code that looks like this:

```rust
let yourinterface = wasmcloud::bus::lattice::CallTargetInterface::new(
    "wasi",
    "keyvalue",
    "store",
);
// Sets the operative link for interface to the named link foo
wasmcloud::bus::lattice::set_link_name("foo", yourinterface);
// Calls over link foo to perform a keyvalue operation
let x = wasi::keyvalue::store::function(args);
// Sets the operative link for interface to the named link bar
wasmcloud::bus::lattice::set_link_name("bar", yourinterface);
// Calls over link bar to perform a keyvalue operation
let y = wasi::keyvalue::store::function(args);
```

For the code above to work inside a component, you must deploy:

- The [wasmCloud host][docs-host]
- The component above, compiled with [`wash`][docs-wash]
- Create two links for `wasi:keyvalue/store` interface with link names "foo" and "bar".

See [wasmCloud documentation on how runtime linking works](https://wasmcloud.com/docs/concepts/linking-components/linking-at-runtime) for more information.

[docs-host]: https://wasmcloud.com/docs/concepts/hosts
[docs-wash]: https://wasmcloud.com/docs/ecosystem/wash/
