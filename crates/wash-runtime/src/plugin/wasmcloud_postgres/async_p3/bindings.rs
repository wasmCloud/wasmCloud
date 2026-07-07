//! Generated wasmtime host bindings for the async `wasmcloud:postgres@0.2.0`
//! world.
//!
//! Two variants of the same world: the plain one, and — under
//! `wasm_component_model_implements` — one that additionally generates
//! `named_imports::*` so a component can import the interfaces multiple times via
//! `(implements ..)`, each routed to its own credentialed connection pool.

#[cfg(not(feature = "wasm_component_model_implements"))]
crate::wasmtime::component::bindgen!({
    world: "async-postgres",
    imports: { default: store | async | trappable | tracing },
});

#[cfg(feature = "wasm_component_model_implements")]
crate::wasmtime::component::bindgen!({
    world: "async-postgres",
    imports: { default: store | async | trappable | tracing },
    named_imports: {
        "wasmcloud:postgres/query@0.2.0": crate::plugin::wasmcloud_postgres::PgId,
        "wasmcloud:postgres/prepared@0.2.0": crate::plugin::wasmcloud_postgres::PgId,
    },
});
