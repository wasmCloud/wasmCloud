use crate::auth::Authorizer;
use crate::capability::link_cache::LinkCache;
use crate::Result;
use crate::{BusDispatcher, Invocation, InvocationResponse, WasccEntity};
use actix::dev::{MessageResponse, ResponseChannel};
use actix::prelude::*;
use std::collections::HashMap;
use wascap::prelude::{Claims, KeyPair};

pub use handlers::OP_BIND_ACTOR;
pub(crate) mod handlers;
mod hb;
mod utils;

pub trait LatticeProvider: Sync + Send {
    fn init(&mut self, dispatcher: BusDispatcher);
    fn name(&self) -> String;
    fn rpc(&self, inv: &Invocation) -> Result<InvocationResponse>;
    fn register_rpc_listener(&self, subscriber: &WasccEntity) -> Result<()>;
    fn remove_rpc_listener(&self, subscriber: &WasccEntity) -> Result<()>;
    fn advertise_link(
        &self,
        actor: &str,
        contract_id: &str,
        link_name: &str,
        provider_id: &str,
        values: HashMap<String, String>,
    ) -> Result<()>;
    fn advertise_claims(&self, claims: Claims<wascap::jwt::Actor>) -> Result<()>;
}

#[derive(Default)]
pub(crate) struct MessageBus {
    pub provider: Option<Box<dyn LatticeProvider>>,
    subscribers: HashMap<WasccEntity, Recipient<Invocation>>,
    link_cache: LinkCache,
    claims_cache: HashMap<String, Claims<wascap::jwt::Actor>>,
    key: Option<KeyPair>,
    authorizer: Option<Box<dyn Authorizer>>,
}

#[derive(Message)]
#[rtype(result = "QueryResponse")]
pub struct QueryActors;

#[derive(Message)]
#[rtype(result = "QueryResponse")]
pub struct QueryProviders;

pub struct QueryResponse {
    pub results: Vec<String>,
}

impl<A, M> MessageResponse<A, M> for QueryResponse
where
    A: Actor,
    M: Message<Result = QueryResponse>,
{
    fn handle<R: ResponseChannel<M>>(self, _: &mut A::Context, tx: Option<R>) {
        if let Some(tx) = tx {
            tx.send(self);
        }
    }
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct SetProvider {
    pub provider: Box<dyn LatticeProvider>,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct Subscribe {
    pub interest: WasccEntity,
    pub subscriber: Recipient<Invocation>,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct Unsubscribe {
    pub interest: WasccEntity,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct PutClaims {
    pub claims: Claims<wascap::jwt::Actor>,
}

#[derive(Message)]
#[rtype(result = "Option<String>")]
pub struct LookupLink {
    // Capability ID
    pub contract_id: String,
    pub actor: String,
    pub link_name: String,
}

#[derive(Message)]
#[rtype(result = "Result<()>")]
pub struct AdvertiseLink {
    pub contract_id: String,
    pub actor: String,
    pub link_name: String,
    pub provider_id: String,
    pub values: HashMap<String, String>,
}

#[derive(Message)]
#[rtype(result = "Result<()>")]
pub struct AdvertiseClaims {
    pub claims: Claims<wascap::jwt::Actor>,
}

#[derive(Message)]
#[rtype(result = "FindLinksResponse")]
pub struct FindLinks {
    pub provider_id: String,
    pub link_name: String,
}

#[derive(Debug)]
pub struct FindLinksResponse {
    pub links: Vec<(String, HashMap<String, String>)>,
}

impl<A, M> MessageResponse<A, M> for FindLinksResponse
where
    A: Actor,
    M: Message<Result = FindLinksResponse>,
{
    fn handle<R: ResponseChannel<M>>(self, _: &mut A::Context, tx: Option<R>) {
        if let Some(tx) = tx {
            tx.send(self);
        }
    }
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct SetKey {
    pub key: KeyPair,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct SetAuthorizer {
    pub auth: Box<dyn Authorizer>,
}
