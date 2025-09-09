use core::future::Future;

use anyhow::Context as _;
use tracing::{info_span, instrument};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use wasmtime::Store;

use crate::capability::messaging0_2_0::{consumer, types};
use crate::capability::wrpc;
use crate::component::{Ctx, Handler};

pub mod bindings {
    wasmtime::component::bindgen!({
        world: "messaging-handler-oh-two",
        async: true,
        with: {
           "wasmcloud:messaging/types": crate::capability::messaging0_2_0::types,
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
        Messaging::request(&self.handler, subject, body, timeout_ms).await
    }

    #[instrument(level = "debug", skip_all)]
    async fn publish(&mut self, msg: types::BrokerMessage) -> anyhow::Result<Result<(), String>> {
        self.attach_parent_context();
        self.handler.publish(msg).await
    }
}

#[instrument(level = "debug", skip_all)]
pub(crate) async fn handle_message<H>(
    pre: bindings::MessagingHandlerOhTwoPre<Ctx<H>>,
    mut store: &mut Store<Ctx<H>>,
    msg: wrpc::wasmcloud::messaging0_2_0::types::BrokerMessage,
) -> anyhow::Result<Result<(), String>>
where
    H: Handler,
{
    let call_handle_message = info_span!("call_handle_message");
    store.data_mut().parent_context = Some(call_handle_message.context());
    let bindings = pre.instantiate_async(&mut store).await?;
    bindings
        .wasmcloud_messaging0_2_0_handler()
        .call_handle_message(
            &mut store,
            &types::BrokerMessage {
                subject: msg.subject,
                body: msg.body.into(),
                reply_to: msg.reply_to,
            },
        )
        .await
        .context("failed to call `wasmcloud:messaging/handler@0.2.0#handle-message`")
}
