pub use wasmcloud_core::CallTargetInterface;

#[allow(clippy::doc_markdown)]
#[allow(missing_docs)]
mod wasmtime_bindings {
    mod blobstore {
        pub type Container = std::sync::Arc<str>;
        pub type IncomingValue = crate::component::blobstore::IncomingValue;
        pub type OutgoingValue = crate::component::blobstore::OutgoingValue;
        pub type StreamObjectNames = crate::component::blobstore::StreamObjectNames;
    }

    mod keyvalue {
        pub type Bucket = std::sync::Arc<str>;
    }

    mod lattice {
        pub type CallTargetInterface = std::sync::Arc<wasmcloud_core::CallTargetInterface>;
    }

    mod messaging0_3_0 {
        pub type Message = crate::component::messaging::v0_3::Message;
        pub type RequestOptions = crate::component::messaging::v0_3::RequestOptions;
        pub type Client = Box<dyn crate::component::messaging::v0_3::Client + Send + Sync>;
    }

    mod secrets {
        use super::wasmcloud::secrets::store::SecretValue;

        pub type Secret = std::sync::Arc<String>;

        impl secrecy::zeroize::Zeroize for SecretValue {
            fn zeroize(&mut self) {
                match self {
                    SecretValue::String(s) => s.zeroize(),
                    SecretValue::Bytes(b) => b.zeroize(),
                }
            }
        }

        /// Permits cloning
        impl secrecy::CloneableSecret for SecretValue {}
    }

    wasmtime::component::bindgen!({
        world: "interfaces",
        async: true,
        tracing: true,
        trappable_imports: true,
        with: {
           "wasi:blobstore/container/container": blobstore::Container,
           "wasi:blobstore/container/stream-object-names": blobstore::StreamObjectNames,
           "wasi:blobstore/types/incoming-value": blobstore::IncomingValue,
           "wasi:blobstore/types/outgoing-value": blobstore::OutgoingValue,
           "wasi:io": wasmtime_wasi::bindings::io,
           "wasi:keyvalue/store/bucket": keyvalue::Bucket,
           "wasmcloud:bus/lattice/call-target-interface": lattice::CallTargetInterface,
           "wasmcloud:messaging/types@0.3.0/client": messaging0_3_0::Client,
           "wasmcloud:messaging/types@0.3.0/message": messaging0_3_0::Message,
           "wasmcloud:messaging/request-reply@0.3.0/request-options": messaging0_3_0::RequestOptions,
           "wasmcloud:secrets/store/secret": secrets::Secret,
        },
    });
}

#[allow(missing_docs)]
mod unversioned_logging_bindings {
    wasmtime::component::bindgen!({
        world: "unversioned-interfaces",
        async: true,
        tracing: true,
        trappable_imports: true,
    });
}

#[allow(missing_docs)]
mod config_legacy {
    wasmtime::component::bindgen!({
        path: "wit/config-legacy",
        world: "wasi:config/imports@0.2.0-draft",
        async: true,
        tracing: true,
        trappable_imports: true,
    });
}

#[allow(clippy::doc_markdown)]
#[allow(missing_docs)]
/// wRPC interface bindings
pub mod wrpc {
    wit_bindgen_wrpc::generate!({
        world: "wrpc-interfaces",
        with: {
            "wasi:blobstore/types@0.2.0-draft": wrpc_interface_blobstore::bindings::wasi::blobstore::types,
            "wrpc:blobstore/types@0.1.0": wrpc_interface_blobstore::bindings::wrpc::blobstore::types,
        },
        generate_all,
    });
}

/// `wasi:config` bindings
pub mod config {
    pub use super::config_legacy::wasi::config::runtime;
    pub use super::wasmtime_bindings::wasi::config::store;
}

impl std::fmt::Display for wasmtime_bindings::wasi::logging0_1_0_draft::logging::Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Trace => "trace",
                Self::Debug => "debug",
                Self::Info => "info",
                Self::Warn => "warn",
                Self::Error => "error",
                Self::Critical => "critical",
            }
        )
    }
}

pub use unversioned_logging_bindings::wasi::logging as unversioned_logging;
pub use wasmtime_bindings::wasi::{blobstore, keyvalue, logging0_1_0_draft as logging};
pub use wasmtime_bindings::wasmcloud::{
    bus1_0_0, bus2_0_0 as bus, bus2_0_0, messaging0_2_0, messaging0_3_0 as messaging,
    messaging0_3_0, secrets,
};
pub use wasmtime_bindings::Interfaces;
pub use wasmtime_wasi_http::bindings::http;
