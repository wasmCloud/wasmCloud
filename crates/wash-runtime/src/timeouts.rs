//! Runtime-tunable timeouts for the cross-store call and trigger-service paths.
//!
//! Every timeout is declared in the single [`declare_timeouts!`] invocation
//! below and read through a generated accessor. Add a new timeout by adding one
//! line there.

use std::sync::LazyLock;
use std::time::Duration;

/// Parse `var` as whole seconds, falling back to `default_secs` if it is unset.
/// A set-but-unparseable value also falls back, with a warning — silently
/// ignoring it would leave an operator's typo undetected.
fn env_secs(var: &str, default_secs: u64) -> Duration {
    let secs = match std::env::var(var) {
        Ok(v) => match v.parse::<u64>() {
            Ok(secs) => secs,
            Err(_) => {
                tracing::warn!(
                    var,
                    value = %v,
                    default_secs,
                    "ignoring unparseable timeout override (want whole seconds)"
                );
                default_secs
            }
        },
        Err(_) => default_secs,
    };
    Duration::from_secs(secs)
}

/// Declare the runtime-tunable timeouts, one `name = ("ENV_VAR", default_secs)`
/// entry per line (separated by `;`). Each entry generates a
/// `pub(crate) fn name() -> Duration` accessor: on first call it reads the named
/// env var as a whole number of seconds (via [`env_secs`]), falling back to the
/// compile-time default if the var is unset or unparseable, and caches the
/// result for the process lifetime with a [`LazyLock`] — so an override must be
/// set before the runtime starts. Per-entry attributes (doc comments,
/// `#[cfg(...)]`) are forwarded to the generated fn.
macro_rules! declare_timeouts {
    ($(
        $(#[$attr:meta])*
        $name:ident = ($var:literal, $default:literal)
    );* $(;)?) => {
        $(
            $(#[$attr])*
            pub(crate) fn $name() -> Duration {
                static VALUE: LazyLock<Duration> = LazyLock::new(|| env_secs($var, $default));
                *VALUE
            }
        )*
    };
}

declare_timeouts! {
    /// Max wall-clock for a single ephemeral cross-store linked call.
    ephemeral_call = ("WASH_EPHEMERAL_CALL_TIMEOUT_SECS", 600);
    /// Max wall-clock to drain an ephemeral call's result streams before its
    /// throwaway store is torn down.
    stream_drain = ("WASH_STREAM_DRAIN_TIMEOUT_SECS", 600);
    /// Max wall-clock for a single shared-store dynamic linked call.
    shared_store_call = ("WASH_SHARED_STORE_CALL_TIMEOUT_SECS", 30);
    /// Max wall-clock for a trigger service to produce an HTTP response.
    http_response = ("WASH_HTTP_RESPONSE_TIMEOUT_SECS", 600);
    /// How long `stop()` waits for a host component plugin's supervisor to exit
    /// before aborting it.
    #[cfg(feature = "host-component-plugins")]
    plugin_stop = ("WASH_PLUGIN_STOP_TIMEOUT_SECS", 5);
    /// Uptime a host component plugin's driver must reach before a later fault
    /// resets its restart budget.
    #[cfg(feature = "host-component-plugins")]
    plugin_healthy_uptime = ("WASH_PLUGIN_HEALTHY_UPTIME_SECS", 60);
    /// Upper bound on a host component plugin's pre-restart backoff.
    #[cfg(feature = "host-component-plugins")]
    plugin_restart_backoff_max = ("WASH_PLUGIN_RESTART_BACKOFF_MAX_SECS", 5);
}
