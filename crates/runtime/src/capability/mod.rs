pub(crate) mod builtin;

/// Provider implementations
pub mod provider;

pub use builtin::{
    ActorIdentifier, Blobstore, Bus, IncomingHttp, KeyValueAtomic, KeyValueReadWrite, Logging,
    Messaging, OutgoingHttp, OutgoingHttpRequest, TargetEntity, TargetInterface,
};

#[allow(clippy::doc_markdown)]
#[allow(missing_docs)]
mod bindgen {
    use wasmtime_wasi::preview2;

    wasmtime::component::bindgen!({
        world: "interfaces",
        async: true,
        with: {
           "wasmcloud:bus/lattice/target-interface": super::TargetInterface,
           "wasi:cli/environment@0.2.0-rc-2023-10-18": preview2::bindings::cli::environment,
           "wasi:cli/exit@0.2.0-rc-2023-10-18": preview2::bindings::cli::exit,
           "wasi:cli/preopens@0.2.0-rc-2023-10-18": preview2::bindings::cli::preopens,
           "wasi:cli/stderr@0.2.0-rc-2023-10-18": preview2::bindings::cli::stderr,
           "wasi:cli/stdin@0.2.0-rc-2023-10-18": preview2::bindings::cli::stdin,
           "wasi:cli/stdout@0.2.0-rc-2023-10-18": preview2::bindings::cli::stdout,
           "wasi:clocks/monotonic_clock@0.2.0-rc-2023-10-18": preview2::bindings::clocks::monotonic_clock,
           "wasi:clocks/timezone@0.2.0-rc-2023-10-18": preview2::bindings::clocks::timezone,
           "wasi:clocks/wall_clock@0.2.0-rc-2023-10-18": preview2::bindings::clocks::wall_clock,
           "wasi:filesystem/filesystem@0.2.0-rc-2023-10-18": preview2::bindings::filesystem::filesystem,
           "wasi:http/incoming-handler@0.2.0-rc-2023-10-18": wasmtime_wasi_http::bindings::http::incoming_handler,
           "wasi:http/outgoing-handler@0.2.0-rc-2023-10-18": wasmtime_wasi_http::bindings::http::incoming_handler,
           "wasi:http/types@0.2.0-rc-2023-10-18": wasmtime_wasi_http::bindings::http::types,
           "wasi:io/poll@0.2.0-rc-2023-10-18": preview2::bindings::io::poll,
           "wasi:io/streams@0.2.0-rc-2023-10-18": preview2::bindings::io::streams,
           "wasi:random/random@0.2.0-rc-2023-10-18": preview2::bindings::random::random,
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
