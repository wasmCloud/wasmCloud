use crate::Result;
use actix::prelude::*;
use wapc::WapcHost;

pub(crate) struct ActorHost{
    //guest_module: WapcHost
}

impl Actor for ActorHost{
    type Context = SyncContext<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        info!("Actor started.");
    }
}