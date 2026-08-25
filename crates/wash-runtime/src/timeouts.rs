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

/// Parse `var` as whole milliseconds, falling back to `default_millis` if it is
/// unset. Unparseable values fall back with a warning, as in [`env_secs`].
fn env_millis(var: &str, default_millis: u64) -> Duration {
    let millis = match std::env::var(var) {
        Ok(v) => match v.parse::<u64>() {
            Ok(millis) => millis,
            Err(_) => {
                tracing::warn!(
                    var,
                    value = %v,
                    default_millis,
                    "ignoring unparseable timeout override (want whole milliseconds)"
                );
                default_millis
            }
        },
        Err(_) => default_millis,
    };
    Duration::from_millis(millis)
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
    /// Max wall-clock for a trigger service to acknowledge a delivered message.
    messaging_deliver = ("WASH_MESSAGING_DELIVER_TIMEOUT_SECS", 600);
    /// How long an abandoned call may keep running before its store acts on the
    /// abandonment (see [`crate::engine::abandon`]). This is what makes
    /// abandonment safe to signal on every disconnect: a healthy guest finishes
    /// the call well inside the grace, while a wedged one is still running —
    /// and still registered — when it runs out.
    abandoned_call_grace = ("WASH_ABANDONED_CALL_GRACE_SECS", 10);
    /// How long a `WarnThenTrap` store carries an abandoned call before
    /// trapping anyway. The long runway lets a yielding call finish harmlessly;
    /// a call still running at the end of it has the store wedged for every
    /// tenant, and the supervised restart is what restores service.
    abandoned_call_escalation = ("WASH_ABANDONED_CALL_ESCALATION_SECS", 60);
    /// The per-plugin stop budget. A host component plugin's `stop()` waits
    /// this long for its supervisor to exit before aborting it; `Host::stop`
    /// caps every plugin's `stop()` at this budget plus a one-second grace so
    /// that abort path always gets to run before the host gives up.
    plugin_stop = ("WASH_PLUGIN_STOP_TIMEOUT_SECS", 5);
    /// Uptime a host component plugin's driver must reach before a later fault
    /// resets its restart budget.
    #[cfg(feature = "host-component-plugins")]
    plugin_healthy_uptime = ("WASH_PLUGIN_HEALTHY_UPTIME_SECS", 60);
    /// Max wall-clock for one `wasmcloud:host/workload-lifecycle` call into a
    /// host component plugin. Bounds how long a plugin's bind handler can hold
    /// up a workload deploy (bind fails on expiry) and how long its unbind
    /// handler can hold up a workload stop (unbind is abandoned on expiry).
    #[cfg(feature = "host-component-plugins")]
    plugin_lifecycle_call = ("WASH_PLUGIN_LIFECYCLE_CALL_TIMEOUT_SECS", 30);
    /// Upper bound on a host component plugin's pre-restart backoff.
    #[cfg(feature = "host-component-plugins")]
    plugin_restart_backoff_max = ("WASH_PLUGIN_RESTART_BACKOFF_MAX_SECS", 5);
    /// Max wall-clock for one capability call into a host component plugin.
    #[cfg(feature = "host-component-plugins")]
    plugin_capability_call = ("WASH_PLUGIN_CAPABILITY_CALL_TIMEOUT_SECS", 600);
}

/// The gap between epoch fires past which a store's guest counts as having
/// paused rather than having been held up (see [`crate::engine::abandon`]).
///
/// Raise it on a host loaded enough that a pinned guest's fires land further
/// apart than the default, where the gaps read as pauses, execution never
/// accumulates and a wedged store is never trapped.
pub(crate) fn abandoned_call_pause_threshold() -> Duration {
    static VALUE: LazyLock<Duration> =
        LazyLock::new(|| env_millis("WASH_ABANDONED_CALL_PAUSE_THRESHOLD_MS", 3_000));
    *VALUE
}
