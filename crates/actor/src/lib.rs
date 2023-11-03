#[cfg(all(not(feature = "module"), feature = "component",))]
wit_bindgen::generate!("interfaces");

#[cfg(feature = "module")]
mod compat;

#[cfg(feature = "module")]
pub use compat::*;

#[cfg(feature = "module")]
pub use wasmcloud_actor_macros::*;

mod wrappers;
pub use wrappers::*;

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
                vec![
                    wasmcloud::bus::lattice::TargetInterface::wasi_blobstore_blobstore(),
                    wasmcloud::bus::lattice::TargetInterface::wasi_keyvalue_readwrite(),
                    wasmcloud::bus::lattice::TargetInterface::wasi_logging_logging(),
                    wasmcloud::bus::lattice::TargetInterface::wasmcloud_messaging_consumer(),
                ],
            );
        }
    }
}
