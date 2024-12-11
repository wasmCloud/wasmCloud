use core::ops::Deref;

use anyhow::Context as _;
use tracing::{info_span, instrument, warn, Instrument as _, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

use crate::capability::messaging0_2_0::types;
use crate::capability::wrpc;
use crate::component::{new_store, Handler, Instance, WrpcServeEvent};

pub mod v0_2;
pub mod v0_3;

impl<H, C> wrpc::exports::wasmcloud::messaging0_2_0::handler::Handler<C> for Instance<H, C>
where
    H: Handler,
    C: Send + Deref<Target = Span>,
{
    #[instrument(level = "debug", skip_all)]
    async fn handle_message(
        &self,
        cx: C,
        msg: wrpc::wasmcloud::messaging0_2_0::types::BrokerMessage,
    ) -> anyhow::Result<Result<(), String>> {
        // Set the parent of the current context to the span passed in
        Span::current().set_parent(cx.deref().context());
        let mut store = new_store(&self.engine, self.handler.clone(), self.max_execution_time);
        let call_handle_message = info_span!("call_handle_message");
        store.data_mut().parent_context = Some(call_handle_message.context());

        let res = if let Ok(pre) = v0_3::bindings::MessagingHandlerPre::new(self.pre.clone()) {
            let bindings = pre.instantiate_async(&mut store).await?;
            let msg = store
                .data_mut()
                .table
                .push(v0_3::Message::Wrpc(msg))
                .context("failed to push message to table")?;
            bindings
                .wasmcloud_messaging0_3_0_incoming_handler()
                .call_handle(&mut store, msg)
                .instrument(call_handle_message)
                .await
                .context("failed to call `wasmcloud:messaging/incoming-handler@0.3.0#handle`")
                .map(|err| err.map_err(|err| err.to_string()))
        } else {
            let pre = v0_2::bindings::MessagingHandlerOhTwoPre::new(self.pre.clone())
                .context("failed to pre-instantiate `wasmcloud:messaging/handler`")?;
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
                .instrument(call_handle_message)
                .await
                .context("failed to call `wasmcloud:messaging/handler@0.2.0#handle-message`")
        };
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
