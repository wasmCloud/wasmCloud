/// The standard logging macro.
///
/// This macro will generically log with the specified `Level` and `format!`
/// based argument list.
///
/// # Examples
///
/// ```no_run
/// use wasmcloud_actor::log;
/// use wasmcloud_actor::wasi::logging::logging::Level;
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
        $crate::wasi::logging::logging::log(
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
    (context: $context:expr, $($arg:tt)+) => ($crate::log!(context: $context, $crate::wasi::logging::logging::Level::Trace, $($arg)+));

    // trace!(context: "context"; "a {} event", "log")
    (context: $context:expr; $($arg:tt)+) => ($crate::log!(context: $context, $crate::wasi::logging::logging::Level::Trace; $($arg)+));

    // trace!("a {} event", "log")
    ($($arg:tt)+) => ($crate::log!($crate::wasi::logging::logging::Level::Trace, $($arg)+))
}

#[macro_export]
macro_rules! debug {
    // debug!(context: "context", "a {} event", "log")
    (context: $context:expr, $($arg:tt)+) => ($crate::log!(context: $context, $crate::wasi::logging::logging::Level::Debug, $($arg)+));

    // debug!(context: "context"; "a {} event", "log")
    (context: $context:expr; $($arg:tt)+) => ($crate::log!(context: $context, $crate::wasi::logging::logging::Level::Debug; $($arg)+));

    // debug!("a {} event", "log")
    ($($arg:tt)+) => ($crate::log!($crate::wasi::logging::logging::Level::Debug, $($arg)+))
}

#[macro_export]
macro_rules! info {
    // info!(context: "context", "a {} event", "log")
    (context: $context:expr, $($arg:tt)+) => ($crate::log!(context: $context, $crate::wasi::logging::logging::Level::Info, $($arg)+));

    // info!(context: "context"; "a {} event", "log")
    (context: $context:expr; $($arg:tt)+) => ($crate::log!(context: $context, $crate::wasi::logging::logging::Level::Info; $($arg)+));

    // info!("a {} event", "log")
    ($($arg:tt)+) => ($crate::log!($crate::wasi::logging::logging::Level::Info, $($arg)+))
}

#[macro_export]
macro_rules! warn {
    // warn!(context: "context", "a {} event", "log")
    (context: $context:expr, $($arg:tt)+) => ($crate::log!(context: $context, $crate::wasi::logging::logging::Level::Warn, $($arg)+));

    // warn!(context: "context"; "a {} event", "log")
    (context: $context:expr; $($arg:tt)+) => ($crate::log!(context: $context, $crate::wasi::logging::logging::Level::Warn; $($arg)+));

    // warn!("a {} event", "log")
    ($($arg:tt)+) => ($crate::log!($crate::wasi::logging::logging::Level::Warn, $($arg)+))
}

#[macro_export]
macro_rules! error {
    // error!(context: "context", "a {} event", "log")
    (context: $context:expr, $($arg:tt)+) => ($crate::log!(context: $context, $crate::wasi::logging::logging::Level::Error, $($arg)+));

    // error!(context: "context"; "a {} event", "log")
    (context: $context:expr; $($arg:tt)+) => ($crate::log!(context: $context, $crate::wasi::logging::logging::Level::Error; $($arg)+));

    // error!("a {} event", "log")
    ($($arg:tt)+) => ($crate::log!($crate::wasi::logging::logging::Level::Error, $($arg)+))
}

#[macro_export]
macro_rules! critical {
    // critical!(context: "context", "a {} event", "log")
    (context: $context:expr, $($arg:tt)+) => ($crate::log!(context: $context, $crate::wasi::logging::logging::Level::Critical, $($arg)+));

    // critical!(context: "context"; "a {} event", "log")
    (context: $context:expr; $($arg:tt)+) => ($crate::log!(context: $context, $crate::wasi::logging::logging::Level::Critical; $($arg)+));

    // critical!("a {} event", "log")
    ($($arg:tt)+) => ($crate::log!($crate::wasi::logging::logging::Level::Critical, $($arg)+))
}
