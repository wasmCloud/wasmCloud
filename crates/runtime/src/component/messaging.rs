use super::{new_store, Ctx, Handler, Instance, WrpcServeEvent};

use crate::capability::messaging::{consumer, types};
use crate::capability::wrpc;

use anyhow::Context as _;
use async_trait::async_trait;
use bytes::Bytes;
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
        match wrpc::wasmcloud::messaging::consumer::request(
            &self.handler,
            None,
            &subject,
            &Bytes::from(body),
            timeout_ms,
        )
        .await?
        {
            Ok(wrpc::wasmcloud::messaging::types::BrokerMessage {
                subject,
                body,
                reply_to,
            }) => Ok(Ok(types::BrokerMessage {
                subject,
                body: body.into(),
                reply_to,
            })),
            Err(err) => Ok(Err(err)),
        }
    }

    #[instrument(skip(self))]
    async fn publish(
        &mut self,
        types::BrokerMessage {
            subject,
            body,
            reply_to,
        }: types::BrokerMessage,
    ) -> anyhow::Result<Result<(), String>> {
        wrpc::wasmcloud::messaging::consumer::publish(
            &self.handler,
            None,
            &wrpc::wasmcloud::messaging::types::BrokerMessage {
                subject,
                body: body.into(),
                reply_to,
            },
        )
        .await
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
        let (bindings, _) =
            wasmtime_handler_bindings::MessagingHandler::instantiate_pre(&mut store, &self.pre)
                .await?;

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
                success, "failed to send `wasmcloud:messaging/handler.handle-message` event"
            );
        }
        res
    }
}
