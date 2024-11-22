use core::future::Future;

use super::{new_store, Ctx, Handler, Instance, WrpcServeEvent};

use crate::capability::messaging::{consumer, types};
use crate::capability::wrpc;

use core::ops::Deref;

use anyhow::Context as _;
use async_trait::async_trait;
use tracing::{info_span, instrument, warn, Instrument as _, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;

pub mod wasmtime_handler_bindings {
    wasmtime::component::bindgen!({
        world: "messaging-handler",
        async: true,
        with: {
           "wasmcloud:messaging/types": crate::capability::messaging::types,
        },
    });
}

/// `wasmcloud:messaging` abstraction
pub trait Messaging {
    /// Handle `wasmcloud:messaging/request`
    fn request(
        &self,
        subject: String,
        body: Vec<u8>,
        timeout_ms: u32,
    ) -> impl Future<Output = anyhow::Result<Result<types::BrokerMessage, String>>> + Send;

    /// Handle `wasmcloud:messaging/publish`
    fn publish(
        &self,
        msg: types::BrokerMessage,
    ) -> impl Future<Output = anyhow::Result<Result<(), String>>> + Send;
}

impl<H> types::Host for Ctx<H> where H: Handler {}

#[async_trait]
impl<H> consumer::Host for Ctx<H>
where
    H: Handler,
{
    #[instrument(level = "debug", skip_all)]
    async fn request(
        &mut self,
        subject: String,
        body: Vec<u8>,
        timeout_ms: u32,
    ) -> anyhow::Result<Result<types::BrokerMessage, String>> {
        self.attach_parent_context();
        self.handler.request(subject, body, timeout_ms).await
    }

    #[instrument(level = "debug", skip_all)]
    async fn publish(&mut self, msg: types::BrokerMessage) -> anyhow::Result<Result<(), String>> {
        self.attach_parent_context();
        self.handler.publish(msg).await
    }
}

impl<H, C> wrpc::exports::wasmcloud::messaging::handler::Handler<C> for Instance<H, C>
where
    H: Handler,
    C: Send + Deref<Target = tracing::Span>,
{
    #[instrument(level = "debug", skip_all)]
    async fn handle_message(
        &self,
        cx: C,
        wrpc::wasmcloud::messaging::types::BrokerMessage {
            subject,
            body,
            reply_to,
        }: wrpc::wasmcloud::messaging::types::BrokerMessage,
    ) -> anyhow::Result<Result<(), String>> {
        // Set the parent of the current context to the span passed in
        Span::current().set_parent(cx.deref().context());
        let mut store = new_store(&self.engine, self.handler.clone(), self.max_execution_time);
        let pre = wasmtime_handler_bindings::MessagingHandlerPre::new(self.pre.clone())
            .context("failed to pre-instantiate `wasmcloud:messaging/handler`")?;
        let bindings = pre.instantiate_async(&mut store).await?;
        let call_handle_message = info_span!("call_handle_message");
        store.data_mut().parent_context = Some(call_handle_message.context());
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
            .instrument(call_handle_message)
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
