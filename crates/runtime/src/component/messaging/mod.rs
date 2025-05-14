use core::ops::Deref;

use anyhow::Context as _;
use tracing::{instrument, warn, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

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
        let mut store = new_store(
            &self.engine,
            self.handler.clone(),
            self.max_execution_time,
            Some(self.max_memory_limit),
        );

        // If wasmcloud:messaging@0.3.0 is enabled and we can instantiate the 0.3.0 bindings,
        // handle the message using 0.3.0. Otherwise, use the 0.2.0 bindings.
        let res = if self.experimental_features.wasmcloud_messaging_v3 {
            if let Ok(pre) = v0_3::bindings::MessagingHandlerPre::new(self.pre.clone()) {
                v0_3::handle_message(pre, &mut store, msg).await
            } else {
                let pre = v0_2::bindings::MessagingHandlerOhTwoPre::new(self.pre.clone())
                    .context("failed to pre-instantiate `wasmcloud:messaging/handler`")?;
                v0_2::handle_message(pre, &mut store, msg).await
            }
        } else {
            let pre = v0_2::bindings::MessagingHandlerOhTwoPre::new(self.pre.clone())
                .context("failed to pre-instantiate `wasmcloud:messaging/handler`")?;
            v0_2::handle_message(pre, &mut store, msg).await
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
