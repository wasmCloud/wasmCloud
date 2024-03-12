# `wasmcloud-provider-wit-bindgen`

This crate contains a Rust compile time [procedural macro][proc-macro] which makes it easy to develop [WIT-based][wit] [wasmcloud capbility provider][wasmcloud-cap-provider] binaries written in Rust.

## Quickstart

It all starts with `wasmcloud_provider_wit_bindgen::generate`, for example:

```rust
wasmcloud_provider_wit_bindgen::generate!({
    impl_struct: KvRedisProvider,
    contract: "wasmcloud:keyvalue",
    wit_bindgen_cfg: "provider-kvredis"
});
```

Assuming a struct named `KvRedisProvider` exists in your source code and you have a `provider-kvredis` world defined in your `wit/` folder, the above macro will expand to `Trait`s, `struct`s, and other required machinery to implement a wasmcloud capability provider.

> [!NOTE]
> For a full example, see the [`kv-redis` provider in wasmcloud][kvredis-provider]

By using the `generate` macro, you'll be required to write implementation blocks like the following:

```rust
impl WasmcloudCapabilityProvider for KvRedisProvider {
    async fn put_link(&self, ld: &LinkDefinition) -> bool { ... }
    async fn delete_link(&self, actor_id: &str) { ... }
    async fn shutdown(&self) { ... }
}
```

```rust
impl WasmcloudKeyvalueKeyValue for KvRedisProvider {
    async fn get(&self, ctx: Context, arg: String) -> ProviderInvocationResult<GetResponse> { ... }
    async fn set(&self, ctx: Context, arg: SetRequest) -> ProviderInvocationResult<()> { ... }
    async fn del(&self, ctx: Context, arg: String) -> ProviderInvocationResult<bool> { ... }
    ...
}
```

Don't worry, types like `SetRequest` and `GetResponse` above will be provided *by the expanded macro code*.

## Re-exports

Note that `wasmcloud-provider-wit-bindgen` re-exports many dependencies in order to ensure that they match and are usable together:

- [`serde`](https://crates.io/crates/serde)
- [`serde_json`](https://crates.io/crates/serde_json)
- [`serde_bytes`](https://crates.io/crates/serde_bytes)
- [`async-trait`](https://crates.io/crates/async-trait)
- [`wasmcloud-provider-sdk`](https://crates.io/crates/wasmcloud-provider-sdk)

It's recommended to use these dependencies in your code to avoid duplicating dependencies which could lead to all sorts of problems. For example the following `use` block:

```rust
use wasmcloud_provider_wit_bindgen::deps::{
    async_trait::async_trait,
    serde::Deserialize,
    serde_json,
    wasmcloud_provider_sdk::core::LinkDefinition,
    wasmcloud_provider_sdk::{load_host_data, start_provider, Context},
};
```

### Special case: re-using `serde`

When re-using a re-exported `serde`, a [known issue](https://github.com/serde-rs/serde/issues/1465) exists which requires that you must use the `#[serde(crate = "...")]` directive:

```rust
#[derive(Deserialize)]
#[serde(crate = "wasmcloud_provider_wit_bindgen::deps::serde")]
struct ExampleStruct {
    /// Some string that is part of this struct
    #[serde(alias = "WORDS", alias = "Words")]
    words: String,
}
```

[proc-macro]: https://doc.rust-lang.org/reference/procedural-macros.html
[kvredis-provider]: https://github.com/wasmCloud/wasmCloud/tree/main/crates/providers/kv-redis
[wasmcloud-cap-provider]: https://wasmcloud.com/docs/concepts/capabilities#capability-providers
[wit]: https://wasmcloud.com/docs/concepts/interface-driven-development#webassembly-interface-type-wit
