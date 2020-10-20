use crate::capability::binding_cache::BindingCache;
use crate::dispatch::{BusDispatcher, Invocation, InvocationResponse, WasccEntity};
use crate::host_controller::{HostController, MintInvocationRequest};
use crate::{Result, SYSTEM_ACTOR};
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
#[rtype(result = "()")]
pub struct SetKey {
    pub key: KeyPair,
}

pub trait LatticeProvider: Sync + Send {
    fn init(&mut self, dispatcher: BusDispatcher);
    fn name(&self) -> String;
    fn rpc(&self, inv: &Invocation) -> Result<InvocationResponse>;
    fn register_rpc_listener(&self, subscriber: &WasccEntity) -> Result<()>;
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
}

impl Supervised for MessageBus {}

impl SystemService for MessageBus {
    fn service_started(&mut self, ctx: &mut Context<Self>) {
        info!("Message Bus started");
        // TODO: make this value configurable
        ctx.set_mailbox_capacity(1000);
    }
}

impl Actor for MessageBus {
    type Context = Context<Self>;
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
            if let Err(e) = lp.advertise_binding(
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
            let req = generate_binding_invocation(t, &msg, self.key.as_ref().unwrap(), target);
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
                    error!("Attempt to advertise a binding with no local subscribers and no lattice provider - binding ignored");
                    Err("Cannot advertise a binding with no local subscribers and no lattice provider. Binding ignored".into())
                } else {
                    Ok(())
                }
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
            addr: ctx.address().recipient().clone(),
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
        println!("handling invocation");
        match self.subscribers.get(&msg.target) {
            Some(target) => Box::pin(target.send(msg.clone()).into_actor(self).map(
                move |res, act, _ctx| {
                    println!("{:?}", res);
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
                    let res = do_rpc(l, &msg);
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

    fn handle(&mut self, msg: Subscribe, _ctx: &mut Context<Self>) {
        info!("Bus registered interest for {}", &msg.interest.url());
        self.subscribers.insert(msg.interest, msg.subscriber);
    }
}

fn do_rpc(l: &Box<dyn LatticeProvider>, inv: &Invocation) -> InvocationResponse {
    match l.rpc(&inv) {
        Ok(ir) => ir,
        Err(e) => InvocationResponse::error(&inv, &format!("RPC failure: {}", e)),
    }
}

fn generate_binding_invocation(
    t: &Recipient<Invocation>,
    msg: &AdvertiseBinding,
    key: &KeyPair,
    target: WasccEntity,
) -> RecipientRequest<Invocation> {
    let config = crate::generated::core::CapabilityConfiguration {
        module: msg.actor.to_string(),
        values: msg.values.clone(),
    };
    let inv = Invocation::new(
        key,
        WasccEntity::Actor(SYSTEM_ACTOR.to_string()),
        target,
        OP_BIND_ACTOR,
        crate::generated::core::serialize(&config).unwrap(),
    );

    t.send(inv)
}

#[cfg(test)]
mod test {
    use crate::dispatch::{Dispatcher, Invocation, InvocationResponse, WasccEntity};
    use crate::messagebus::{MessageBus, SetProvider, Subscribe};
    use crate::Result;
    use crate::{BusDispatcher, LatticeProvider};
    use actix::prelude::*;
    use std::collections::hash_map::RandomState;
    use std::collections::HashMap;
    use wascap::jwt::Claims;
    use wascap::prelude::KeyPair;

    // This test demonstrates the basic role of the message bus -- to act as an intermediary
    // between targets and senders. Any actor in the application can simply send
    // the bus an invocation with a proper target, and the bus will deliver the invocation to that
    // target
    #[actix_rt::test]
    async fn bus_supports_actor_to_actor() {
        let hk = KeyPair::new_server();
        let b = MessageBus::from_registry();
        let h1 = SyncArbiter::start(1, || HappyActor { inv_count: 0 });
        let h2 = SyncArbiter::start(1, || HappyActor { inv_count: 0 });
        let a1 = WasccEntity::Actor("Mxxx1".to_string());
        let a2 = WasccEntity::Actor("Mxxx2".to_string());

        let recip1 = h1.clone().recipient();
        let recip2 = h2.clone().recipient();
        b.send(Subscribe {
            interest: a1.clone(),
            subscriber: recip1,
        })
        .await
        .unwrap();
        b.send(Subscribe {
            interest: a2.clone(),
            subscriber: recip2,
        })
        .await
        .unwrap();
        println!("Subscribed...");
        let inv1 = Invocation::new(&hk, a1.clone(), a2.clone(), "OP_FOO", vec![]);
        let ir1 = b.send(inv1).await.unwrap(); // Actor a1 calls a2
        println!("A called A2");
        assert!(ir1.error.is_none());
        let inv2 = Invocation::new(&hk, a2.clone(), a1.clone(), "OP_FOO", vec![]);
        let ir2 = b.send(inv2).await.unwrap(); // Actor a2 calls a1
        println!("A2 called A");
        assert!(ir2.error.is_none());

        let val1 = h1.send(Query).await.unwrap();
        let val2 = h2.send(Query).await.unwrap();
        assert_eq!(val1, 1);
        assert_eq!(val2, 1);
    }

    #[actix_rt::test]
    async fn bus_defers_to_lattice_for_missing_subscriber() {
        let hk = KeyPair::new_server();
        let b = MessageBus::from_registry();
        let a1 = WasccEntity::Actor("Mxxx1".to_string());
        let a2 = WasccEntity::Actor("Mxxx2".to_string());

        b.send(SetProvider {
            provider: Box::new(FauxLattice {
                result: vec![1, 2, 3, 4, 5],
            }),
        })
        .await
        .unwrap();

        // The answer to this invocation should come from the lattice
        let inv1 = Invocation::new(&hk, a1.clone(), a2.clone(), "OP_FOO", vec![]);
        let inv1_id = inv1.id.to_string();
        let ir1 = b.send(inv1).await.unwrap(); // Actor a1 calls a2.. but a2 isn't local!

        assert!(ir1.error.is_none());
        assert_eq!(ir1.msg, vec![1, 2, 3, 4, 5]);
        assert_eq!(ir1.invocation_id, inv1_id);
    }

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

        fn init(&mut self, dispatcher: BusDispatcher) {
            unimplemented!()
        }

        fn register_rpc_listener(&self, subscriber: &WasccEntity) -> Result<()> {
            unimplemented!()
        }

        fn advertise_binding(
            &self,
            actor: &str,
            contract_id: &str,
            binding_name: &str,
            provider_id: &str,
            values: HashMap<String, String, RandomState>,
        ) -> Result<()> {
            unimplemented!()
        }

        fn advertise_claims(&self, claims: Claims<Actor>) -> Result<()> {
            unimplemented!()
        }
    }
}
