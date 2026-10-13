//! Remediation hints for guest traps whose cause is known to be in the guest's
//! toolchain rather than in the host.
//!
//! The trap itself is still logged where it surfaces; this only adds a line
//! telling the operator what to rebuild.

use crate::engine::ctx::Ctx;

/// What Wasmtime 48 traps with when a guest cancels a stream, future or
/// subtask synchronously while that waitable is still in a waitable set.
///
/// Async runtimes from `wit-bindgen` 0.58 and older with `inter-task-wakeup`
/// do exactly that (most often `wasip3` 0.7.x with `http-compat`); 0.59
/// stopped doing it.
pub(crate) const WAITABLE_SYNC_CANCEL_TRAP: &str =
    "waitable cannot be used synchronously while added to a waitable set";

/// Whether `err`, or anything in its source chain, is the synchronous-cancel
/// trap that old `wit-bindgen` async runtimes hit.
pub(crate) fn is_wit_bindgen_cancel_trap(err: &wasmtime::Error) -> bool {
    err.chain()
        .any(|e| e.to_string().contains(WAITABLE_SYNC_CANCEL_TRAP))
}

/// Log how to fix the guest when `err` is the old-`wit-bindgen` cancel trap.
/// Does nothing for any other error.
pub(crate) fn warn_if_wit_bindgen_cancel_trap(err: &wasmtime::Error, ctx: &Ctx) {
    if is_wit_bindgen_cancel_trap(err) {
        tracing::warn!(
            component_id = %ctx.component_id,
            workload.id = %ctx.workload_id,
            "component was built with an async runtime from wit-bindgen 0.58 or older, which \
             this host's Wasmtime rejects; rebuild it on wasip3 0.9 / wit-bindgen 0.59 or newer \
             (0.62 is current)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_trap_through_its_context_chain() {
        let err = wasmtime::Error::new(wasmtime::Trap::WaitableSyncAndAsync)
            .context("error while executing at wasm backtrace")
            .context("trigger service driver exited");
        assert!(is_wit_bindgen_cancel_trap(&err));
    }

    #[test]
    fn ignores_unrelated_errors() {
        let trap = wasmtime::Error::new(wasmtime::Trap::UnreachableCodeReached)
            .context("error while executing at wasm backtrace");
        assert!(!is_wit_bindgen_cancel_trap(&trap));
        assert!(!is_wit_bindgen_cancel_trap(&wasmtime::format_err!(
            "pooled instance dropped the request"
        )));
    }
}
