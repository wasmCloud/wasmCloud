pub(crate) mod builtin;

/// Provider implementations
pub mod provider;

pub use builtin::{
    ActorIdentifier, Blobstore, Bus, IncomingHttp, KeyValueAtomic, KeyValueEventual, Logging,
    Messaging, OutgoingHttp, OutgoingHttpRequest, TargetEntity, WrpcInterfaceTarget,
};

// NOTE: this import is used below in bindgen
pub use builtin::CallTargetInterface;

#[allow(clippy::doc_markdown)]
#[allow(missing_docs)]
mod bindgen {
    use wasmtime_wasi::preview2;

    mod keyvalue {
        pub type Bucket = std::sync::Arc<String>;
        pub type IncomingValue = (Box<dyn tokio::io::AsyncRead + Send + Sync + Unpin>, u64);
        pub type OutgoingValue = crate::io::AsyncVec;
        pub type Error = anyhow::Error;
    }

    mod blobstore {
        pub type Container = std::sync::Arc<String>;
        pub type IncomingValue = (Box<dyn tokio::io::AsyncRead + Send + Sync + Unpin>, u64);
        pub type OutgoingValue = crate::io::AsyncVec;
        pub type StreamObjectNames =
            Box<dyn futures::Stream<Item = anyhow::Result<String>> + Sync + Send + Unpin>;
    }

    wasmtime::component::bindgen!({
        world: "interfaces",
        async: true,
        with: {
           "wasi:blobstore/container/container": blobstore::Container,
           "wasi:blobstore/container/stream-object-names": blobstore::StreamObjectNames,
           "wasi:blobstore/types/incoming-value": blobstore::IncomingValue,
           "wasi:blobstore/types/outgoing-value": blobstore::OutgoingValue,
           "wasi:cli/environment": preview2::bindings::cli::environment,
           "wasi:cli/exit": preview2::bindings::cli::exit,
           "wasi:cli/preopens": preview2::bindings::cli::preopens,
           "wasi:cli/stderr": preview2::bindings::cli::stderr,
           "wasi:cli/stdin": preview2::bindings::cli::stdin,
           "wasi:cli/stdout": preview2::bindings::cli::stdout,
           "wasi:clocks/monotonic-clock": preview2::bindings::clocks::monotonic_clock,
           "wasi:clocks/timezone": preview2::bindings::clocks::timezone,
           "wasi:clocks/wall_clock": preview2::bindings::clocks::wall_clock,
           "wasi:filesystem/filesystem": preview2::bindings::filesystem::filesystem,
           "wasi:http/incoming-handler": wasmtime_wasi_http::bindings::http::incoming_handler,
           "wasi:http/outgoing-handler": wasmtime_wasi_http::bindings::http::incoming_handler,
           "wasi:http/types": wasmtime_wasi_http::bindings::http::types,
           "wasi:io/error": preview2::bindings::io::error,
           "wasi:io/poll": preview2::bindings::io::poll,
           "wasi:io/streams": preview2::bindings::io::streams,
           "wasi:keyvalue/types/bucket": keyvalue::Bucket,
           "wasi:keyvalue/types/incoming-value": keyvalue::IncomingValue,
           "wasi:keyvalue/types/outgoing-value": keyvalue::OutgoingValue,
           "wasi:keyvalue/wasi-keyvalue-error/error": keyvalue::Error,
           "wasi:random/random": preview2::bindings::random::random,
           "wasmcloud:bus/lattice/call-target-interface": super::CallTargetInterface,
        },
    });
}

pub use bindgen::wasi::{blobstore, keyvalue, logging};
pub use bindgen::wasmcloud::{
    bus::{self, guest_config},
    messaging,
};
pub use bindgen::Interfaces;
pub use wasmtime_wasi_http::bindings::http;

fn format_opt<T>(opt: &Option<T>) -> &'static str {
    if opt.is_some() {
        "set"
    } else {
        "unset"
    }
}
