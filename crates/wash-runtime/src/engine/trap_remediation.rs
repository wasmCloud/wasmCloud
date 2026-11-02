//! Remediation hints for guest faults whose fix is known: a guest toolchain to
//! rebuild, or a host knob to raise.
//!
//! The fault itself is still logged where it surfaces; this only adds a line
//! telling the operator what to change, each hint once per workload deploy.

use std::sync::atomic::{AtomicU8, Ordering};

use wasmtime::Trap;

use crate::engine::ctx::SharedCtx;

/// A remediation hint [`note_guest_fault`] can log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum TrapHint {
    WaitableSync = 1 << 0,
    HostcallFuel = 1 << 1,
}

/// The hints a workload deploy has already logged. One set is shared by every
/// component of a deploy, so a redeploy starts with a fresh one.
#[derive(Debug, Default)]
pub(crate) struct TrapHints(AtomicU8);

impl TrapHints {
    /// Records `hint` as logged; true only the first time.
    pub(crate) fn first(&self, hint: TrapHint) -> bool {
        self.0.fetch_or(hint as u8, Ordering::Relaxed) & hint as u8 == 0
    }

    #[cfg(test)]
    pub(crate) fn logged(&self, hint: TrapHint) -> bool {
        self.0.load(Ordering::Relaxed) & hint as u8 != 0
    }
}

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

/// What wasmtime says when a guest copies more to the host in one call than
/// the store's hostcall fuel allows.
const HOSTCALL_FUEL_EXHAUSTED: &str = "fuel allocated for hostcalls has been exhausted";

/// Whether `err` is a guest running out of hostcall fuel.
///
/// Matched on text: wasmtime's `HostcallFuelExhausted` is private. Unlike
/// [`is_waitable_sync_trap`], a flattened callee error matching is correct
/// here, because the fix is the host-wide knob, not a component.
pub(crate) fn is_hostcall_fuel_exhausted(err: &wasmtime::Error) -> bool {
    err.chain()
        .any(|cause| cause.to_string().contains(HOSTCALL_FUEL_EXHAUSTED))
}

/// Note a fault from driving a guest on the store whose data is `data`, and
/// log how to fix it if the fix is known. Logs each hint at most once per
/// workload deploy; does nothing for any other error.
///
/// Call it with the store's own error, before anything flattens it into a
/// message.
pub(crate) fn note_guest_fault(data: &SharedCtx, err: &wasmtime::Error) {
    let hint = if is_waitable_sync_trap(err) {
        TrapHint::WaitableSync
    } else if is_hostcall_fuel_exhausted(err) {
        TrapHint::HostcallFuel
    } else {
        return;
    };
    // `active_ctx` names the store's root component. The fault may come from
    // a component linked into it, which the messages allow for.
    let ctx = &data.active_ctx;
    if !ctx.trap_hints.first(hint) {
        return;
    }
    match hint {
        TrapHint::WaitableSync => tracing::warn!(
            component_id = %ctx.component_id,
            workload_id = %ctx.workload_id,
            trap = %Trap::WaitableSyncAndAsync,
            "guest trapped using a waitable synchronously while it was in a waitable set. This \
             is the known signature of wit-bindgen's async runtime at 0.58 or older with \
             `inter-task-wakeup` (usually wasip3 0.7.x with `http-compat`), which Wasmtime 48 \
             and later rejects. If this component, or a component linked into it, uses that \
             runtime, rebuild it on wasip3 0.9 or wit-bindgen 0.59 or newer"
        ),
        TrapHint::HostcallFuel => {
            let hostcall_fuel = match data.store_settings.hostcall_fuel {
                Some(fuel) => crate::engine::host_memory::render_bytes(
                    u64::try_from(fuel.get()).unwrap_or(u64::MAX),
                ),
                None => "128MiB (wasmtime's default)".to_string(),
            };
            tracing::warn!(
                component_id = %ctx.component_id,
                workload_id = %ctx.workload_id,
                %hostcall_fuel,
                "a guest copied more data to the host in one call, or in one future or stream \
                 value, than this host's hostcall fuel allows. If this component, or a \
                 component linked into it, legitimately sends that much at once, raise \
                 `--hostcall-fuel` (`WASH_HOSTCALL_FUEL`, chart `resources.hostcallFuel`); the \
                 host allocates those bytes outside the guest memory budget"
            )
        }
    }
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

        // Logged once per workload deploy: the set the hint marks is shared.
        let hints = &store.data().active_ctx.trap_hints;
        assert!(!hints.logged(TrapHint::WaitableSync));
        note_guest_fault(store.data(), &err);
        assert!(hints.logged(TrapHint::WaitableSync));
        assert!(!hints.logged(TrapHint::HostcallFuel));
    }

    /// A guest handing the host a 32-byte string through an import.
    const STRING_TO_HOST_WAT: &str = r#"
        (component
          (import "take" (func $take (param "s" string)))
          (core module $libc
            (memory (export "memory") 1)
            (data (i32.const 0) "0123456789abcdef0123456789abcdef"))
          (core instance $libc (instantiate $libc))
          (core func $take-lower (canon lower (func $take) (memory (core memory $libc "memory"))))
          (core module $m
            (import "" "take" (func $take (param i32 i32)))
            (func (export "run") (call $take (i32.const 0) (i32.const 32))))
          (core instance $i (instantiate $m
            (with "" (instance (export "take" (func $take-lower))))))
          (func (export "run") (canon lift (core func $i "run"))))
    "#;

    /// Pins wasmtime's wording: the hint matches on text, so an upgrade that
    /// rewords the error fails here rather than silently losing the hint.
    #[tokio::test]
    async fn hostcall_fuel_exhaustion_is_detected() {
        #[cfg(feature = "wasi-tls")]
        crate::init_crypto();

        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        let wasm = wat::parse_str(STRING_TO_HOST_WAT).unwrap();
        let component = wasmtime::component::Component::new(&engine, wasm).unwrap();
        let mut linker = wasmtime::component::Linker::<SharedCtx>::new(&engine);
        linker
            .root()
            .func_wrap("take", |_, (_s,): (String,)| Ok(()))
            .unwrap();
        let ctx = crate::engine::ctx::Ctx::builder("test-workload", "test-component").build();
        let mut store = wasmtime::Store::new(&engine, SharedCtx::new(ctx));
        store.set_hostcall_fuel(16);
        let instance = linker
            .instantiate_async(&mut store, &component)
            .await
            .unwrap();
        let run = instance
            .get_typed_func::<(), ()>(&mut store, "run")
            .unwrap();

        let err = run
            .call_async(&mut store, ())
            .await
            .expect_err("32 bytes on 16 bytes of hostcall fuel must trap");

        assert!(is_hostcall_fuel_exhausted(&err), "{err:?}");
        assert!(!is_waitable_sync_trap(&err));
        // A linked call's flattened copy still names the same host knob.
        assert!(is_hostcall_fuel_exhausted(&wasmtime::format_err!(
            "{err:#}"
        )));

        let hints = &store.data().active_ctx.trap_hints;
        note_guest_fault(store.data(), &err);
        assert!(hints.logged(TrapHint::HostcallFuel));
        assert!(!hints.logged(TrapHint::WaitableSync));
        assert!(
            !hints.first(TrapHint::HostcallFuel),
            "logged once per deploy"
        );
    }

    /// A guest writing a 32-byte string into a future the host reads.
    const FUTURE_TO_HOST_WAT: &str = r#"
        (component
          (type $f (future string))
          (import "take" (func $take (param "f" $f)))
          (core module $libc
            (memory (export "memory") 1)
            (data (i32.const 0) "0123456789abcdef0123456789abcdef")
            (data (i32.const 64) "\00\00\00\00\20\00\00\00"))
          (core instance $libc (instantiate $libc))
          (core func $new (canon future.new $f))
          (core func $write
            (canon future.write $f async (memory (core memory $libc "memory"))))
          (core func $take-lower (canon lower (func $take)))
          (core module $m
            (import "" "new" (func $new (result i64)))
            (import "" "write" (func $write (param i32 i32) (result i32)))
            (import "" "take" (func $take (param i32)))
            (func (export "run") (local $ends i64)
              (local.set $ends (call $new))
              (call $take (i32.wrap_i64 (local.get $ends)))
              (drop (call $write
                (i32.wrap_i64 (i64.shr_u (local.get $ends) (i64.const 32)))
                (i32.const 64)))))
          (core instance $i (instantiate $m
            (with "" (instance
              (export "new" (func $new))
              (export "write" (func $write))
              (export "take" (func $take-lower))))))
          (func (export "run") (canon lift (core func $i "run"))))
    "#;

    /// A value the host lifts from a future draws on the same fuel, and its
    /// exhaustion comes out of the store's event loop where the call sites
    /// that note guest faults see it.
    #[tokio::test]
    async fn hostcall_fuel_exhaustion_on_a_future_is_detected() {
        #[cfg(feature = "wasi-tls")]
        crate::init_crypto();

        let mut config = wasmtime::Config::new();
        config.wasm_component_model_async(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        let wasm = wat::parse_str(FUTURE_TO_HOST_WAT).unwrap();
        let component = wasmtime::component::Component::new(&engine, wasm).unwrap();
        let received = std::sync::Arc::new(std::sync::Mutex::new(None));
        let mut linker = wasmtime::component::Linker::<SharedCtx>::new(&engine);
        let slot = std::sync::Arc::clone(&received);
        linker
            .root()
            .func_wrap(
                "take",
                move |mut store, (future,): (wasmtime::component::FutureReader<String>,)| {
                    let (sink, rx, _) = crate::engine::store::stream_pump::future_channel();
                    future.pipe(&mut store, sink)?;
                    *slot.lock().unwrap() = Some(rx);
                    Ok(())
                },
            )
            .unwrap();
        let ctx = crate::engine::ctx::Ctx::builder("test-workload", "test-component").build();
        let mut store = wasmtime::Store::new(&engine, SharedCtx::new(ctx));
        store.set_hostcall_fuel(16);
        let instance = linker
            .instantiate_async(&mut store, &component)
            .await
            .unwrap();
        let run = instance
            .get_typed_func::<(), ()>(&mut store, "run")
            .unwrap();

        let err = store
            .run_concurrent(async |accessor| {
                run.call_concurrent(accessor, ()).await?;
                let rx = received.lock().unwrap().take().expect("take was called");
                rx.await.map_err(wasmtime::Error::from)
            })
            .await
            .and_then(|inner| inner)
            .expect_err("a 32-byte string on 16 bytes of hostcall fuel must trap");

        assert!(is_hostcall_fuel_exhausted(&err), "{err:?}");
        note_guest_fault(store.data(), &err);
        assert!(
            store
                .data()
                .active_ctx
                .trap_hints
                .logged(TrapHint::HostcallFuel)
        );
    }
}
