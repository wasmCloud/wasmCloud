//! Cross-store value interop: moving argument/result `Val` trees — and the
//! `stream`/`future` handles they carry — between wasmtime stores for linked
//! calls that don't co-locate caller and callee.
//!
//! - [`relocate`] extracts a `Val` tree in one store and injects it into another.
//! - [`stream_pump`] bridges a `stream<T>`/`future<T>` across the boundary as a
//!   live, no-buffering pump.

pub(crate) mod relocate;
pub(crate) mod stream_pump;
