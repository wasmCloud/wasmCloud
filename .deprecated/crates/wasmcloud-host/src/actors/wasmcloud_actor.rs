use crate::Result;
use std::fs::File;
use std::io::prelude::*;
use std::path::Path;
use wascap::jwt::{Claims, Token};

/// An actor is a WebAssembly module that conforms to the wasmCloud protocols and can securely
/// consume capabilities exposed by capability providers.
#[derive(Debug)]
pub struct WasmCloudActor {
    pub(crate) token: Token<wascap::jwt::Actor>,
    pub(crate) bytes: Vec<u8>,
}

impl WasmCloudActor {
    /// Create an actor from the bytes of a signed WebAssembly module. Attempting to load
    /// an unsigned module, or a module signed improperly, will result in an error.
    pub fn from_slice(buf: &[u8]) -> Result<WasmCloudActor> {
        let token = wascap::wasm::extract_claims(&buf)?;
        if let Some(t) = token {
            Ok(WasmCloudActor {
                token: t,
                bytes: buf.to_vec(),
            })
        } else {
            Err("Unable to extract embedded token from WebAssembly module".into())
        }
    }

    /// Create an actor from a signed WebAssembly (`.wasm`) file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<WasmCloudActor> {
        let mut file = File::open(path)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;

        WasmCloudActor::from_slice(&buf)
    }

    /// Obtain the actor's public key (The `sub` field of the JWT). It is safe to treat this value as a globally unique identifier.
    pub fn public_key(&self) -> String {
        self.token.claims.subject.to_string()
    }

    /// The actor's human-friendly display name
    pub fn name(&self) -> String {
        match self.token.claims.metadata.as_ref().unwrap().name {
            Some(ref n) => n.to_string(),
            None => "Unnamed".to_string(),
        }
    }

    /// Obtain the issuer's public key as it resides in the actor's token (the `iss` field of the JWT).
    pub fn issuer(&self) -> String {
        self.token.claims.issuer.to_string()
    }

    /// Obtain the list of capabilities declared in this actor's embedded token.
    pub fn capabilities(&self) -> Vec<String> {
        match self.token.claims.metadata.as_ref().unwrap().caps {
            Some(ref caps) => caps.clone(),
            None => vec![],
        }
    }

    /// Obtain the list of tags in the actor's token.
    pub fn tags(&self) -> Vec<String> {
        match self.token.claims.metadata.as_ref().unwrap().tags {
            Some(ref tags) => tags.clone(),
            None => vec![],
        }
    }

    // Obtain the raw set of claims for this actor.
    pub fn claims(&self) -> Claims<wascap::jwt::Actor> {
        self.token.claims.clone()
    }
}
