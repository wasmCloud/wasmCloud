pub(crate) mod builtin;

/// Provider implementations
pub mod provider;

pub use builtin::{
    Blobstore, Bus, ComponentIdentifier, Config, IncomingHttp, KeyValueAtomics, KeyValueStore,
    LatticeInterfaceTarget, Logging, Messaging, MessagingHandler, OutgoingHttp,
    OutgoingHttpRequest, TargetEntity,
};
pub use wasmcloud_core::CallTargetInterface;

#[allow(clippy::doc_markdown)]
#[allow(missing_docs)]
mod bindgen {
    mod keyvalue {
        pub type Bucket = std::sync::Arc<String>;
    }

    mod blobstore {
        pub type Container = std::sync::Arc<String>;
        pub type IncomingValue = wrpc_transport_legacy::IncomingInputStream;
        pub type OutgoingValue = crate::io::AsyncVec;
        pub type StreamObjectNames = crate::io::BufferedIncomingStream<String>;
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
           "wasi:clocks": wasmtime_wasi::bindings::clocks,
           "wasi:http": wasmtime_wasi_http::bindings::http,
           "wasi:io": wasmtime_wasi::bindings::io,
           "wasi:keyvalue/store/bucket": keyvalue::Bucket,
           "wasmcloud:bus/lattice/call-target-interface": wasmcloud_core::CallTargetInterface,
        //    "wasi:sockets": wasmtime_wasi::bindings::sockets,
        //    "wasi:cli": wasmtime_wasi::bindings::cli,
        //    "wasi:filesystem": wasmtime_wasi::bindings::filesystem,
        //    "wasi:random": wasmtime_wasi::bindings::random,
        },
    });
}

pub use bindgen::wasi::{blobstore, config, keyvalue, logging};
pub use bindgen::wasmcloud::bus;
pub use bindgen::wasmcloud::messaging;
pub use bindgen::Interfaces;
pub use wasmtime_wasi_http::bindings::http;

fn format_opt<T>(opt: &Option<T>) -> &'static str {
    if opt.is_some() {
        "set"
    } else {
        "unset"
    }
}
