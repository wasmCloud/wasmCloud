//! Setting environment variables in a test, without racing the other tests.
//!
//! A test that reads a variable by its *real* name cannot use the trick the
//! rest of this crate's environment tests use — a UUID-suffixed key nobody else
//! touches. It has to set `OTEL_EXPORTER_OTLP_ENDPOINT` itself, and `setenv`
//! and `getenv` are not thread-safe against each other, while cargo runs a
//! crate's tests on threads of one process.
//!
//! [`with_vars`] is the whole answer: one process-wide lock, so two such tests
//! never overlap, and a guard that puts back what was there — including when
//! the body panics, so one failing test does not leave the variable set for
//! whatever runs next.

use std::cell::Cell;
use std::ffi::{OsStr, OsString};
use std::sync::{Mutex, MutexGuard};

/// Held for the duration of a [`with_vars`] body, so no two of them overlap.
static SERIALIZED: Mutex<()> = Mutex::new(());

thread_local! {
    /// How many [`with_vars`] bodies this thread is inside.
    ///
    /// Only the outermost takes [`SERIALIZED`]: a `std::sync::Mutex` is not
    /// reentrant, so a nested call would deadlock against a lock its own thread
    /// already holds. Nesting is worth supporting rather than forbidding —
    /// setting a variable inside a scope that set another is the obvious way to
    /// write these tests, and a deadlock is a miserable way to learn otherwise.
    /// While the outer body holds the lock no other thread can interleave, so
    /// the inner ones need no lock of their own.
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// Run `body` with `vars` applied to the process environment, then put the
/// environment back.
///
/// `None` removes a variable for the duration, which is how a test asserts what
/// happens when one is *not* set — a real case here, since these accessors all
/// have an unset behavior worth pinning.
///
/// Note that a value cached in a `LazyLock` is not affected by this: an
/// accessor that memoizes has to be tested through the uncached function it
/// wraps, or whichever test ran first fixes the answer for the whole process.
pub(crate) fn with_vars<K, V, R>(
    vars: impl IntoIterator<Item = (K, Option<V>)>,
    body: impl FnOnce() -> R,
) -> R
where
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    // Poisoning is not a reason to stop: a panicking body already restored the
    // environment through the guard's `Drop`, and refusing the lock afterwards
    // would fail every later test for someone else's failure.
    let outermost = DEPTH.get() == 0;
    let lock = outermost.then(|| {
        SERIALIZED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    });
    DEPTH.set(DEPTH.get() + 1);
    let mut guard = EnvGuard {
        saved: Vec::new(),
        _lock: lock,
    };
    for (key, value) in vars {
        guard.set(key.as_ref(), value.as_ref().map(AsRef::as_ref));
    }
    body()
}

/// Restores what [`with_vars`] displaced, whether its body returned or panicked.
struct EnvGuard {
    saved: Vec<(OsString, Option<OsString>)>,
    /// `None` on a nested call, whose outer scope already holds it.
    _lock: Option<MutexGuard<'static, ()>>,
}

impl EnvGuard {
    #[allow(unsafe_code)]
    fn set(&mut self, key: &OsStr, value: Option<&OsStr>) {
        self.saved.push((key.to_os_string(), std::env::var_os(key)));
        // SAFETY: `SERIALIZED` is held for as long as this guard lives — by
        // this guard, or by the outer one on a nested call — so no other
        // thread's `with_vars` body is reading or writing the environment
        // concurrently. Tests that mutate the environment without going through
        // here use keys nobody else names.
        unsafe {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

impl Drop for EnvGuard {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        DEPTH.set(DEPTH.get() - 1);
        // Reversed: a key set twice in one call is restored to what it held
        // before the first of them.
        for (key, value) in self.saved.iter().rev() {
            // SAFETY: as in `set`. The lock outlives this loop: `_lock` is
            // dropped after `drop` returns, and on a nested call the outer
            // guard holds it until its own body finishes.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(key: &str) -> Option<String> {
        std::env::var(key).ok()
    }

    #[test]
    fn a_variable_is_set_for_the_body_and_put_back_after() {
        let key = "WASH_ENV_GUARD_SET";
        assert_eq!(value(key), None);
        with_vars([(key, Some("inside"))], || {
            assert_eq!(value(key), Some("inside".to_string()));
        });
        assert_eq!(value(key), None, "the variable is gone again");
    }

    #[test]
    fn an_existing_value_is_restored_not_dropped() {
        let key = "WASH_ENV_GUARD_RESTORE";
        with_vars([(key, Some("original"))], || {
            with_vars([(key, Some("shadowed"))], || {
                assert_eq!(value(key), Some("shadowed".to_string()));
            });
            assert_eq!(
                value(key),
                Some("original".to_string()),
                "the outer value comes back, not the pre-outer one"
            );
        });
        assert_eq!(value(key), None);
    }

    /// `None` is how a test pins what happens when a variable is *unset*, so it
    /// has to remove one that is set.
    #[test]
    fn none_removes_a_variable_for_the_body() {
        let key = "WASH_ENV_GUARD_REMOVE";
        with_vars([(key, Some("present"))], || {
            with_vars([(key, None::<&str>)], || assert_eq!(value(key), None));
            assert_eq!(value(key), Some("present".to_string()));
        });
    }

    /// A failing test must not leave the environment set for whatever runs
    /// next, which is what makes the guard a `Drop` rather than a cleanup call.
    #[test]
    fn a_panicking_body_still_restores() {
        let key = "WASH_ENV_GUARD_PANIC";
        let panicked = std::panic::catch_unwind(|| {
            with_vars([(key, Some("during"))], || panic!("body failed"));
        });
        assert!(panicked.is_err());
        assert_eq!(value(key), None, "restored despite the panic");
    }
}
