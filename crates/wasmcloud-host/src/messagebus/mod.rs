use crate::auth::Authorizer;
use crate::capability::binding_cache::BindingCache;
use crate::Result;
use crate::{Invocation, InvocationResponse, WasccEntity};
use actix::dev::{MessageResponse, ResponseChannel};
use actix::prelude::*;
use std::collections::HashMap;
use wascap::prelude::{Claims, KeyPair};

use crate::messagebus::rpc_client::RpcClient;
pub use handlers::OP_BIND_ACTOR;
use std::sync::Arc;
use std::time::Duration;

pub(crate) mod handlers;
mod hb;
pub(crate) mod nats_subscriber;
pub(crate) mod rpc_client;
pub(crate) mod rpc_subscription;
mod utils;

pub(crate) use nats_subscriber::{NatsMessage, NatsSubscriber};

#[derive(Default)]
pub(crate) struct MessageBus {
    nc: Option<nats::asynk::Connection>,
    namespace: Option<String>,
    subscribers: HashMap<WasccEntity, Recipient<Invocation>>,
    rpc_outbound: Option<Addr<RpcClient>>,
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
pub struct Initialize {
    pub nc: Option<nats::asynk::Connection>,
    pub namespace: Option<String>,
    pub key: KeyPair,
    pub auth: Box<dyn Authorizer>,
    pub rpc_timeout: Duration,
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

#[derive(Message, Clone)]
#[rtype(result = "Result<()>")]
pub struct AdvertiseBinding {
    pub contract_id: String,
    pub actor: String,
    pub binding_name: String,
    pub provider_id: String,
    pub values: HashMap<String, String>,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct PutLink {
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

#[derive(Message)]
#[rtype(result = "ClaimsResponse")]
pub struct GetClaims;

#[derive(Debug)]
pub struct ClaimsResponse {
    pub claims: HashMap<String, Claims<wascap::jwt::Actor>>,
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

impl<A, M> MessageResponse<A, M> for ClaimsResponse
where
    A: Actor,
    M: Message<Result = ClaimsResponse>,
{
    fn handle<R: ResponseChannel<M>>(self, _: &mut A::Context, tx: Option<R>) {
        if let Some(tx) = tx {
            tx.send(self);
        }
    }
}
