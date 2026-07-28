//! Cross-store value interop: moving argument/result `Val` trees — and the
//! `stream`/`future`/`resource` handles they carry — between wasmtime stores for
//! linked calls that don't co-locate caller and callee.
//!
//! - [`relocate`] extracts a `Val` tree in one store and injects it into another.
//! - [`stream_pump`] bridges a `stream<T>`/`future<T>` across the boundary as a
//!   live, no-buffering pump.
//! - [`resource_bridge`] proxies a `resource` handle so its real lives in one
//!   store while callers hold an opaque proxy.

pub(crate) mod relocate;
pub(crate) mod resource_bridge;
pub(crate) mod stream_pump;
