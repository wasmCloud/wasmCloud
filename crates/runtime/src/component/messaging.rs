use super::{new_store, Ctx, Handler, Instance, WrpcServeEvent};

use crate::capability::messaging::{consumer, types};

use anyhow::Context as _;
use async_trait::async_trait;
use tracing::{instrument, warn};

pub mod wasmtime_handler_bindings {
    wasmtime::component::bindgen!({
        world: "messaging-handler",
        async: true,
        with: {
           "wasmcloud:messaging/types": crate::capability::messaging::types,
        },
    });
}

pub mod wrpc_handler_bindings {
    wit_bindgen_wrpc::generate!({
        world: "messaging-handler",
        with: {
           "wasmcloud:messaging/types@0.2.0": generate,
           "wasmcloud:messaging/handler@0.2.0": generate,
        }
    });
}

impl<H> types::Host for Ctx<H> where H: Handler {}

#[async_trait]
pub trait MessagingClient {
    async fn request(
        &self,
        subject: String,
        body: Vec<u8>,
        timeout_ms: u32,
    ) -> anyhow::Result<Result<types::BrokerMessage, String>>;

    async fn publish(&self, msg: types::BrokerMessage) -> anyhow::Result<Result<(), String>>;
}

#[async_trait]
impl<H> consumer::Host for Ctx<H>
where
    H: Handler,
{
    #[instrument]
    async fn request(
        &mut self,
        subject: String,
        body: Vec<u8>,
        timeout_ms: u32,
    ) -> anyhow::Result<Result<types::BrokerMessage, String>> {
        self.handler.request(subject, body, timeout_ms).await
    }

    #[instrument(skip(self))]
    async fn publish(&mut self, msg: types::BrokerMessage) -> anyhow::Result<Result<(), String>> {
        self.handler.publish(msg).await
    }
}

impl<H, C> wrpc_handler_bindings::exports::wasmcloud::messaging::handler::Handler<C>
    for Instance<H, C>
where
    H: Handler,
    C: Send,
{
    #[instrument(level = "debug", skip_all)]
    async fn handle_message(
        &self,
        cx: C,
        wrpc_handler_bindings::wasmcloud::messaging::types::BrokerMessage {
            subject,
            body,
            reply_to,
        }: wrpc_handler_bindings::wasmcloud::messaging::types::BrokerMessage,
    ) -> anyhow::Result<Result<(), String>> {
        let mut store = new_store(&self.engine, self.handler.clone(), self.max_execution_time);
        let pre = wasmtime_handler_bindings::MessagingHandlerPre::new(self.pre.clone())
            .context("failed to pre-instantiate `wasmcloud:messaging/handler`")?;
        let bindings = pre.instantiate_async(&mut store).await?;
        let res = bindings
            .wasmcloud_messaging_handler()
            .call_handle_message(
                &mut store,
                &types::BrokerMessage {
                    subject,
                    body: body.into(),
                    reply_to,
                },
            )
            .await
            .context("failed to call `wasmcloud:messaging/handler.handle-message`");
        let success = res.is_ok();
        if let Err(err) =
            self.events
                .try_send(WrpcServeEvent::MessagingHandlerHandleMessageReturned {
                    context: cx,
                    success,
                })
        {
            warn!(
                ?err,
                success, "failed to send `wasmcloud:messaging/handler.handle-message` return event"
            );
        }
        res
    }
}
