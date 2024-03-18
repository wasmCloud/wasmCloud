//! This crate contains derive macros that enable Rust types to derive [`wrpc_transport::Encode`] and [`wrpc_transport::Receive`] traits.
//!
//! This crate is intended to be used via `wrpc-transport-derive`, the umbrella crate which hosts dependencies required by this (internal) macro crate.
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
use proc_macro2::TokenStream;
use tracing::trace;
use tracing_subscriber::EnvFilter;

use crate::wrpc_transport::encode::derive_encode_inner;
use crate::wrpc_transport::receive::{derive_receive_inner, derive_subscribe_inner};

mod config;
mod wrpc_transport;

/// Derive an [`wrpc_transport::Encode`] implementation
#[proc_macro_derive(Encode, attributes(wrpc_transport_derive))]
pub fn derive_encode(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let input = TokenStream::from(input);
    trace!("derive(wrpc_transport::Encode) input:\n---\n{input}\n---");

    let derived = derive_encode_inner(input).expect("failed to perform derive");
    trace!("derive(wrpc_transport::Encode) output:\n---\n{derived}\n---");

    derived.into()
}

/// Derive an [`wrpc_transport::Receive`] implementation
#[proc_macro_derive(Receive, attributes(wrpc_transport_derive))]
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

/// Derive an [`wrpc_transport::Subscribe`] implementation
#[proc_macro_derive(Subscribe, attributes(wrpc_transport_derive))]
pub fn derive_subscribe(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let input = TokenStream::from(input);
    trace!("derive(wrpc_transport::Subscribe) input\n---\n{input}\n---");

    let derived = derive_subscribe_inner(input).expect("failed to perform derive");
    trace!("derive(wrpc_transport::Subscribe) output:\n---\n{derived}\n---");

    derived.into()
}
