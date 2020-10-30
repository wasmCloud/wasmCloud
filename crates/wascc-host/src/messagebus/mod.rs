use std::collections::HashMap;
use crate::capability::binding_cache::BindingCache;
use crate::{WasccEntity, Invocation, BusDispatcher, InvocationResponse};
use actix::prelude::*;
use wascap::prelude::{Claims, KeyPair};
use crate::auth::Authorizer;
use actix::dev::{MessageResponse, ResponseChannel};
use crate::Result;

pub use handlers::OP_BIND_ACTOR;
pub(crate) mod handlers;
mod utils;
mod hb;

pub trait LatticeProvider: Sync + Send {
    fn init(&mut self, dispatcher: BusDispatcher);
    fn name(&self) -> String;
    fn rpc(&self, inv: &Invocation) -> Result<InvocationResponse>;
    fn register_rpc_listener(&self, subscriber: &WasccEntity) -> Result<()>;
    fn remove_rpc_listener(&self, subscriber: &WasccEntity) -> Result<()>;
    fn advertise_binding(
        &self,
        actor: &str,
        contract_id: &str,
        binding_name: &str,
        provider_id: &str,
        values: HashMap<String, String>,
    ) -> Result<()>;
    fn advertise_claims(&self, claims: Claims<wascap::jwt::Actor>) -> Result<()>;
}



#[derive(Default)]
pub(crate) struct MessageBus {
    pub provider: Option<Box<dyn LatticeProvider>>,
    subscribers: HashMap<WasccEntity, Recipient<Invocation>>,
    binding_cache: BindingCache,
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
pub struct LookupBinding {
    // Capability ID
    pub contract_id: String,
    pub actor: String,
    pub binding_name: String,
}

#[derive(Message)]
#[rtype(result = "Result<()>")]
pub struct AdvertiseBinding {
    pub contract_id: String,
    pub actor: String,
    pub binding_name: String,
    pub provider_id: String,
    pub values: HashMap<String, String>,
}

#[derive(Message)]
#[rtype(result = "Result<()>")]
pub struct AdvertiseClaims {
    pub claims: Claims<wascap::jwt::Actor>,
}

#[derive(Message)]
#[rtype(result = "FindBindingsResponse")]
pub struct FindBindings {
    pub provider_id: String,
    pub binding_name: String,
}

#[derive(Debug)]
pub struct FindBindingsResponse {
    pub bindings: Vec<(String, HashMap<String, String>)>,
}

impl<A, M> MessageResponse<A, M> for FindBindingsResponse
    where
        A: Actor,
        M: Message<Result = FindBindingsResponse>,
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
