use crate::errors::{self, ErrorKind};
use crate::hlreg::HostLocalSystemService;
use crate::messagebus::{LookupLink, MessageBus, OP_BIND_ACTOR};
use crate::{Result, SYSTEM_ACTOR};
use actix::dev::{MessageResponse, ResponseChannel};
use actix::prelude::*;
use data_encoding::HEXUPPER;
use futures::executor::block_on;
use ring::digest::{Context, Digest, SHA256};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::io::Read;
use uuid::Uuid;
use wascap::prelude::{Claims, KeyPair};
use wascc_codec::capabilities::Dispatcher;

pub(crate) const URL_SCHEME: &str = "wasmbus";

pub const CONFIG_WASCC_CLAIMS_ISSUER: &str = "__wascc_issuer";
pub const CONFIG_WASCC_CLAIMS_CAPABILITIES: &str = "__wascc_capabilities";
pub const CONFIG_WASCC_CLAIMS_NAME: &str = "__wascc_name";
pub const CONFIG_WASCC_CLAIMS_EXPIRES: &str = "__wascc_expires";
pub const CONFIG_WASCC_CLAIMS_TAGS: &str = "__wascc_tags";

#[doc(hidden)]
// Given to a capability provider plugin to give it the means
// to communicate with the host machinery
pub struct ProviderDispatcher {
    pub(crate) addr: Recipient<Invocation>, // the bus
    kp: KeyPair,
    me: WasccEntity,
}

impl ProviderDispatcher {
    pub fn new(bus: Recipient<Invocation>, kp: KeyPair, me: WasccEntity) -> ProviderDispatcher {
        ProviderDispatcher { addr: bus, kp, me }
    }
}

impl Dispatcher for ProviderDispatcher {
    fn dispatch(
        &self,
        actor: &str,
        op: &str,
        msg: &[u8],
    ) -> ::std::result::Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        trace!(
            "Provider {} dispatching to bus. Destination {} {}",
            self.me.key(),
            actor,
            op
        );
        let inv = Invocation::new(
            &self.kp,
            self.me.clone(),
            WasccEntity::Actor(actor.to_string()),
            op,
            msg.to_vec(),
        );
        match block_on(async { self.addr.send(inv).await.map(|ir| ir.msg) }) {
            Ok(v) => Ok(v),
            Err(_e) => {
                error!("Provider dispatch to bus failed (mailbox error)");
                Err("Mailbox error during provider dispatch".into())
            }
        }
    }
}

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

    /// A fully-qualified URL indicating the origin of the invocation
    pub fn origin_url(&self) -> String {
        self.origin.url()
    }

    /// A fully-qualified URL indicating the target of the invocation
    pub fn target_url(&self) -> String {
        format!("{}/{}", self.target.url(), self.operation)
    }

    /// The hash of the invocation's target, origin, and raw bytes
    pub fn hash(&self) -> String {
        invocation_hash(&self.target_url(), &self.origin_url(), &self.msg)
    }

    /// Validates the current invocation to ensure that the invocation claims have
    /// not been forged, are not expired, etc
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

impl<A, M> MessageResponse<A, M> for Invocation
where
    A: Actor,
    M: Message<Result = Invocation>,
{
    fn handle<R: ResponseChannel<M>>(self, _: &mut A::Context, tx: Option<R>) {
        if let Some(tx) = tx {
            tx.send(self);
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

impl InvocationResponse {
    /// Creates a successful invocation response. All invocation responses contain the
    /// invocation ID to which they correlate
    pub fn success(inv: &Invocation, msg: Vec<u8>) -> InvocationResponse {
        InvocationResponse {
            msg,
            error: None,
            invocation_id: inv.id.to_string(),
        }
    }

    /// Creates an error response
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

    /// The unique (public) key of the entity
    pub fn key(&self) -> String {
        match self {
            WasccEntity::Actor(pk) => pk.to_string(),
            WasccEntity::Capability { id, .. } => id.to_string(),
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

pub(crate) fn wapc_host_callback(
    kp: KeyPair,
    claims: Claims<wascap::jwt::Actor>,
    link_name: &str,
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

    // Look up the public key of the provider bound to the origin actor
    // for the given capability contract ID.
    let bus = MessageBus::from_hostlocal_registry(&kp.public_key());
    let prov = block_on(async {
        bus.send(LookupLink {
            contract_id: namespace.to_string(),
            actor: claims.subject.to_string(),
            link_name: link_name.to_string(),
        })
        .await
        .unwrap()
    });
    if let Some(p) = prov {
        let inv = invocation_from_callback(
            &kp,
            &claims.subject,
            link_name,
            namespace,
            operation,
            &p,
            payload,
        );
        match block_on(async { bus.send(inv).await.map(|ir| ir.msg) }) {
            Ok(v) => Ok(v),
            Err(_e) => Err("Mailbox error during host callback".into()),
        }
    } else {
        Err(format!(
            "Unable to locate a known link for {}->{}:{}",
            claims.subject, namespace, link_name
        )
        .into())
    }
}

fn invocation_from_callback(
    hostkey: &KeyPair,
    origin: &str,
    bd: &str,
    ns: &str,
    op: &str,
    provider_id: &str,
    payload: &[u8],
) -> Invocation {
    let link_name = if bd.trim().is_empty() {
        // Some actor SDKs may not specify a link field by default
        "default".to_string()
    } else {
        bd.to_string()
    };
    let target = if ns.len() == 56 && ns.starts_with("M") {
        WasccEntity::Actor(ns.to_string())
    } else {
        WasccEntity::Capability {
            link_name,
            contract_id: ns.to_string(),
            id: provider_id.to_string(),
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

pub(crate) fn gen_config_invocation(
    hostkey: &KeyPair,
    actor: &str,
    contract_id: &str,
    provider_id: &str,
    claims: Claims<wascap::jwt::Actor>,
    link_name: String,
    values: HashMap<String, String>,
) -> Invocation {
    let mut values = values.clone();
    values.insert(
        CONFIG_WASCC_CLAIMS_ISSUER.to_string(),
        claims.issuer.to_string(),
    );
    values.insert(
        CONFIG_WASCC_CLAIMS_CAPABILITIES.to_string(),
        claims
            .metadata
            .as_ref()
            .unwrap()
            .caps
            .as_ref()
            .unwrap_or(&Vec::new())
            .join(","),
    );
    values.insert(CONFIG_WASCC_CLAIMS_NAME.to_string(), claims.name());
    values.insert(
        CONFIG_WASCC_CLAIMS_EXPIRES.to_string(),
        claims.expires.unwrap_or(0).to_string(),
    );
    values.insert(
        CONFIG_WASCC_CLAIMS_TAGS.to_string(),
        claims
            .metadata
            .as_ref()
            .unwrap()
            .tags
            .as_ref()
            .unwrap_or(&Vec::new())
            .join(","),
    );
    let cfgvals = crate::generated::core::CapabilityConfiguration {
        module: actor.to_string(),
        values,
    };
    let payload = crate::generated::core::serialize(&cfgvals).unwrap();
    Invocation::new(
        hostkey,
        WasccEntity::Actor(SYSTEM_ACTOR.to_string()),
        WasccEntity::Capability {
            contract_id: contract_id.to_string(),
            id: provider_id.to_string(),
            link_name,
        },
        OP_BIND_ACTOR,
        payload,
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
                link_name: "default".into(),
            },
            "OP_TESTING",
            vec![1, 2, 3, 4],
        );

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
