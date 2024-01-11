# `wasmcloud-provider-wit-bindgen`

This crate contains a macro that helps build [Wasmcloud Capability Providers][wasmcloud-capability-providers] which are distributed as binaries and work with the [WIT][wit].

This crate leverages [`wit-bindgen`][wit-bindgen] in it's generation of interfaces and code, but generates pure Rust code (i.e. for non-`wasm32-*` Rust targets) that can be used in building a capability provider binary.

## Usage

This crate can be be used similarly to `wit-bindgen`, with the following syntax:

```rust
wasmcloud_provider_wit_bindgen::generate!(
    YourProvider,
    "wasmcloud:contract",
    "your-world"
);

/// Implementation that you will contribute
struct YourProvider;

impl Trait for YourProvider {
  ...
}
```

Note that arguments after the second are similar to the options used by `wit-bindgen`, but the code generated is meant for use in a Rust binary, managed by a [`wasmcloud` host][wasmcloud-host].

For example, to build a provider for the [wasmCloud keyvalue WIT interface][wasmcloud-keyvalue]:

```rust
/// Generate bindings for a wasmCloud provider
wasmcloud_provider_wit_bindgen::generate!(
    MyKeyvalueProvider,
    "wasmcloud:keyvalue",
    "keyvalue"
);
```

> **Warning**
> You'll need to have the appropriate WIT interface file (ex. `keyvalue.wit`) in your crate root, at `<crate root>/wit/keyvalue.wit`

Note that after you generate bindings appropriate for your WIT, you must:

- follow the compiler to implement the appropriate traits
- write a `main.rs` that properly sets up your provider
- use the compiled binary for your provider on your wasmCloud lattice

[wit-bindgen]: https://github.com/bytecodealliance/wit-bindgen
[wasmcloud-capability-providers]: https://wasmcloud.com/docs/fundamentals/capabilities/
[wit]: https://github.com/WebAssembly/component-model/blob/main/design/mvp/WIT.md
[wasmcloud-keyvalue]: https://github.com/wasmCloud/interfaces/blob/main/wit/keyvalue.wit
[wasmcloud-host]: https://github.com/wasmCloud/wasmCloud
