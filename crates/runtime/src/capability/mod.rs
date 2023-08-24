pub(crate) mod builtin;

/// Provider implementations
pub mod provider;

pub use builtin::{
    ActorIdentifier, Blobstore, Bus, IncomingHttp, KeyValueAtomic, KeyValueReadWrite, Logging,
    Messaging, TargetEntity, TargetInterface,
};

#[allow(clippy::doc_markdown)]
#[allow(missing_docs)]
mod bindgen {
    use wasmtime_wasi::preview2;

    wasmtime::component::bindgen!({
        world: "interfaces",
        async: true,
        with: {
           "wasi:cli/environment": preview2::bindings::cli::environment,
           "wasi:cli/exit": preview2::bindings::cli::exit,
           "wasi:cli/preopens": preview2::bindings::cli::preopens,
           "wasi:cli/stderr": preview2::bindings::cli::stderr,
           "wasi:cli/stdin": preview2::bindings::cli::stdin,
           "wasi:cli/stdout": preview2::bindings::cli::stdout,
           "wasi:clocks/monotonic_clock": preview2::bindings::clocks::monotonic_clock,
           "wasi:clocks/timezone": preview2::bindings::clocks::timezone,
           "wasi:clocks/wall_clock": preview2::bindings::clocks::wall_clock,
           "wasi:filesystem/filesystem": preview2::bindings::filesystem::filesystem,
           "wasi:io/streams": preview2::bindings::io::streams,
           "wasi:poll/poll": preview2::bindings::poll::poll,
           "wasi:random/random": preview2::bindings::random::random,
        },
    });
}

pub use bindgen::wasi::{blobstore, http, keyvalue, logging};
pub use bindgen::wasmcloud::{bus, messaging};
pub use bindgen::Interfaces;

fn format_opt<T>(opt: &Option<T>) -> &'static str {
    if opt.is_some() {
        "set"
    } else {
        "unset"
    }
}
