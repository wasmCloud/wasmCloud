pub use wasmcloud_core::CallTargetInterface;

#[allow(clippy::doc_markdown)]
#[allow(missing_docs)]
mod wasmtime_bindings {
    mod keyvalue {
        pub type Bucket = std::sync::Arc<str>;
    }

    mod blobstore {
        pub type Container = std::sync::Arc<str>;
        pub type IncomingValue =
            core::pin::Pin<Box<dyn futures::Stream<Item = bytes::Bytes> + Send>>;
        pub type OutgoingValue = crate::io::AsyncVec;
        pub type StreamObjectNames = crate::io::BufferedIncomingStream<String>;
    }

    mod lattice {
        pub type CallTargetInterface = std::sync::Arc<wasmcloud_core::CallTargetInterface>;
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
        },
    });
}

#[allow(clippy::doc_markdown)]
#[allow(missing_docs)]
/// wRPC interface bindings
pub mod wrpc {
    wit_bindgen_wrpc::generate!({
        world: "wrpc-interfaces",
        with: {
            "wasi:blobstore/types@0.2.0-draft": generate,
            "wasi:io/error@0.2.0": generate,
            "wasi:io/poll@0.2.0": generate,
            "wasi:io/streams@0.2.0": generate,
            "wasmcloud:messaging/consumer@0.2.0": generate,
            "wasmcloud:messaging/handler@0.2.0": generate,
            "wasmcloud:messaging/types@0.2.0": generate,
            "wrpc:blobstore/blobstore@0.1.0": generate,
            "wrpc:blobstore/types@0.1.0": generate,
            "wrpc:keyvalue/atomics@0.2.0-draft": generate,
            "wrpc:keyvalue/store@0.2.0-draft": generate,
        }
    });
}

pub use wasmtime_bindings::wasi::{blobstore, config, keyvalue, logging};
pub use wasmtime_bindings::wasmcloud::{bus, messaging};
pub use wasmtime_bindings::Interfaces;
pub use wasmtime_wasi_http::bindings::http;
