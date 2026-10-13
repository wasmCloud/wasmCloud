//! Remediation hints for guest traps whose cause is known to be in the guest's
//! toolchain rather than in the host.
//!
//! The trap itself is still logged where it surfaces; this only adds a line
//! telling the operator what to rebuild, once per workload deploy.

use std::sync::atomic::Ordering;

use wasmtime::Trap;

use crate::engine::ctx::SharedCtx;

/// Whether `err` is the trap Wasmtime 48 raises when a guest uses a waitable
/// synchronously while it is in a waitable set.
///
/// Async runtimes from `wit-bindgen` 0.58 and older with `inter-task-wakeup`
/// do exactly that when they cancel a stream, future or subtask (most often
/// `wasip3` 0.7.x with `http-compat`); 0.59 stopped doing it.
///
/// Matched on the typed trap rather than its text: a linked call flattens a
/// callee's trap into a plain message, and matching that text would blame
/// the caller.
pub(crate) fn is_waitable_sync_trap(err: &wasmtime::Error) -> bool {
    matches!(err.downcast_ref::<Trap>(), Some(Trap::WaitableSyncAndAsync))
}

/// Note a fault from driving a guest on the store whose data is `data`, and
/// log how to fix the guest if it is the waitable sync trap. Logs at most once
/// per workload deploy; does nothing for any other error.
///
/// Call it with the store's own error, before anything flattens it into a
/// message.
pub(crate) fn note_guest_fault(data: &SharedCtx, err: &wasmtime::Error) {
    if !is_waitable_sync_trap(err) {
        return;
    }
    // `active_ctx` names the store's root component. The trap may come from a
    // component linked into it, which the message allows for.
    let ctx = &data.active_ctx;
    if ctx.trap_hint_logged.swap(true, Ordering::Relaxed) {
        return;
    }
    tracing::warn!(
        component_id = %ctx.component_id,
        workload_id = %ctx.workload_id,
        trap = %Trap::WaitableSyncAndAsync,
        "guest trapped using a waitable synchronously while it was in a waitable set. This is \
         the known signature of wit-bindgen's async runtime at 0.58 or older with \
         `inter-task-wakeup` (usually wasip3 0.7.x with `http-compat`), which Wasmtime 48 and \
         later rejects. If this component, or a component linked into it, uses that runtime, \
         rebuild it on wasip3 0.9 or wit-bindgen 0.59 or newer"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_typed_trap_through_context() {
        let err = wasmtime::Error::new(Trap::WaitableSyncAndAsync)
            .context("error while executing at wasm backtrace")
            .context("trigger service driver exited");
        assert!(is_waitable_sync_trap(&err));
    }

    #[test]
    fn ignores_other_traps_and_flattened_text() {
        let other = wasmtime::Error::new(Trap::UnreachableCodeReached)
            .context("error while executing at wasm backtrace");
        assert!(!is_waitable_sync_trap(&other));
        // What a linked call leaves of a callee's trap: text, no trap. Blaming
        // the caller for it would be wrong, so it must not match.
        let trap = wasmtime::Error::new(Trap::WaitableSyncAndAsync);
        let flattened = wasmtime::format_err!("{trap:#}");
        assert!(!is_waitable_sync_trap(&flattened));
    }

    /// The guest pattern itself, in miniature: an async export starts a
    /// stream read that blocks, joins the readable end to a waitable set, then
    /// cancels the read synchronously. That is what an old `wit-bindgen`
    /// async runtime does on an ordinary request.
    const SYNC_CANCEL_IN_SET_WAT: &str = r#"
        (component
          (core module $libc (memory (export "memory") 1))
          (core instance $libc (instantiate $libc))
          (type $s (stream u8))
          (core func $stream-new (canon stream.new $s))
          (core func $stream-read (canon stream.read $s async (memory (core memory $libc "memory"))))
          (core func $stream-cancel-read (canon stream.cancel-read $s))
          (core func $waitable-set-new (canon waitable-set.new))
          (core func $waitable-join (canon waitable.join))
          (core module $m
            (import "" "stream.new" (func $stream-new (result i64)))
            (import "" "stream.read" (func $stream-read (param i32 i32 i32) (result i32)))
            (import "" "stream.cancel-read" (func $stream-cancel-read (param i32) (result i32)))
            (import "" "waitable-set.new" (func $waitable-set-new (result i32)))
            (import "" "waitable.join" (func $waitable-join (param i32 i32)))
            (func (export "run") (result i32)
              (local $rx i32)
              (local $set i32)
              (local.set $rx (i32.wrap_i64 (call $stream-new)))
              (drop (call $stream-read (local.get $rx) (i32.const 0) (i32.const 4)))
              (local.set $set (call $waitable-set-new))
              (call $waitable-join (local.get $rx) (local.get $set))
              (drop (call $stream-cancel-read (local.get $rx)))
              (i32.const 0))
            (func (export "callback") (param i32 i32 i32) (result i32) (i32.const 0)))
          (core instance $i (instantiate $m
            (with "" (instance
              (export "stream.new" (func $stream-new))
              (export "stream.read" (func $stream-read))
              (export "stream.cancel-read" (func $stream-cancel-read))
              (export "waitable-set.new" (func $waitable-set-new))
              (export "waitable.join" (func $waitable-join))))))
          (type $run (func async))
          (func (export "run") (type $run)
            (canon lift (core func $i "run") async (callback (core func $i "callback")))))
    "#;

    #[tokio::test]
    async fn a_sync_cancel_in_a_waitable_set_is_detected() {
        #[cfg(feature = "wasi-tls")]
        crate::init_crypto();

        let mut config = wasmtime::Config::new();
        config
            .wasm_component_model(true)
            .wasm_component_model_async(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        let wasm = wat::parse_str(SYNC_CANCEL_IN_SET_WAT).unwrap();
        let component = wasmtime::component::Component::new(&engine, wasm).unwrap();
        let linker = wasmtime::component::Linker::<SharedCtx>::new(&engine);
        let ctx = crate::engine::ctx::Ctx::builder("test-workload", "test-component").build();
        let mut store = wasmtime::Store::new(&engine, SharedCtx::new(ctx));
        let instance = linker
            .instantiate_async(&mut store, &component)
            .await
            .unwrap();
        let run = instance.get_func(&mut store, "run").unwrap();

        let err = store
            .run_concurrent(async move |accessor| run.call_concurrent(accessor, &[], &mut []).await)
            .await
            .and_then(|inner| inner)
            .expect_err("a sync cancel on a waitable in a set must trap");

        assert_eq!(
            err.downcast_ref::<Trap>(),
            Some(&Trap::WaitableSyncAndAsync),
            "{err:?}"
        );
        assert!(is_waitable_sync_trap(&err));

        // Logged once per workload deploy: the flag the hint sets is shared.
        assert!(
            !store
                .data()
                .active_ctx
                .trap_hint_logged
                .load(Ordering::Relaxed)
        );
        note_guest_fault(store.data(), &err);
        assert!(
            store
                .data()
                .active_ctx
                .trap_hint_logged
                .load(Ordering::Relaxed)
        );
    }
}
