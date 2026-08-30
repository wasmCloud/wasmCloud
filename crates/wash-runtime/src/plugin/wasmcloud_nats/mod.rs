//! `wasmcloud:nats` — NATS-native capabilities (core pub/sub, JetStream and
//! KV), split by interface.
//!
//! The plugin itself is in `plugin`; what a binding is described by is parsed
//! in `config`, and the long-lived subscription loops it spawns are in
//! `subscriber`.

pub(super) mod config;
pub(super) mod conn;
pub(super) mod interfaces;
pub(super) mod jetstream;
pub(super) mod keys;
pub(super) mod ledger;
pub(super) mod macros;
mod plugin;
pub(super) mod policy;
mod subscriber;
pub(super) mod warm;

pub use plugin::{ComponentData, WasmcloudNats};

/// How this plugin classifies its config keys, built from the one table that
/// also drives the reader — see [`keys`].
///
/// Closed: every key the plugin reads is named, so a manifest key it does not
/// recognize is refused rather than silently ignored. That is what keeps the
/// table honest, since a key added to `NatsConfig::from_map` and forgotten here
/// fails the first manifest that uses it.
///
/// Built once: the table is `const`, and this is asked for on every workload
/// bind.
#[must_use]
pub fn binding_schema() -> crate::plugin::BindingSchema {
    static SCHEMA: std::sync::LazyLock<crate::plugin::BindingSchema> =
        std::sync::LazyLock::new(|| {
            crate::plugin::BindingSchema::with_host_owned_keys(keys::host_owned())
                .and_host_ceiling_keys(keys::host_ceiling())
                .and_workload_owned_keys(keys::workload_owned())
        });
    SCHEMA.clone()
}

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

/// This plugin's host-unique id, and the key an operator declares it under
/// in `host.plugins`.
pub const PLUGIN_NATS_ID: &str = "wasmcloud-nats";

const NATS_VERSION: &str = "0.1.0";
