use crate::dispatch::{Invocation, WasccEntity};
use crate::Result;
use actix::prelude::*;
use std::collections::HashMap;

#[derive(Message)]
#[rtype(result = "()")]
pub struct SetProvider {
    pub provider: Box<dyn MessageBusProvider>,
}

#[derive(Message)]
#[rtype(result = "()")]
pub struct Subscribe {
    pub interest: WasccEntity,
    pub subscriber: Recipient<Invocation>,
}

pub trait MessageBusProvider: Sync + Send {
    fn name(&self) -> String;
    fn init(&self) -> Result<()>;
}

#[derive(Default)]
pub(crate) struct MessageBus {
    pub provider: Option<Box<dyn MessageBusProvider>>,
    subscribers: HashMap<WasccEntity, Recipient<Invocation>>,
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

impl Handler<Subscribe> for MessageBus {
    type Result = ();

    fn handle(&mut self, msg: Subscribe, _ctx: &mut Context<Self>) {
        info!("Bus registered interest for {}", &msg.interest.url());
        self.subscribers.insert(msg.interest, msg.subscriber);
    }
}
