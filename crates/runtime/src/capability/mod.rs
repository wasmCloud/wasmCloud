pub(crate) mod builtin;

pub use builtin::{Bus, IncomingHttp, Logging, Messaging};

#[allow(missing_docs)]
mod bindgen {
    use wasmtime_wasi::preview2;

    wasmtime::component::bindgen!({
        world: "interfaces",
        async: true,
        with: {
           "wasi:cli/environment": preview2::wasi::cli::environment,
           "wasi:cli/exit": preview2::wasi::cli::exit,
           "wasi:cli/preopens": preview2::wasi::cli::preopens,
           "wasi:cli/stderr": preview2::wasi::cli::stderr,
           "wasi:cli/stdin": preview2::wasi::cli::stdin,
           "wasi:cli/stdout": preview2::wasi::cli::stdout,
           "wasi:clocks/monotonic_clock": preview2::wasi::clocks::monotonic_clock,
           "wasi:clocks/timezone": preview2::wasi::clocks::timezone,
           "wasi:clocks/wall_clock": preview2::wasi::clocks::wall_clock,
           "wasi:filesystem/filesystem": preview2::wasi::filesystem::filesystem,
           "wasi:io/streams": preview2::wasi::io::streams,
           "wasi:poll/poll": preview2::wasi::poll::poll,
           "wasi:random/random": preview2::wasi::random::random,
        },
    });
}

pub use bindgen::wasi::logging;
pub use bindgen::wasmcloud::blobstore;
pub use bindgen::wasmcloud::bus;
pub use bindgen::wasmcloud::messaging;
pub use bindgen::Interfaces;

fn format_opt<T>(opt: &Option<T>) -> &'static str {
    if opt.is_some() {
        "set"
    } else {
        "unset"
    }
}
