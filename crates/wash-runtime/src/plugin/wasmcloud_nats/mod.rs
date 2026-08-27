//! `wasmcloud:nats` — NATS-native capabilities (core pub/sub, JetStream and
//! KV), split by interface.
//!
//! The plugin itself is in `plugin`; what a binding is described by is parsed
//! in `config`, and the long-lived subscription loops it spawns are in
//! `subscriber`.

pub mod bindings;
pub(super) mod config;
pub(super) mod conn;
pub(super) mod interfaces;
pub(super) mod jetstream;
pub(super) mod ledger;
pub(super) mod macros;
mod plugin;
pub(super) mod policy;
mod subscriber;
pub(super) mod warm;

pub use bindings::{NatsBindings, WorkloadConfig};
pub use plugin::{ComponentData, WasmcloudNats};

// Handler worlds. Each lives in its own module so their duplicate import types
// don't collide, and so a component exporting only one handler still
// pre-instantiates. Every export is an `async func`, so each is driven inside
// `store.run_concurrent(..)` — see [`subscriber`].
pub(super) mod jetstream_bindings {
    crate::wasmtime::component::bindgen!({
        world: "wasmcloud:nats/js-processor@0.1.0",
        imports: { default: async | trappable | tracing },
        exports: { default: async | tracing },
        with: {
            "wasmcloud:nats/jetstream@0.1.0.message-handle": super::jetstream::MessageHandle,
            "wasmcloud:nats/jetstream@0.1.0.pull-consumer": super::jetstream::PullConsumerHandle,
        },
    });
}

pub(super) mod core_bindings {
    crate::wasmtime::component::bindgen!({
        world: "wasmcloud:nats/subscriber@0.1.0",
        imports: { default: async | trappable | tracing },
        exports: { default: async | tracing },
    });
}

pub(super) mod kv_bindings {
    crate::wasmtime::component::bindgen!({
        world: "wasmcloud:nats/kv-watcher@0.1.0",
        imports: { default: async | trappable | tracing },
        exports: { default: async | tracing },
        with: {
            "wasmcloud:nats/kv@0.1.0.bucket": super::jetstream::BucketHandle,
        },
    });
}

pub(super) const PLUGIN_NATS_ID: &str = "wasmcloud-nats";

const NATS_VERSION: &str = "0.1.0";
