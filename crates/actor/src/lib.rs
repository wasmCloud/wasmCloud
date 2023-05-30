#[cfg(all(not(feature = "module"), feature = "component"))]
wit_bindgen::generate!("interfaces");

#[cfg(feature = "module")]
mod module;

#[cfg(feature = "module")]
pub use module::*;

#[cfg(feature = "rand")]
pub use rand::{Rng, RngCore};

#[cfg(feature = "uuid")]
pub use uuid::Uuid;

pub struct HostRng;

#[cfg(all(not(feature = "module"), feature = "component"))]
impl HostRng {
    /// Generate a 32-bit random number
    #[inline]
    pub fn random32() -> u32 {
        random::get_random_u64() as _
    }

    /// Generate a v4-format guid in the form "nnnnnnnn-nnnn-nnnn-nnnn-nnnnnnnnnnnn"
    /// where n is a lowercase hex digit and all bits are random.
    #[cfg(feature = "uuid")]
    pub fn generate_guid() -> Uuid {
        let buf = uuid::Bytes::try_from(random::get_random_bytes(16))
            .expect("invalid amount of bytes generated");
        uuid::Builder::from_random_bytes(buf).into_uuid()
    }

    /// Generate a random integer within an inclusive range. ( min <= n <= max )
    #[cfg(feature = "rand")]
    pub fn random_in_range(min: u32, max: u32) -> u32 {
        HostRng.gen_range(min..=max)
    }
}

#[cfg(feature = "module")]
impl HostRng {
    /// Generate a 32-bit random number
    #[inline]
    pub fn random32() -> u32 {
        random::random32()
    }

    /// Generate a v4-format guid in the form "nnnnnnnn-nnnn-nnnn-nnnn-nnnnnnnnnnnn"
    /// where n is a lowercase hex digit and all bits are random.
    #[cfg(feature = "uuid")]
    pub fn generate_guid() -> Uuid {
        random::generate_guid()
    }

    /// Generate a random integer within an inclusive range. ( min <= n <= max )
    pub fn random_in_range(min: u32, max: u32) -> u32 {
        random::random_in_range(min, max)
    }
}

#[cfg(feature = "rand")]
impl RngCore for HostRng {
    #[inline]
    fn next_u32(&mut self) -> u32 {
        HostRng::random32()
    }

    #[inline]
    fn next_u64(&mut self) -> u64 {
        random::get_random_u64()
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        let n = dest.len();
        if usize::BITS <= u64::BITS || n <= u64::MAX as _ {
            dest.copy_from_slice(&random::get_random_bytes(n as _));
        } else {
            let (head, tail) = dest.split_at_mut(u64::MAX as _);
            head.copy_from_slice(&random::get_random_bytes(u64::MAX));
            // TODO: Optimize
            self.fill_bytes(tail);
        }
    }

    #[inline]
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

/// The standard logging macro.
///
/// This macro will generically log with the specified `Level` and `format!`
/// based argument list.
///
/// # Examples
///
/// ```no_run
/// use wasmcloud_actor::log;
/// use wasmcloud_actor::logging::Level;
///
/// # fn main() {
/// let data = (42, "Forty-two");
/// let private_data = "private";
///
/// log!(Level::Error, "Received errors: {}, {}", data.0, data.1);
/// log!(context: "app_events", Level::Warn, "App warning: {}, {}, {}",
///     data.0, data.1, private_data);
/// # }
/// ```
#[macro_export]
macro_rules! log {
    // log!(context: "my_context", Level::Info, "a {} event", "log");
    (context: $context:expr, $lvl:expr, $($arg:tt)+) => ({
        $crate::logging::log(
            $lvl,
            $context,
            &std::fmt::format(format_args!($($arg)*)),
        );
    });

    // log!(context: "my_context", Level::Info; "a {} event", "log");
    (context: $context:expr, $lvl:expr; $($arg:tt)+) => ({
        ($crate::log!(context: $context, $lvl, $($arg)+));
    });

    // log!(Level::Info, "a log event")
    ($lvl:expr, $($arg:tt)+) => ($crate::log!(context: "", $lvl, $($arg)+));
}

#[macro_export]
macro_rules! trace {
    // trace!(context: "context", "a {} event", "log")
    (context: $context:expr, $($arg:tt)+) => ($crate::log!(context: $context, $crate::logging::Level::Trace, $($arg)+));

    // trace!(context: "context"; "a {} event", "log")
    (context: $context:expr; $($arg:tt)+) => ($crate::log!(context: $context, $crate::logging::Level::Trace; $($arg)+));

    // trace!("a {} event", "log")
    ($($arg:tt)+) => ($crate::log!($crate::logging::Level::Trace, $($arg)+))
}

#[macro_export]
macro_rules! debug {
    // debug!(context: "context", "a {} event", "log")
    (context: $context:expr, $($arg:tt)+) => ($crate::log!(context: $context, $crate::logging::Level::Debug, $($arg)+));

    // debug!(context: "context"; "a {} event", "log")
    (context: $context:expr; $($arg:tt)+) => ($crate::log!(context: $context, $crate::logging::Level::Debug; $($arg)+));

    // debug!("a {} event", "log")
    ($($arg:tt)+) => ($crate::log!($crate::logging::Level::Debug, $($arg)+))
}

#[macro_export]
macro_rules! info {
    // info!(context: "context", "a {} event", "log")
    (context: $context:expr, $($arg:tt)+) => ($crate::log!(context: $context, $crate::logging::Level::Info, $($arg)+));

    // info!(context: "context"; "a {} event", "log")
    (context: $context:expr; $($arg:tt)+) => ($crate::log!(context: $context, $crate::logging::Level::Info; $($arg)+));

    // info!("a {} event", "log")
    ($($arg:tt)+) => ($crate::log!($crate::logging::Level::Info, $($arg)+))
}

#[macro_export]
macro_rules! warn {
    // warn!(context: "context", "a {} event", "log")
    (context: $context:expr, $($arg:tt)+) => ($crate::log!(context: $context, $crate::logging::Level::Warn, $($arg)+));

    // warn!(context: "context"; "a {} event", "log")
    (context: $context:expr; $($arg:tt)+) => ($crate::log!(context: $context, $crate::logging::Level::Warn; $($arg)+));

    // warn!("a {} event", "log")
    ($($arg:tt)+) => ($crate::log!($crate::logging::Level::Warn, $($arg)+))
}

#[macro_export]
macro_rules! error {
    // error!(context: "context", "a {} event", "log")
    (context: $context:expr, $($arg:tt)+) => ($crate::log!(context: $context, $crate::logging::Level::Error, $($arg)+));

    // error!(context: "context"; "a {} event", "log")
    (context: $context:expr; $($arg:tt)+) => ($crate::log!(context: $context, $crate::logging::Level::Error; $($arg)+));

    // error!("a {} event", "log")
    ($($arg:tt)+) => ($crate::log!($crate::logging::Level::Error, $($arg)+))
}

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
            logging::log(logging::Level::Trace, "context", "message");
            logging::log(logging::Level::Debug, "context", "message");
            logging::log(logging::Level::Info, "context", "message");
            logging::log(logging::Level::Warn, "context", "message");
            logging::log(logging::Level::Error, "context", "message");
            let _: Vec<u8> = random::get_random_bytes(4);
            let _: u64 = random::get_random_u64();
            let (_, _): (u64, u64) = random::insecure_random();
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
            host::call("binding", "namespace", "operation", Some(b"payload")).unwrap();
        }
    }
}
