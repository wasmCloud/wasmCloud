//! This crate contains derive macros that enable Rust types to derive [`wrpc_transport::EncodeSync`] and [`wrpc_transport::Receive`] traits.
//!
//! This crate is intended to be used via `wrpc-transport-derive`, the umbrella crate which hosts dependencies required by this (internal) macro crate.
//!
//! # Example
//!
//! ```rust,ignore
//! use wrpc_transport_derive::{Encode, Receive};
//!
//! #[derive(Trace, PartialEq, Eq, EncodeSync, Receive, Default)]
//! struct TestStruct {
//!     one: u32,
//! }
//!
//! let mut buffer: Vec<u8> = Vec::new();
//! // Encode the TestStruct
//! TestStruct { one: 1 }
//!     .encode_sync(&mut buffer)
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
use proc_macro2::TokenStream;
use tracing::trace;
use tracing_subscriber::EnvFilter;

use crate::wrpc_transport::encode_sync::derive_encode_sync_inner;
use crate::wrpc_transport::receive::derive_receive_inner;

mod rust;
mod wrpc_transport;

#[proc_macro_derive(EncodeSync)]
pub fn derive_encode_sync(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let input = TokenStream::from(input);
    trace!("derive(wrpc_transport::EncodeSync) input:\n---\n{input}\n---");

    let derived = derive_encode_sync_inner(input).expect("failed to perform derive");
    trace!("derive(wrpc_transport::EncodeSync) output:\n---\n{derived}\n---");

    derived.into()
}

#[proc_macro_derive(Receive)]
pub fn derive_receive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let input = TokenStream::from(input);
    trace!("derive(wrpc_transport::Receive) input\n---\n{input}\n---");

    let derived = derive_receive_inner(input).expect("failed to perform derive");
    trace!("derive(wrpc_transport::Receive) output:\n---\n{derived}\n---");

    derived.into()
}
