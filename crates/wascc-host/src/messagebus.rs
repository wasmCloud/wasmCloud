use crate::capability::binding_cache::BindingCache;
use crate::dispatch::{Dispatcher, Invocation, InvocationResponse, WasccEntity};
use crate::Result;
use actix::prelude::*;
use futures::executor::block_on;
use std::collections::HashMap;

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
    pub contract_id: String,
    // Capability ID
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

pub trait LatticeProvider: Sync + Send {
    fn name(&self) -> String;
    fn rpc(&self, inv: &Invocation) -> Result<InvocationResponse>;
    fn register_interest(&self, subscriber: &WasccEntity, dispatcher: Dispatcher) -> Result<()>;
}

#[derive(Default)]
pub(crate) struct MessageBus {
    pub provider: Option<Box<dyn LatticeProvider>>,
    subscribers: HashMap<WasccEntity, Recipient<Invocation>>,
    binding_cache: BindingCache,
}

impl Supervised for MessageBus {}

impl SystemService for MessageBus {
    fn service_started(&mut self, ctx: &mut Context<Self>) {
        info!("Message Bus started");
    }
}

impl Actor for MessageBus {
    type Context = Context<Self>;
}

impl Handler<AdvertiseBinding> for MessageBus {
    type Result = Result<()>;

    fn handle(&mut self, msg: AdvertiseBinding, _ctx: &mut Context<Self>) -> Result<()> {
        self.binding_cache.add_binding(
            &msg.actor,
            &msg.contract_id,
            &msg.binding_name,
            &msg.provider_id,
            msg.values,
        );

        // TODO: where / when do we do the configure actor invocation on the provider?
        // -- I think this should be done in the subscription handler for the "advertise binding"
        // -- which should be idempotent
        // if there is a provider registered locally, we should invoke that directly

        // TODO: if there's a lattice provider, tell that provider to advertise said binding
        Ok(())
    }
}

impl Handler<SetProvider> for MessageBus {
    type Result = ();

    fn handle(&mut self, msg: SetProvider, _ctx: &mut Context<Self>) {
        self.provider = Some(msg.provider);
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
        match self.subscribers.get(&msg.target) {
            Some(target) => Box::pin(target.send(msg.clone()).into_actor(self).map(
                move |res, act, _ctx| {
                    if let Ok(r) = res {
                        r
                    } else {
                        InvocationResponse::error(
                            &msg,
                            "Mailbox error attempting to perform invocation",
                        )
                    }
                },
            )),
            None => {
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

impl Handler<Subscribe> for MessageBus {
    type Result = ();

    fn handle(&mut self, msg: Subscribe, ctx: &mut Context<Self>) {
        info!("Bus registered interest for {}", &msg.interest.url());
        self.subscribers
            .insert(msg.interest.clone(), msg.subscriber.clone());
        if let Some(ref l) = self.provider {
            let dispatcher = Dispatcher {
                addr: ctx.address().recipient().clone(),
            };
            if let Err(e) = l.register_interest(&msg.interest, dispatcher) {
                error!(
                    "Failed to register subscriber interest with lattice provider: {}",
                    e
                );
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

fn do_rpc(l: &Box<dyn LatticeProvider>, inv: &Invocation) -> InvocationResponse {
    match l.rpc(&inv) {
        Ok(ir) => ir,
        Err(e) => InvocationResponse::error(&inv, &format!("RPC failure: {}", e)),
    }
}

#[cfg(test)]
mod test {
    use crate::dispatch::{Dispatcher, Invocation, InvocationResponse, WasccEntity};
    use crate::messagebus::{MessageBus, SetProvider, Subscribe};
    use crate::LatticeProvider;
    use crate::Result;
    use actix::prelude::*;
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

        fn register_interest(
            &self,
            subscriber: &WasccEntity,
            dispatcher: Dispatcher,
        ) -> Result<()> {
            Ok(())
        }
    }
}
