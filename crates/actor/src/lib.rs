#![allow(clippy::missing_safety_doc)]

wit_bindgen::generate!("interfaces");

mod wrappers;
pub use wrappers::*;

#[cfg(test)]
mod test {
    use super::*;

    #[allow(dead_code)]
    struct Actor;

    #[allow(dead_code)]
    impl Actor {
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

            wasmcloud::bus::lattice::set_link_name(
                "default",
                vec![
                    wasmcloud::bus::lattice::CallTargetInterface::new(
                        "wasi",
                        "blobstore",
                        "blobstore",
                    ),
                    wasmcloud::bus::lattice::CallTargetInterface::new(
                        "wasi", "keyvalue", "eventual",
                    ),
                    wasmcloud::bus::lattice::CallTargetInterface::new("wasi", "logging", "logging"),
                    wasmcloud::bus::lattice::CallTargetInterface::new(
                        "wasmcloud",
                        "messaging",
                        "consumer",
                    ),
                ],
            );
        }
    }
}
