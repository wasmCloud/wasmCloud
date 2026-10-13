//! Load-time repair for guests whose async runtime wasmtime traps.
//!
//! wit-bindgen before 0.59 cancels its wakeup stream's read while the stream
//! is still in a waitable set, which wasmtime traps from 48 on. The repair
//! swaps that cancel with the `waitable.join` after it, in place.

use std::borrow::Cow;
use std::ops::Range;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};

use opentelemetry::KeyValue;
use opentelemetry::metrics::Counter;
use wasmparser::{Encoding, FunctionBody, Operator, Parser, Payload, TypeRef};

use crate::engine::ctx::SharedCtx;

/// Version of the rewrite. Changes whenever the same input could produce
/// different output.
pub const PASS_VERSION: u32 = 1;

/// The module wit-bindgen's runtime imports its canonical builtins from.
const BUILTINS: &str = "$root";
/// `stream.cancel-read` on the unit stream the runtime wakes tasks with.
const CANCEL_READ: &str = "[stream-cancel-read-unit]";
/// `waitable.join`, which with a zero set removes a waitable from its set.
const JOIN: &str = "[waitable-join]";
const REBUILD: &str = "rebuild it with wit-bindgen 0.59 or later (wasip3 0.9 or later)";

/// What to do with a guest whose async runtime cancels in the trapping order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GuestCompat {
    /// Reorder the calls in the bytes handed to the compiler.
    #[default]
    Auto,
    /// Report the guest and compile it as it arrived.
    Detect,
    /// Compile every guest as it arrived, without looking.
    Off,
}

impl GuestCompat {
    /// The spelling used by `WASH_GUEST_COMPAT` and in logs.
    pub fn as_str(self) -> &'static str {
        match self {
            GuestCompat::Auto => "auto",
            GuestCompat::Detect => "detect",
            GuestCompat::Off => "off",
        }
    }
}

impl FromStr for GuestCompat {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(GuestCompat::Auto),
            "detect" => Ok(GuestCompat::Detect),
            "off" => Ok(GuestCompat::Off),
            _ => Err(format!(
                "invalid guest compatibility mode {raw:?}; expected 'auto', 'detect' or 'off'"
            )),
        }
    }
}

/// The repair as one engine runs it.
#[derive(Clone)]
pub(crate) struct Pass {
    mode: GuestCompat,
    /// Loads of a component found with a legacy call site, by mode.
    found: Counter<u64>,
}

impl Pass {
    pub(crate) fn new(mode: GuestCompat) -> Self {
        let found = opentelemetry::global::meter("wash-runtime")
            .u64_counter("guest.compat.loads")
            .with_description(
                "Loads of a component whose async runtime cancels a stream read before leaving \
                 its waitable set, by what the host did about it",
            )
            .build();
        Self { mode, found }
    }

    /// The bytes to compile for `bytes`: a reordered copy under
    /// [`GuestCompat::Auto`] when the guest has a legacy call site, otherwise
    /// `bytes`. `digest` names the bytes that arrived and only labels the log.
    pub(crate) fn prepare<'a>(&self, bytes: &'a [u8], digest: Option<&str>) -> Cow<'a, [u8]> {
        if self.mode == GuestCompat::Off {
            return Cow::Borrowed(bytes);
        }
        let digest = digest.unwrap_or("none");
        let sites = match legacy_cancel_sites(bytes) {
            Ok(sites) if sites.is_empty() => return Cow::Borrowed(bytes),
            Ok(sites) => sites,
            // The compiler reads with a parser of its own and may accept what
            // this one did not, so say that the component went unchecked.
            Err(e) => {
                tracing::warn!(
                    digest,
                    err = %e,
                    "could not scan the component for a legacy async runtime; \
                     compiling it as it arrived"
                );
                return Cow::Borrowed(bytes);
            }
        };
        self.found
            .add(1, &[KeyValue::new("mode", self.mode.as_str())]);
        match self.mode {
            GuestCompat::Auto => {
                let mut patched = bytes.to_vec();
                for site in &sites {
                    site.reorder(&mut patched);
                }
                tracing::warn!(
                    digest,
                    sites = sites.len(),
                    pass_version = PASS_VERSION,
                    advice = REBUILD,
                    "component cancels a stream read before leaving its waitable set; \
                     reordered the calls in memory before compiling, so what runs is not \
                     the bytes this digest names"
                );
                Cow::Owned(patched)
            }
            GuestCompat::Detect | GuestCompat::Off => {
                tracing::warn!(
                    digest,
                    sites = sites.len(),
                    advice = REBUILD,
                    "component cancels a stream read before leaving its waitable set, \
                     and will trap when a call suspends"
                );
                Cow::Borrowed(bytes)
            }
        }
    }
}

/// Whether a workload's owner has been told what its trap may mean. Shared by
/// every store built for the workload, so they are told once.
#[derive(Debug, Default)]
pub struct RebuildAdvised(AtomicBool);

/// Log, once per workload, what its owner can do about `err`, when it is the
/// trap an unrepaired legacy guest hits. Per workload because linked
/// components share a store, and the trap does not say which one raised it.
/// Logging the fault itself stays with the caller.
pub(crate) fn advise_rebuild(ctx: &SharedCtx, err: &wasmtime::Error) {
    match err.downcast_ref::<wasmtime::Trap>() {
        Some(wasmtime::Trap::WaitableSyncAndAsync) => {}
        _ => return,
    }
    if ctx.rebuild_advised.0.swap(true, Ordering::Relaxed) {
        return;
    }
    let identity = ctx.executed.identity();
    tracing::error!(
        workload.id = &*ctx.active_ctx.workload_id,
        workload.namespace = identity.map(|i| &*i.namespace),
        workload.name = identity.map(|i| &*i.name),
        "a component of this workload used a waitable synchronously while it was in a waitable \
         set. A guest built with wit-bindgen older than 0.59 does that whenever a call suspends; \
         if this one was, {REBUILD}"
    );
}

/// One legacy call site: `cancel(handle); drop; join(handle, 0)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacySite {
    /// The whole sequence, as byte offsets into the component.
    pub range: Range<usize>,
    /// Where the `join` half starts.
    join_at: usize,
}

impl LegacySite {
    /// Move the `join` half in front of the cancel half.
    fn reorder(&self, bytes: &mut [u8]) {
        if let Some(site) = bytes.get_mut(self.range.clone()) {
            site.rotate_left(self.join_at - self.range.start);
        }
    }
}

/// Where a core module's two builtins sit in its function index space.
#[derive(Default)]
struct Builtins {
    imported_functions: u32,
    cancel_read: Option<u32>,
    join: Option<u32>,
}

/// The instructions the legacy sequence is made of.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    LocalGet(u32),
    CallCancel,
    Drop,
    Zero,
    CallJoin,
    Other,
}

/// Every place `component` cancels the wakeup stream's read and then removes
/// the stream from its waitable set. An unoptimized build reports none: its
/// two calls sit in separate functions, where legacy and fixed look the same.
/// Errors when `component` cannot be parsed as far as its function bodies.
pub fn legacy_cancel_sites(component: &[u8]) -> anyhow::Result<Vec<LegacySite>> {
    let mut sites = Vec::new();
    // One entry per module or component being read, innermost last. Only a
    // core module has builtins to find.
    let mut nesting: Vec<Option<Builtins>> = Vec::new();
    for payload in Parser::new(0).parse_all(component) {
        match payload? {
            Payload::Version { encoding, .. } => nesting.push(match encoding {
                Encoding::Module => Some(Builtins::default()),
                Encoding::Component => None,
            }),
            Payload::End(_) => {
                nesting.pop();
            }
            Payload::ImportSection(imports) => {
                let Some(Some(builtins)) = nesting.last_mut() else {
                    continue;
                };
                for import in imports.into_imports() {
                    let import = import?;
                    if !matches!(import.ty, TypeRef::Func(_) | TypeRef::FuncExact(_)) {
                        continue;
                    }
                    if import.module == BUILTINS {
                        match import.name {
                            CANCEL_READ => builtins.cancel_read = Some(builtins.imported_functions),
                            JOIN => builtins.join = Some(builtins.imported_functions),
                            _ => {}
                        }
                    }
                    builtins.imported_functions += 1;
                }
            }
            Payload::CodeSectionEntry(body) => {
                if let Some(Some(Builtins {
                    cancel_read: Some(cancel_read),
                    join: Some(join),
                    ..
                })) = nesting.last()
                {
                    scan_body(&body, *cancel_read, *join, &mut sites)?;
                }
            }
            _ => {}
        }
    }
    Ok(sites)
}

/// Append the legacy sites in one function body.
fn scan_body(
    body: &FunctionBody<'_>,
    cancel_read: u32,
    join: u32,
    sites: &mut Vec<LegacySite>,
) -> anyhow::Result<()> {
    // The six instructions before the one being read, oldest first, each with
    // the offset it starts at.
    let mut window = [(Step::Other, 0usize); 6];
    for op in body.get_operators_reader()?.into_iter_with_offsets() {
        let (op, offset) = op?;
        let offset = usize::try_from(offset)?;
        if let [
            (Step::LocalGet(cancelled), start),
            (Step::CallCancel, _),
            (Step::Drop, _),
            (Step::LocalGet(joined), join_at),
            (Step::Zero, _),
            (Step::CallJoin, _),
        ] = window
            && cancelled == joined
        {
            sites.push(LegacySite {
                range: start..offset,
                join_at,
            });
        }
        let step = match op {
            Operator::LocalGet { local_index } => Step::LocalGet(local_index),
            Operator::Call { function_index } if function_index == cancel_read => Step::CallCancel,
            Operator::Call { function_index } if function_index == join => Step::CallJoin,
            Operator::Drop => Step::Drop,
            Operator::I32Const { value: 0 } => Step::Zero,
            _ => Step::Other,
        };
        let [_, second, third, fourth, fifth, sixth] = window;
        window = [second, third, fourth, fifth, sixth, (step, offset)];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A component whose core module holds `body` in a function taking the
    /// stream handle as local 0, with the builtins imported after `padding`
    /// other functions so their indices need more than one byte.
    fn component(padding: usize, body: &str) -> Vec<u8> {
        let padding: String = (0..padding)
            .map(|n| format!(r#"(import "env" "f{n}" (func))"#))
            .collect();
        wat::parse_str(format!(
            r#"(component
                (core module
                    {padding}
                    (import "$root" "[waitable-join]" (func $join (param i32 i32)))
                    (import "$root" "[stream-cancel-read-unit]"
                        (func $cancel (param i32) (result i32)))
                    (import "env" "cancel-other" (func $other (param i32) (result i32)))
                    (func (param i32) (local i32) {body})))"#
        ))
        .expect("the test component is valid WAT")
    }

    fn prepare(mode: GuestCompat, bytes: &[u8]) -> Cow<'_, [u8]> {
        Pass::new(mode).prepare(bytes, None)
    }

    const LEGACY: &str = "local.get 0 call $cancel drop local.get 0 i32.const 0 call $join";
    const FIXED: &str = "local.get 0 i32.const 0 call $join local.get 0 call $cancel drop";

    #[test]
    fn reorders_the_legacy_sequence_into_the_fixed_one() {
        for padding in [0, 200] {
            let legacy = component(padding, LEGACY);
            let patched = prepare(GuestCompat::Auto, &legacy);
            assert_eq!(*patched, *component(padding, FIXED), "padding {padding}");
            assert_eq!(legacy_cancel_sites(&patched).expect("still parses"), []);
        }
    }

    #[test]
    fn reorders_every_site_and_nothing_between_them() {
        let body = |site: &str| format!("{site} i32.const 7 drop {site}");
        let legacy = component(0, &body(LEGACY));
        let patched = prepare(GuestCompat::Auto, &legacy);
        assert_eq!(*patched, *component(0, &body(FIXED)));
    }

    #[test]
    fn leaves_a_fixed_guest_byte_identical() {
        let fixed = component(0, FIXED);
        assert!(matches!(
            prepare(GuestCompat::Auto, &fixed),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn leaves_anything_short_of_the_exact_sequence_alone() {
        for body in [
            // A different handle is removed than was cancelled.
            "local.get 0 call $cancel drop local.get 1 i32.const 0 call $join",
            // Joins a set instead of leaving one.
            "local.get 0 call $cancel drop local.get 0 i32.const 1 call $join",
            // The result is used.
            "local.get 0 call $cancel local.set 1 local.get 0 i32.const 0 call $join",
            // Some other cancel.
            "local.get 0 call $other drop local.get 0 i32.const 0 call $join",
            // Something runs in between.
            "local.get 0 call $cancel drop nop local.get 0 i32.const 0 call $join",
        ] {
            let bytes = component(0, body);
            assert_eq!(legacy_cancel_sites(&bytes).expect("parses"), [], "{body}");
        }
    }

    #[test]
    fn detect_and_off_compile_the_bytes_that_arrived() {
        let legacy = component(0, LEGACY);
        for mode in [GuestCompat::Detect, GuestCompat::Off] {
            assert!(
                matches!(prepare(mode, &legacy), Cow::Borrowed(_)),
                "{mode:?}"
            );
        }
    }

    #[test]
    fn ignores_the_names_outside_the_builtin_module() {
        let bytes = wat::parse_str(
            r#"(component (core module
                (import "env" "[waitable-join]" (func $join (param i32 i32)))
                (import "env" "[stream-cancel-read-unit]" (func $cancel (param i32) (result i32)))
                (func (param i32)
                    local.get 0 call $cancel drop local.get 0 i32.const 0 call $join)))"#,
        )
        .expect("valid WAT");
        assert_eq!(legacy_cancel_sites(&bytes).expect("parses"), []);
    }

    #[test]
    fn unparseable_bytes_are_handed_on_untouched() {
        let junk = b"\0asm not a component";
        assert!(legacy_cancel_sites(junk).is_err());
        assert!(matches!(prepare(GuestCompat::Auto, junk), Cow::Borrowed(_)));
    }

    #[test]
    fn an_environment_value_that_is_not_a_mode_is_an_error() {
        let key = format!("WASH_GUEST_COMPAT_{}", uuid::Uuid::new_v4().simple());
        let read = |value: Option<&str>| {
            crate::env_guard::with_vars([(key.as_str(), value)], || {
                crate::engine::getenv_strict::<GuestCompat>(&key)
            })
        };
        assert_eq!(read(None).expect("unset"), None);
        assert_eq!(read(Some(" ")).expect("blank"), None);
        assert_eq!(
            read(Some("detect")).expect("a mode"),
            Some(GuestCompat::Detect)
        );
        assert!(read(Some("false")).is_err());
    }

    #[test]
    fn modes_parse_from_their_spelling() {
        for mode in [GuestCompat::Auto, GuestCompat::Detect, GuestCompat::Off] {
            assert_eq!(mode.as_str().parse(), Ok(mode));
        }
        for not_a_mode in ["on", "false", ""] {
            assert!(not_a_mode.parse::<GuestCompat>().is_err(), "{not_a_mode}");
        }
    }
}
