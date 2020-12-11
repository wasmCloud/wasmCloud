// Philosophical note: These two main structs (Invocation and InvocationResponse) exist inside
// the wasmcloud-host crate. If this crate were to refer to that, we'd create a cyclical reference
// and all perish inside a black hole.
//
// The other alternative is to create a new, third crate shared by the two, but that creates a brand
// new "reason to change" now for three crates and produces a "dependency as a liability"
//
// Therefore, it's easier to simply create copies of the data types since we only need them to be
// able to push them on the wire. The "real" Invocation and InvocationResponse types are in the wasmcloud-host
// crate in the dispatch module because we need to implement other traits on those types.

use crate::Result;
use data_encoding::HEXUPPER;
use ring::digest::{Context, Digest, SHA256};
use serde::{Deserialize, Serialize};
use std::io::Read;
use uuid::Uuid;
use wascap::jwt::Claims;
use wascap::prelude::KeyPair;

const URL_SCHEME: &str = "wasmbus";

/// An immutable representation of an invocation within waSCC
#[derive(Debug, Clone, Serialize, Deserialize)]

pub struct Invocation {
    pub origin: WasccEntity,
    pub target: WasccEntity,
    pub operation: String,
    pub msg: Vec<u8>,
    pub id: String,
    pub encoded_claims: String,
    pub host_id: String,
}

impl Invocation {
    /// Creates a new invocation. All invocations are signed with the host key as a way
    /// of preventing them from being forged over the network when connected to a lattice,
    /// so an invocation requires a reference to the host (signing) key
    pub fn new(
        hostkey: &KeyPair,
        origin: WasccEntity,
        target: WasccEntity,
        op: &str,
        msg: Vec<u8>,
    ) -> Invocation {
        let subject = format!("{}", Uuid::new_v4());
        let issuer = hostkey.public_key();
        let target_url = format!("{}/{}", target.url(), op);
        let claims = Claims::<wascap::prelude::Invocation>::new(
            issuer.to_string(),
            subject.to_string(),
            &target_url,
            &origin.url(),
            &invocation_hash(&target_url, &origin.url(), &msg),
        );
        Invocation {
            origin,
            target,
            operation: op.to_string(),
            msg,
            id: subject,
            encoded_claims: claims.encode(&hostkey).unwrap(),
            host_id: issuer.to_string(),
        }
    }
}

/// The response to an invocation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvocationResponse {
    pub msg: Vec<u8>,
    pub error: Option<String>,
    pub invocation_id: String,
}

/// Represents an entity within the host runtime that can be the source
/// or target of an invocation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Eq, Hash)]
pub enum WasccEntity {
    Actor(String),
    Capability {
        id: String,
        contract_id: String,
        link_name: String,
    },
}

impl WasccEntity {
    /// The URL of the entity
    pub fn url(&self) -> String {
        match self {
            WasccEntity::Actor(pk) => format!("{}://{}", URL_SCHEME, pk),
            WasccEntity::Capability {
                id,
                contract_id,
                link_name,
            } => format!(
                "{}://{}/{}/{}",
                URL_SCHEME,
                contract_id
                    .replace(":", "/")
                    .replace(" ", "_")
                    .to_lowercase(),
                link_name.replace(" ", "_").to_lowercase(),
                id
            ),
        }
    }
}

fn sha256_digest<R: Read>(mut reader: R) -> Result<Digest> {
    let mut context = Context::new(&SHA256);
    let mut buffer = [0; 1024];

    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        context.update(&buffer[..count]);
    }

    Ok(context.finish())
}

pub(crate) fn invocation_hash(target_url: &str, origin_url: &str, msg: &[u8]) -> String {
    use std::io::Write;
    let mut cleanbytes: Vec<u8> = Vec::new();
    cleanbytes.write(origin_url.as_bytes()).unwrap();
    cleanbytes.write(target_url.as_bytes()).unwrap();
    cleanbytes.write(msg).unwrap();
    let digest = sha256_digest(cleanbytes.as_slice()).unwrap();
    HEXUPPER.encode(digest.as_ref())
}
