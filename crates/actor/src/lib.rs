#[cfg(all(
    not(feature = "module"),
    feature = "component",
    not(feature = "compat")
))]
wit_bindgen::generate!("interfaces");

#[cfg(any(feature = "module", all(feature = "component", feature = "compat")))]
mod compat;

#[cfg(any(feature = "module", all(feature = "component", feature = "compat")))]
pub use compat::*;

#[cfg(feature = "module")]
pub use wasmcloud_actor_derive::*;

// TODO: Remove once `wasi-http` is integrated
mod http;
pub use http::{Request as HttpRequest, Response as HttpResponse};

mod io;
mod logging;
mod random;

pub use io::*;
pub use logging::*;
pub use random::*;

#[cfg(test)]
mod test {
    #[cfg(any(feature = "module", feature = "component"))]
    use super::*;

    #[allow(dead_code)]
    struct Actor;

    #[allow(dead_code)]
    impl Actor {
        #[cfg(any(feature = "module", feature = "component"))]
        fn use_host_exports() {
            wasi::logging::logging::log(wasi::logging::logging::Level::Trace, "context", "message");
            wasi::logging::logging::log(wasi::logging::logging::Level::Debug, "context", "message");
            wasi::logging::logging::log(wasi::logging::logging::Level::Info, "context", "message");
            wasi::logging::logging::log(wasi::logging::logging::Level::Warn, "context", "message");
            wasi::logging::logging::log(wasi::logging::logging::Level::Error, "context", "message");
            wasi::logging::logging::log(
                wasi::logging::logging::Level::Critical,
                "context",
                "message",
            );
            let _: Vec<u8> = wasi::random::random::get_random_bytes(4);
            let _: u64 = wasi::random::random::get_random_u64();
            // TODO: Add support for HTTP
            //outgoing_http::handle(
            //    types::new_outgoing_request(
            //        types::MethodParam::Get,
            //        "path",
            //        "query",
            //        Some(types::SchemeParam::Https),
            //        "authority",
            //        types::new_fields(&[("myheader", "myvalue")]),
            //    ),
            //    Some(types::RequestOptions {
            //        connect_timeout_ms: Some(42),
            //        first_byte_timeout_ms: Some(42),
            //        between_bytes_timeout_ms: Some(42),
            //    }),
            //);
            // TODO
            #[cfg(not(feature = "module"))]
            let (_, _): (
                wasi::io::streams::InputStream,
                wasi::io::streams::OutputStream,
            ) = wasmcloud::bus::host::call("mycompany:mypackage/interface.operation").unwrap();
        }
    }
}
