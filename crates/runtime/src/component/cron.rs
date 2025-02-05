use crate::capability::wrpc;
use core::ops::Deref;

use tracing::{instrument, Span};

use crate::component::{Handler, Instance};
impl<H, C> wrpc::exports::wasmcloud::cron::scheduler::Handler<C> for Instance<H, C>
where
    H: Handler,
    C: Send + Deref<Target = Span>,
{
    #[instrument(level = "debug", skip_all)]
    async  fn timed_invoke(&self,cx:C,payload: wit_bindgen_wrpc::bytes::Bytes,) -> wit_bindgen_wrpc::anyhow::Result<::core::result::Result<(),String>> {
        //TODO
    }
}
