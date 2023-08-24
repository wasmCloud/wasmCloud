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
pub use wasmcloud_compat::{HttpRequest, HttpResponse};

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

            // TODO: Implement `call` in module world
            #[cfg(not(feature = "module"))]
            let (_, _, _): (
                wasmcloud::bus::host::FutureResult,
                wasi::io::streams::InputStream,
                wasi::io::streams::OutputStream,
            ) = wasmcloud::bus::host::call(None, "mycompany:mypackage/interface.operation")
                .unwrap();

            let _: Result<Vec<u8>, String> = wasmcloud::bus::host::call_sync(
                Some(&wasmcloud::bus::lattice::TargetEntity::Link(Some(
                    "test".into(),
                ))),
                "mycompany:mypackage/interface.operation",
                &[],
            );

            wasmcloud::bus::lattice::set_target(
                None,
                &[
                    wasmcloud::bus::lattice::target_wasi_blobstore_blobstore(),
                    wasmcloud::bus::lattice::target_wasi_keyvalue_readwrite(),
                    wasmcloud::bus::lattice::target_wasi_logging_logging(),
                    wasmcloud::bus::lattice::target_wasmcloud_messaging_consumer(),
                ],
            );
        }
    }
}
