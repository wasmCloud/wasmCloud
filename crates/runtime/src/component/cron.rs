use crate::capability::wrpc;
use core::ops::Deref;

use anyhow::Context;
use tracing::{debug_span, instrument, warn, Instrument, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

use crate::component::{new_store, Handler, Instance};

use super::WrpcServeEvent;

pub mod bindings {
    wasmtime::component::bindgen!({
        world: "scheduler",
        async: true,
    });
}

impl<H, C> wrpc::exports::wasmcloud::cron::scheduler::Handler<C> for Instance<H, C>
where
    H: Handler,
    C: Send + Deref<Target = Span>,
{
    #[instrument(level = "debug", skip_all)]
    async fn timed_invoke(
        &self,
        cx: C,
        payload: bytes::Bytes,
    ) -> anyhow::Result<Result<(), String>> {
        Span::current().set_parent(cx.deref().context());

        let mut store = new_store(&self.engine, self.handler.clone(), self.max_execution_time);
        let pre = bindings::SchedulerPre::new(self.pre.clone())
            .context("failed to pre-instantiate `wasmcloud:cron/scheduler`")?;
        let bindings = pre
            .instantiate_async(&mut store)
            .instrument(debug_span!("instantiate_async"))
            .await
            .context("failed to instantiate `wasmcloud:cron/scheduler`")?;
        let res = bindings
            .wasmcloud_cron_scheduler()
            .call_timed_invoke(store, &payload)
            .instrument(debug_span!("call_timed_invoke"))
            .await
            .context("failed to call `wasmcloud:cron/scheduler.timed-invoke`")
            .map(|err| err.map_err(|err| err.to_string()));
        let success = res.is_ok();
        if let Err(err) = self
            .events
            .try_send(WrpcServeEvent::CronInvocationReturned {
                context: cx,
                success,
            })
        {
            warn!(
                ?err,
                success, "failed to send `wasmcloud:cron/scheduler.timed-invoke` return event"
            );
        }
        res
    }
}
