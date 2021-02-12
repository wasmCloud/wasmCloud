use actix::prelude::*;
use futures::StreamExt;

#[derive(Message)]
#[rtype(result = "()")]
pub(crate) struct Initialize {
    pub nc: nats::asynk::Connection,
    pub subject: String,
    pub queue: Option<String>,
    pub receiver: Recipient<NatsMessage>,
}

#[derive(Message, Clone)]
#[rtype(result = "()")]
pub(crate) struct NatsMessage {
    pub msg: nats::asynk::Message,
}

#[derive(Default)]
pub(crate) struct NatsSubscriber {
    state: Option<SubscriberState>,
}

struct SubscriberState {
    receiver: Recipient<NatsMessage>,
}

impl Actor for NatsSubscriber {
    type Context = Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        trace!("NATS Subscriber started");
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {}
}

impl Handler<Initialize> for NatsSubscriber {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: Initialize, _ctx: &mut Self::Context) -> Self::Result {
        let state = SubscriberState {
            receiver: msg.receiver,
        };
        self.state = Some(state);
        let nc = msg.nc;
        let subject = msg.subject;
        let queue = msg.queue;
        Box::pin(
            async move {
                if let Some(q) = queue {
                    nc.queue_subscribe(&subject, &q).await
                } else {
                    nc.subscribe(&subject).await
                }
            }
            .into_actor(self)
            .map(|sub, _act, ctx| {
                if let Ok(sub) = sub {
                    ctx.add_message_stream(sub.map(|m| NatsMessage { msg: m }))
                }
            }),
        )
    }
}

impl Handler<NatsMessage> for NatsSubscriber {
    type Result = ResponseActFuture<Self, ()>;

    fn handle(&mut self, msg: NatsMessage, _ctx: &mut Self::Context) -> Self::Result {
        trace!("NATS subscriber forwarding message");
        let target = self.state.as_ref().unwrap().receiver.clone();
        Box::pin(
            async move {
                if target.send(msg).await.is_err() {
                    error!("Target failed to process NATS subscription message.");
                }
            }
            .into_actor(self),
        )
    }
}
