//! A library for managing signed JWT (JSON Web Tokens) in WebAssembly modules. These
//! are designed to be used with the [wasmCloud](https://github.com/wasmCloud) host, but can be
//! used for any WebAssembly module, as the embedding technique used is compliant with
//! the WebAssembly standard.
//!
//! This library can be used for embedding, extracting, and validating capabilities claims
//! in WebAssembly modules. While there are some standard, well-known claims already defined
//! for use with *wasmCloud*, you can add custom claims in your own namespaces if you like.
//!
//! The following example illustrates embedding a new set of claims
//! into a WebAssembly module, then extracting, validating, and examining those claims.
//! ```rust
//!use wascap::prelude::*;
//!
//!# fn read_unsigned_wasm() -> Vec<u8> {
//!#   include_bytes!("../examples/loop.wasm").to_vec()
//!# }
//!# fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
//! let unsigned = read_unsigned_wasm(); // Read a Wasm file into a byte vector
//! let issuer = KeyPair::new_account(); // Create an Ed25519 key pair to sign the module
//! let module = KeyPair::new_module(); // Create a key pair for the module itself
//!
//! // Grant the module some basic capabilities, with no date limits
//! let claims = ClaimsBuilder::<Actor>::new()
//!     .issuer(&issuer.public_key())
//!     .subject(&module.public_key())
//!     .with_metadata(Actor{
//!         name: Some("test".to_string()),
//!         caps: Some(vec![caps::MESSAGING.to_string(), caps::KEY_VALUE.to_string()]),
//!         .. Default::default()
//!      })
//!     .build();
//!
//! // Sign the JWT and embed it into the WebAssembly module, returning the signed bytes
//! let embedded = wasm::embed_claims(&unsigned, &claims, &issuer)?;
//!
//! // Extract a signed JWT from a WebAssembly module's bytes (performs a check on
//! // the signed module hash)
//! let extracted = wasm::extract_claims(&embedded)?.unwrap();
//!
//! // Validate dates, signature, JWT structure, etc.
//! let v = validate_token::<Actor>(&extracted.jwt)?;
//!
//! assert_eq!(v.expired, false);
//! assert_eq!(v.cannot_use_yet, false);
//! assert_eq!(v.expires_human, "never");
//! assert_eq!(v.not_before_human, "immediately");
//! assert_eq!(extracted.claims.issuer, issuer.public_key());
//!
//!# Ok(())
//!# }
//! ```
//!
//! The `Ed25519` key functionality is provided by the [nkeys](https://docs.rs/nkeys) crate.

/// Wascap-specific result type
pub type Result<T> = std::result::Result<T, errors::Error>;
pub use errors::Error;

pub mod caps;
mod errors;
pub mod jwt;
pub mod wasm;

pub mod prelude {
    //! Public re-exports of the most commonly used wascap types
    pub use super::{Error as WascapError, Result as WascapResult};
    pub use crate::{
        caps,
        jwt::{validate_token, Account, Actor, Claims, ClaimsBuilder, Invocation, Operator},
        wasm,
    };
    pub use nkeys::KeyPair;
}
