use crate::control_plane::{ControlPlane, GetProviderForBinding};
use crate::errors::{self, ErrorKind};
use crate::messagebus::MessageBus;
use crate::Result;
use actix::dev::{MessageResponse, ResponseChannel};
use actix::prelude::*;
use data_encoding::HEXUPPER;
use futures::executor::block_on;
use ring::digest::{Context, Digest, SHA256};
use serde::{Deserialize, Serialize};
use std::io::Read;
use uuid::Uuid;
use wascap::prelude::{Claims, KeyPair};

pub(crate) const URL_SCHEME: &str = "wasmbus";

/// An immutable representation of an invocation within waSCC
#[derive(Debug, Clone, Serialize, Deserialize, Message)]
#[rtype(result = "InvocationResponse")]
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

    pub fn origin_url(&self) -> String {
        self.origin.url()
    }

    pub fn target_url(&self) -> String {
        format!("{}/{}", self.target.url(), self.operation)
    }

    pub fn hash(&self) -> String {
        invocation_hash(&self.target_url(), &self.origin_url(), &self.msg)
    }

    pub fn validate_antiforgery(&self) -> Result<()> {
        let vr = wascap::jwt::validate_token::<wascap::prelude::Invocation>(&self.encoded_claims)?;
        let claims = Claims::<wascap::prelude::Invocation>::decode(&self.encoded_claims)?;
        if vr.expired {
            return Err(errors::new(ErrorKind::Authorization(
                "Invocation claims token expired".into(),
            )));
        }
        if !vr.signature_valid {
            return Err(errors::new(ErrorKind::Authorization(
                "Invocation claims signature invalid".into(),
            )));
        }
        if vr.cannot_use_yet {
            return Err(errors::new(ErrorKind::Authorization(
                "Attempt to use invocation before claims token allows".into(),
            )));
        }
        let inv_claims = claims.metadata.unwrap();
        if inv_claims.invocation_hash != self.hash() {
            return Err(errors::new(ErrorKind::Authorization(
                "Invocation hash does not match signed claims hash".into(),
            )));
        }
        if claims.subject != self.id {
            return Err(errors::new(ErrorKind::Authorization(
                "Subject of invocation claims token does not match invocation ID".into(),
            )));
        }
        if claims.issuer != self.host_id {
            return Err(errors::new(ErrorKind::Authorization(
                "Invocation claims issuer does not match invocation host".into(),
            )));
        }
        if inv_claims.target_url != self.target_url() {
            return Err(errors::new(ErrorKind::Authorization(
                "Invocation claims and invocation target URL do not match".into(),
            )));
        }
        if inv_claims.origin_url != self.origin_url() {
            return Err(errors::new(ErrorKind::Authorization(
                "Invocation claims and invocation origin URL do not match".into(),
            )));
        }

        Ok(())
    }
}

/// The response to an invocation
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InvocationResponse {
    pub msg: Vec<u8>,
    pub error: Option<String>,
    pub invocation_id: String,
}

impl InvocationResponse {
    pub fn success(inv: &Invocation, msg: Vec<u8>) -> InvocationResponse {
        InvocationResponse {
            msg,
            error: None,
            invocation_id: inv.id.to_string(),
        }
    }

    pub fn error(inv: &Invocation, err: &str) -> InvocationResponse {
        InvocationResponse {
            msg: Vec::new(),
            error: Some(err.to_string()),
            invocation_id: inv.id.to_string(),
        }
    }
}

impl<A, M> MessageResponse<A, M> for InvocationResponse
where
    A: Actor,
    M: Message<Result = InvocationResponse>,
{
    fn handle<R: ResponseChannel<M>>(self, _: &mut A::Context, tx: Option<R>) {
        if let Some(tx) = tx {
            tx.send(self);
        }
    }
}

/// Represents an invocation target - either an actor or a bound capability provider
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Eq, Hash)]
pub enum WasccEntity {
    Actor(String),
    Capability {
        id: String,
        contract_id: String,
        binding: String,
    },
}

impl WasccEntity {
    pub fn url(&self) -> String {
        match self {
            WasccEntity::Actor(pk) => format!("{}://{}", URL_SCHEME, pk),
            WasccEntity::Capability {
                id,
                contract_id,
                binding,
            } => format!(
                "{}://{}/{}/{}",
                URL_SCHEME,
                contract_id
                    .replace(":", "/")
                    .replace(" ", "_")
                    .to_lowercase(),
                binding.replace(" ", "_").to_lowercase(),
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

pub fn invocation_hash(target_url: &str, origin_url: &str, msg: &[u8]) -> String {
    use std::io::Write;
    let mut cleanbytes: Vec<u8> = Vec::new();
    cleanbytes.write(origin_url.as_bytes()).unwrap();
    cleanbytes.write(target_url.as_bytes()).unwrap();
    cleanbytes.write(msg).unwrap();
    let digest = sha256_digest(cleanbytes.as_slice()).unwrap();
    HEXUPPER.encode(digest.as_ref())
}

pub(crate) fn wapc_host_callback(
    kp: KeyPair,
    claims: Claims<wascap::jwt::Actor>,
    binding_name: &str,
    namespace: &str,
    operation: &str,
    payload: &[u8],
) -> std::result::Result<Vec<u8>, Box<dyn ::std::error::Error + Sync + Send>> {
    trace!(
        "Guest {} invoking {}:{}",
        claims.subject,
        namespace,
        operation
    );

    let capability_id = namespace;
    let inv = invocation_from_callback(
        &kp,
        &claims.subject,
        binding_name,
        namespace,
        operation,
        payload,
    );

    let bus = MessageBus::from_registry();
    match block_on(async { bus.send(inv).await.map(|ir| ir.msg) }) {
        Ok(v) => Ok(v),
        Err(e) => Err("Mailbox error during host callback".into()),
    }
}

fn invocation_from_callback(
    hostkey: &KeyPair,
    origin: &str,
    bd: &str,
    ns: &str,
    op: &str,
    payload: &[u8],
) -> Invocation {
    let binding = if bd.trim().is_empty() {
        // Some actor SDKs may not specify a binding field by default
        "default".to_string()
    } else {
        bd.to_string()
    };
    let target = if ns.len() == 56 && ns.starts_with("M") {
        WasccEntity::Actor(ns.to_string())
    } else {
        // Look up the public key of the provider bound to the origin actor
        // for the given capability contract ID.
        let cp = ControlPlane::from_registry();
        let prov = block_on(async {
            cp.send(GetProviderForBinding {
                contract_id: ns.to_string(),
                actor: origin.to_string(),
            })
            .await
            .unwrap()
        });
        WasccEntity::Capability {
            binding,
            contract_id: ns.to_string(),
            id: prov.unwrap(),
        }
    };
    Invocation::new(
        hostkey,
        WasccEntity::Actor(origin.to_string()),
        target,
        op,
        payload.to_vec(),
    )
}

#[cfg(test)]
mod test {
    use crate::dispatch::{Invocation, WasccEntity};
    use wascap::prelude::KeyPair;

    #[test]
    fn invocation_antiforgery() {
        let hostkey = KeyPair::new_server();
        // As soon as we create the invocation, the claims are baked and signed with the hash embedded.
        let inv = Invocation::new(
            &hostkey,
            WasccEntity::Actor("testing".into()),
            WasccEntity::Capability {
                id: "Vxxx".to_string(),
                contract_id: "wascc:messaging".into(),
                binding: "default".into(),
            },
            "OP_TESTING",
            vec![1, 2, 3, 4],
        );
        let res = inv.validate_antiforgery();
        //println!("{:?}", res);
        // Obviously an invocation we just created should pass anti-forgery check
        assert!(inv.validate_antiforgery().is_ok());

        // Let's tamper with the invocation and we should hit the hash check first
        let mut bad_inv = inv.clone();
        bad_inv.target = WasccEntity::Actor("BADACTOR-EXFILTRATOR".into());
        assert!(bad_inv.validate_antiforgery().is_err());

        // Alter the payload and we should also hit the hash check
        let mut really_bad_inv = inv.clone();
        really_bad_inv.msg = vec![5, 4, 3, 2];
        assert!(really_bad_inv.validate_antiforgery().is_err());

        // And just to double-check the routing address
        assert_eq!(
            inv.target_url(),
            "wasmbus://wascc/messaging/default/Vxxx/OP_TESTING"
        );
    }
}
