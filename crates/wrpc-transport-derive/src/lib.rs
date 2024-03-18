//! This crate exposes derive macros that enable Rust types to derive [`wrpc_transport::Encode`] and [`wrpc_transport::Receive`] traits.
//!
//! # Example
//!
//! ```rust,ignore
//! use wrpc_transport_derive::{Encode, Receive};
//!
//! #[derive(Trace, PartialEq, Eq, Encode, Receive, Default)]
//! struct TestStruct {
//!     one: u32,
//! }
//!
//! let mut buffer: Vec<u8> = Vec::new();
//! // Encode the TestStruct
//! TestStruct { one: 1 }
//!     .encode(&mut buffer)
//!     .await
//!     .context("failed to perform encode")?;
//!
//! // Attempt to receive the value
//! let (received, leftover): (TestStruct, _) =
//!     Receive::receive_sync(Bytes::from(buffer), &mut empty())
//!         .await
//!         .context("failed to receive")?;
//!
//! // At this point, we expect the received bytes to be exactly the same as what we started with
//! assert_eq!(received, TestStruct { one: 1 }, "received matches");
//! assert_eq!(leftover.remaining(), 0, "payload was completely consumed");
//! ```
//!
//! NOTE: This macro crate uses `tracing`, so if you'd like to see the input & output tokens, prepend your command (ex. `cargo build`) with `RUST_LOG=trace`.
//!

/// Dependencies of the macros container in the "inner" macro crate (`wrpc_transport_derive_macro`),
/// hoisted to this level so they can be referenced from the inner macro, and are sure to be included
pub mod deps {
    pub use anyhow;
    pub use async_trait;
    pub use bytes;
    pub use futures;
    pub use wrpc_transport;
}

pub use wrpc_transport_derive_macro::{Encode, Receive, Subscribe};

/// Re-export of [`wrpc_transport::Encode`] to make usage easier
pub use wrpc_transport::{Encode, Receive};
