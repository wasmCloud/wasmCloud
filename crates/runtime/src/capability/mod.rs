pub(crate) mod builtin;

/// Provider implementations
pub mod provider;

pub use builtin::{
    ActorIdentifier, Blobstore, Bus, IncomingHttp, KeyValueAtomic, KeyValueEventual,
    LatticeInterfaceTarget, Logging, Messaging, MessagingHandler, OutgoingHttp,
    OutgoingHttpRequest, TargetEntity,
};

// NOTE: this import is used below in bindgen
pub use wasmcloud_core::CallTargetInterface;

#[allow(clippy::doc_markdown)]
#[allow(missing_docs)]
mod bindgen {
    mod keyvalue {
        pub type Bucket = std::sync::Arc<String>;
        pub type IncomingValue = wrpc_transport::IncomingInputStream;
        pub type OutgoingValue = crate::io::AsyncVec;
        pub type Error = anyhow::Error;
    }

    mod blobstore {
        pub type Container = std::sync::Arc<String>;
        pub type IncomingValue = wrpc_transport::IncomingInputStream;
        pub type OutgoingValue = crate::io::AsyncVec;
        pub type StreamObjectNames = crate::io::BufferedIncomingStream<String>;
    }

    wasmtime::component::bindgen!({
        world: "interfaces",
        async: true,
        with: {
           "wasi:blobstore/container/container": blobstore::Container,
           "wasi:blobstore/container/stream-object-names": blobstore::StreamObjectNames,
           "wasi:blobstore/types/incoming-value": blobstore::IncomingValue,
           "wasi:blobstore/types/outgoing-value": blobstore::OutgoingValue,
           "wasi:cli/environment": wasmtime_wasi::bindings::cli::environment,
           "wasi:cli/exit": wasmtime_wasi::bindings::cli::exit,
           "wasi:cli/preopens": wasmtime_wasi::bindings::cli::preopens,
           "wasi:cli/stderr": wasmtime_wasi::bindings::cli::stderr,
           "wasi:cli/stdin": wasmtime_wasi::bindings::cli::stdin,
           "wasi:cli/stdout": wasmtime_wasi::bindings::cli::stdout,
           "wasi:clocks/monotonic-clock": wasmtime_wasi::bindings::clocks::monotonic_clock,
           "wasi:clocks/timezone": wasmtime_wasi::bindings::clocks::timezone,
           "wasi:clocks/wall_clock": wasmtime_wasi::bindings::clocks::wall_clock,
           "wasi:filesystem/filesystem": wasmtime_wasi::bindings::filesystem::filesystem,
           "wasi:http/incoming-handler": wasmtime_wasi_http::bindings::http::incoming_handler,
           "wasi:http/outgoing-handler": wasmtime_wasi_http::bindings::http::incoming_handler,
           "wasi:http/types": wasmtime_wasi_http::bindings::http::types,
           "wasi:io/error": wasmtime_wasi::bindings::io::error,
           "wasi:io/poll": wasmtime_wasi::bindings::io::poll,
           "wasi:io/streams": wasmtime_wasi::bindings::io::streams,
           "wasi:keyvalue/types/bucket": keyvalue::Bucket,
           "wasi:keyvalue/types/incoming-value": keyvalue::IncomingValue,
           "wasi:keyvalue/types/outgoing-value": keyvalue::OutgoingValue,
           "wasi:keyvalue/wasi-keyvalue-error/error": keyvalue::Error,
           "wasi:random/random": wasmtime_wasi::bindings::random::random,
           "wasmcloud:bus/lattice/call-target-interface": super::CallTargetInterface,
        },
    });
}

pub use bindgen::wasi::{blobstore, keyvalue, logging};
pub use bindgen::wasmcloud::bus::{self, guest_config};
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
