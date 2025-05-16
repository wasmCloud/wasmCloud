mod bindings {
    wit_bindgen::generate!({
        world: "interfaces",
        with: {
            "wasi:blobstore/blobstore@0.2.0-draft": generate,
            "wasi:blobstore/container@0.2.0-draft": generate,
            "wasi:blobstore/types@0.2.0-draft": generate,
            "wasi:clocks/monotonic-clock@0.2.2": ::wasi::clocks::monotonic_clock,
            "wasi:config/store@0.2.0-draft": generate,
            "wasi:http/outgoing-handler@0.2.2": ::wasi::http::outgoing_handler,
            "wasi:http/types@0.2.2": ::wasi::http::types,
            "wasi:io/error@0.2.2": ::wasi::io::error,
            "wasi:io/poll@0.2.2": ::wasi::io::poll,
            "wasi:io/streams@0.2.2": ::wasi::io::streams,
            "wasi:keyvalue/atomics@0.2.0-draft": generate,
            "wasi:keyvalue/batch@0.2.0-draft": generate,
            "wasi:keyvalue/store@0.2.0-draft": generate,
            "wasi:keyvalue/watcher@0.2.0-draft": generate,
            "wasi:logging/logging@0.1.0-draft": generate,
            "wasi:random/random@0.2.2": ::wasi::random::random,
            "wasmcloud:bus/lattice@2.0.0": generate,
            "wasmcloud:messaging/consumer@0.2.0": generate,
            "wasmcloud:messaging/producer@0.3.0": generate,
            "wasmcloud:messaging/request-reply@0.3.0": generate,
            "wasmcloud:messaging/types@0.2.0": generate,
            "wasmcloud:messaging/types@0.3.0": generate,
            "wasmcloud:cron/scheduler@0.1.0" : generate,
        }
    });
}

pub mod wasi {
    pub use super::bindings::wasi::*;
    pub use ::wasi::*;
}

pub mod wasmcloud {
    pub use super::bindings::wasmcloud::messaging0_2_0 as messaging;
    pub use super::bindings::wasmcloud::*;
}

mod wrappers;
pub use wrappers::*;

pub trait From<T> {
    fn from(v: T) -> Self;
}

pub trait Into<T> {
    fn into(v: Self) -> T;
}

impl<T, U: From<T>> Into<U> for T {
    fn into(v: Self) -> U {
        U::from(v)
    }
}

impl<T, U: From<T>> TryFrom<T> for U {
    type Error = core::convert::Infallible;

    fn try_from(v: T) -> Result<Self, Self::Error> {
        Ok(U::from(v))
    }
}

pub trait TryFrom<T>
where
    Self: Sized,
{
    type Error;

    fn try_from(v: T) -> Result<Self, Self::Error>;
}

pub trait TryInto<T> {
    type Error;

    fn try_into(v: Self) -> Result<T, Self::Error>;
}

impl<T, U: TryFrom<T>> TryInto<U> for T {
    type Error = U::Error;

    fn try_into(v: Self) -> Result<U, Self::Error> {
        U::try_from(v)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[allow(dead_code)]
    struct Component;

    #[allow(dead_code)]
    impl Component {
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

            let _ = wasi::config::store::get("foo");
            let _ = wasi::config::store::get_all();

            let _ = wasmcloud::bus::lattice::set_link_name(
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
