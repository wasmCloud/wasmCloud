use async_trait::async_trait;

use wasmbus_rpc::actor::prelude::*;
use wasmcloud_interface_messaging::{
    MessageSubscriber, MessageSubscriberReceiver, Messaging, MessagingSender, PubMessage,
    SubMessage,
};

/// An example actor that uses the wasmcloud Smithy-based toolchain
/// to interact with the wasmcloud lattice, responding to messages
/// coming in over the messaging interface
#[derive(Debug, Default, Actor, HealthResponder)]
#[services(Actor, MessageSubscriber)]
struct SmithyMessagingReceiverActor {}

/// Implementation of message handler
#[async_trait]
impl MessageSubscriber for SmithyMessagingReceiverActor {
    async fn handle_message(&self, ctx: &Context, msg: &SubMessage) -> RpcResult<()> {
        // If a reply-to is set, echo back the contents
        if let Some(reply_to) = &msg.reply_to {
            let _ = MessagingSender::new()
                .publish(
                    ctx,
                    &PubMessage {
                        subject: reply_to.clone(),
                        reply_to: None,
                        body: msg.body.clone(),
                    },
                )
                .await;
        }

        Ok(())
    }
}
