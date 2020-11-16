use super::MessageBus;
use crate::auth::Authorizer;
use crate::capability::binding_cache::BindingCache;
use crate::dispatch::{BusDispatcher, Invocation, InvocationResponse, WasccEntity};
use crate::host_controller::{HostController, MintInvocationRequest};
use crate::messagebus::{
    AdvertiseBinding, AdvertiseClaims, FindBindings, FindBindingsResponse, LookupBinding,
    PutClaims, PutLink, QueryActors, QueryProviders, QueryResponse, SetAuthorizer, SetKey,
    SetProvider, Subscribe, Unsubscribe,
};
use crate::{auth, Result, SYSTEM_ACTOR};
use actix::dev::{MessageResponse, ResponseChannel};
use actix::prelude::*;
use futures::executor::block_on;
use std::collections::HashMap;
use wascap::jwt::Claims;
use wascap::prelude::KeyPair;

pub const OP_PERFORM_LIVE_UPDATE: &str = "PerformLiveUpdate";
pub const OP_IDENTIFY_CAPABILITY: &str = "IdentifyCapability";
pub const OP_HEALTH_REQUEST: &str = "HealthRequest";
pub const OP_INITIALIZE: &str = "Initialize";
pub const OP_BIND_ACTOR: &str = "BindActor";
pub const OP_REMOVE_ACTOR: &str = "RemoveActor";

impl Supervised for MessageBus {}

impl SystemService for MessageBus {
    fn service_started(&mut self, ctx: &mut Context<Self>) {
        info!("Message Bus started");
        // TODO: make this value configurable
        ctx.set_mailbox_capacity(1000);
        self.hb(ctx);
    }
}

impl Actor for MessageBus {
    type Context = Context<Self>;
}

impl Handler<FindBindings> for MessageBus {
    type Result = FindBindingsResponse;

    fn handle(&mut self, msg: FindBindings, _ctx: &mut Context<Self>) -> Self::Result {
        println!(
            "Looking for bindings {:?} - cache size {}",
            &self.binding_cache,
            self.binding_cache.len()
        );
        let res = self
            .binding_cache
            .find_bindings(&msg.binding_name, &msg.provider_id);
        FindBindingsResponse { bindings: res }
    }
}

impl Handler<QueryActors> for MessageBus {
    type Result = QueryResponse;

    fn handle(&mut self, _msg: QueryActors, _ctx: &mut Context<Self>) -> QueryResponse {
        QueryResponse {
            results: self
                .subscribers
                .keys()
                .filter_map(|k| match k {
                    WasccEntity::Actor(s) => Some(s.to_string()),
                    WasccEntity::Capability { .. } => None,
                })
                .collect(),
        }
    }
}

// Receive a notification of claims
impl Handler<PutClaims> for MessageBus {
    type Result = ();

    fn handle(&mut self, msg: PutClaims, _ctx: &mut Context<Self>) {
        self.claims_cache
            .insert(msg.claims.subject.to_string(), msg.claims);
    }
}

// Receive a link definition through an advertisement
impl Handler<PutLink> for MessageBus {
    type Result = ();

    fn handle(&mut self, msg: PutLink, _ctx: &mut Context<Self>) {
        self.binding_cache.add_binding(
            &msg.actor,
            &msg.contract_id,
            &msg.binding_name,
            &msg.provider_id,
            msg.values.clone(),
        );
    }
}

impl Handler<QueryProviders> for MessageBus {
    type Result = QueryResponse;

    fn handle(&mut self, _msg: QueryProviders, _ctx: &mut Context<Self>) -> QueryResponse {
        QueryResponse {
            results: self
                .subscribers
                .keys()
                .filter_map(|k| match k {
                    WasccEntity::Capability { id, .. } => Some(id.to_string()),
                    _ => None,
                })
                .collect(),
        }
    }
}

impl Handler<SetKey> for MessageBus {
    type Result = ();

    fn handle(&mut self, msg: SetKey, _ctx: &mut Context<Self>) {
        self.key = Some(msg.key)
    }
}

impl Handler<SetAuthorizer> for MessageBus {
    type Result = ();

    fn handle(&mut self, msg: SetAuthorizer, _ctx: &mut Context<Self>) {
        self.authorizer = Some(msg.auth);
    }
}

impl Handler<AdvertiseBinding> for MessageBus {
    type Result = ResponseActFuture<Self, Result<()>>;

    fn handle(&mut self, msg: AdvertiseBinding, ctx: &mut Context<Self>) -> Self::Result {
        let target = WasccEntity::Capability {
            id: msg.provider_id.to_string(),
            contract_id: msg.contract_id.to_string(),
            binding: msg.binding_name.to_string(),
        };
        // If there's a lattice provider, tell that provider to advertise said binding
        // if we fail to advertise the binding on the lattice, return and error and skip
        // the local binding code below.
        if let Some(ref lp) = self.provider {
            if let Err(e) = lp.advertise_link(
                &msg.actor,
                &msg.contract_id,
                &msg.binding_name,
                &msg.provider_id,
                msg.values.clone(),
            ) {
                error!("Failed to advertise binding on the lattice: {}", e);
                return Box::pin(async move { Err(e) }.into_actor(self));
            }
        }

        self.binding_cache.add_binding(
            &msg.actor,
            &msg.contract_id,
            &msg.binding_name,
            &msg.provider_id,
            msg.values.clone(),
        );

        if let Some(t) = self.subscribers.get(&target) {
            let req = super::utils::generate_binding_invocation(
                t,
                &msg,
                self.key.as_ref().unwrap(),
                target,
            );
            Box::pin(req.into_actor(self).map(move |res, act, _ctx| match res {
                Ok(ir) => {
                    if let Some(er) = ir.error {
                        Err(format!("Failed to set binding: {}", er).into())
                    } else {
                        Ok(())
                    }
                }
                Err(_) => Err("Mailbox error setting binding".into()),
            }))
        } else {
            // No _local_ subscriber found for this target.
            let is_none = self.provider.as_ref().is_none();
            Box::pin( async move {
                if is_none {
                    info!("No potential targets for advertised binding. Assuming this provider will be added later.");
                }
                Ok(())
            }.into_actor(self))
        }
    }
}

impl Handler<AdvertiseClaims> for MessageBus {
    type Result = Result<()>;

    fn handle(&mut self, msg: AdvertiseClaims, _ctx: &mut Context<Self>) -> Result<()> {
        self.claims_cache
            .insert(msg.claims.subject.to_string(), msg.claims.clone());

        if let Some(ref lp) = self.provider {
            lp.advertise_claims(msg.claims)?
        }
        Ok(())
    }
}

impl Handler<SetProvider> for MessageBus {
    type Result = ();

    fn handle(&mut self, msg: SetProvider, ctx: &mut Context<Self>) {
        self.provider = Some(msg.provider);
        self.provider.as_mut().unwrap().init(BusDispatcher {
            addr: ctx.address().clone(),
        });
        info!(
            "Message bus using provider - {}",
            self.provider.as_ref().unwrap().name()
        );
    }
}

impl Handler<Invocation> for MessageBus {
    type Result = ResponseActFuture<Self, InvocationResponse>;

    /// Handle an invocation from any source to any target. If there is a local subscriber
    /// then the invocation will be delivered directly to that subscriber. If the subscriber
    /// is not local, _and_ there is a lattice provider configured, then the bus will attempt
    /// to satisfy that call via RPC over lattice.
    fn handle(&mut self, msg: Invocation, _ctx: &mut Context<Self>) -> Self::Result {
        if let Err(e) = auth::authorize_invocation(
            &msg,
            self.authorizer.as_ref().unwrap().clone(),
            &self.claims_cache,
        ) {
            error!("Authorization failure: {}", e);
            println!("Authorization failure: {}", e);
            return Box::pin(
                async move {
                    InvocationResponse::error(&msg, &format!("Authorization denied: {}", e))
                }.into_actor(self)
            );
        }
        match self.subscribers.get(&msg.target) {
            Some(target) => Box::pin(target.send(msg.clone()).into_actor(self).map(
                move |res, act, _ctx| {
                    println!("Bus invocation - {:?}", res);
                    if let Ok(r) = res {
                        println!("success");
                        r
                    } else {
                        println!("failure");
                        InvocationResponse::error(
                            &msg,
                            "Mailbox error attempting to perform invocation",
                        )
                    }
                },
            )),
            None => {
                println!("deferring to lattice");
                if let Some(ref l) = self.provider {
                    let res = super::utils::do_rpc(l, &msg);
                    Box::pin(async move { res }.into_actor(self))
                } else {
                    Box::pin(
                        async move {
                            InvocationResponse::error(
                                &msg,
                                &format!("No matching target found on bus {:?}", &msg.target),
                            )
                        }
                        .into_actor(self),
                    )
                }
            }
        }
    }
}

impl Handler<LookupBinding> for MessageBus {
    type Result = Option<String>;

    fn handle(&mut self, msg: LookupBinding, ctx: &mut Self::Context) -> Self::Result {
        self.binding_cache
            .find_provider_id(&msg.actor, &msg.contract_id, &msg.binding_name)
    }
}

impl Handler<Subscribe> for MessageBus {
    type Result = ();

    fn handle(&mut self, msg: Subscribe, _ctx: &mut Context<Self>) -> Self::Result {
        trace!("Bus registered interest for {}", &msg.interest.url());
        self.subscribers
            .insert(msg.interest.clone(), msg.subscriber.clone());
        if let Some(ref mut lp) = self.provider.as_mut() {
            if let Err(e) = lp.register_rpc_listener(&msg.interest) {
                error!("Failed to register lattice interest for {} - actor should be considered unstable.", msg.interest.url());
            }
        }
    }
}

impl Handler<Unsubscribe> for MessageBus {
    type Result = ();

    fn handle(&mut self, msg: Unsubscribe, _ctx: &mut Context<Self>) {
        println!("Unsubscribing {}", msg.interest.url());
        trace!("Bus removing interest for {}", msg.interest.url());
        if let None = self.subscribers.remove(&msg.interest) {
            println!("{:?}", self.subscribers.keys());
            println!("did not remove subscriber {:?}", msg.interest);
        }
        if let Some(ref mut lp) = self.provider.as_mut() {
            if let Err(e) = lp.remove_rpc_listener(&msg.interest) {
                error!(
                    "Failed to remove lattice interest for {} - lattice may be unstable.",
                    msg.interest.url()
                );
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::auth::DefaultAuthorizer;
    use crate::dispatch::{Invocation, InvocationResponse, WasccEntity};
    use crate::messagebus::{MessageBus, SetAuthorizer, SetProvider, Subscribe};
    use crate::Result;
    use crate::{BusDispatcher, LatticeProvider};
    use actix::prelude::*;
    use std::collections::hash_map::RandomState;
    use std::collections::HashMap;
    use wascap::jwt::Claims;
    use wascap::prelude::KeyPair;
    use wascc_codec::capabilities::Dispatcher;

    #[derive(Debug, Clone, Message)]
    #[rtype(result = "u32")]
    struct Query;

    struct HappyActor {
        inv_count: u32,
    }

    impl Actor for HappyActor {
        type Context = SyncContext<Self>;
    }

    impl Handler<Invocation> for HappyActor {
        type Result = InvocationResponse;

        fn handle(&mut self, msg: Invocation, ctx: &mut Self::Context) -> Self::Result {
            self.inv_count = self.inv_count + 1;
            InvocationResponse::success(&msg, vec![])
        }
    }

    impl Handler<Query> for HappyActor {
        type Result = u32;

        fn handle(&mut self, _msg: Query, _ctx: &mut Self::Context) -> Self::Result {
            self.inv_count
        }
    }

    struct FauxLattice {
        result: Vec<u8>,
    }

    impl LatticeProvider for FauxLattice {
        fn name(&self) -> String {
            "FAUX".to_string()
        }

        fn rpc(&self, inv: &Invocation) -> Result<InvocationResponse> {
            Ok(InvocationResponse::success(&inv, self.result.clone()))
        }

        fn init(&mut self, dispatcher: BusDispatcher) {}

        fn register_rpc_listener(&self, subscriber: &WasccEntity) -> Result<()> {
            Ok(())
        }

        fn advertise_binding(
            &self,
            actor: &str,
            contract_id: &str,
            binding_name: &str,
            provider_id: &str,
            values: HashMap<String, String, RandomState>,
        ) -> Result<()> {
            Ok(())
        }

        fn advertise_claims(&self, claims: Claims<wascap::jwt::Actor>) -> Result<()> {
            Ok(())
        }

        fn remove_rpc_listener(&self, subscriber: &WasccEntity) -> Result<()> {
            Ok(())
        }
    }
}
