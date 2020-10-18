use crate::actors::WasccActor;
use crate::dispatch::{Invocation, InvocationResponse, WasccEntity};
use crate::middleware::{run_actor_post_invoke, run_actor_pre_invoke, Middleware};
use crate::Result;
use actix::prelude::*;
use wapc::{WapcHost, WasiParams};
use wascap::prelude::{Claims, KeyPair};

pub(crate) struct ActorHost {
    guest_module: WapcHost,
    claims: Claims<wascap::jwt::Actor>,
    mw_chain: Vec<Box<dyn Middleware>>,
}

impl ActorHost {
    pub fn new(
        actor: WasccActor,
        wasi: Option<WasiParams>,
        mw_chain: Vec<Box<dyn Middleware>>,
        signing_seed: String,
    ) -> ActorHost {
        let buf = actor.bytes.clone();
        #[cfg(feature = "wasmtime")]
        let engine = wasmtime_provider::WasmtimeEngineProvider::new(&buf, wasi);
        #[cfg(feature = "wasm3")]
        let engine = wasm3_provider::Wasm3EngineProvider::new(&buf);

        let c = actor.token.claims.clone();
        let mut guest = WapcHost::new(Box::new(engine), move |_id, bd, ns, op, payload| {
            crate::dispatch::wapc_host_callback(
                KeyPair::from_seed(&signing_seed).unwrap(),
                actor.token.claims.clone(),
                bd,
                ns,
                op,
                payload,
            )
        })
        .unwrap();

        ActorHost {
            guest_module: guest,
            claims: c,
            mw_chain,
        }
    }
}

impl Actor for ActorHost {
    type Context = SyncContext<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        info!("Actor {} started", self.claims.subject);
    }
}

impl Handler<Invocation> for ActorHost {
    type Result = InvocationResponse;

    /// Receives an invocation from any source. This will execute the full pre-exec
    /// middleware chain, perform the requested operation, and then perform the full
    /// post-exec middleware chain, assuming no errors indicate a pre-emptive halt
    fn handle(&mut self, msg: Invocation, ctx: &mut Self::Context) -> Self::Result {
        if msg.validate_antiforgery().is_err() {
            return InvocationResponse::error(
                &msg,
                "Anti-forgery validation failed for invocation.",
            );
        }

        if let WasccEntity::Actor(ref target) = msg.target {
            if run_actor_pre_invoke(&msg, &self.mw_chain).is_err() {
                return InvocationResponse::error(
                    &msg,
                    "Pre-invoke middleware execution failure on actor",
                );
            }
            match self.guest_module.call(&msg.operation, &msg.msg) {
                Ok(v) => {
                    let resp = InvocationResponse::success(&msg, v);
                    match run_actor_post_invoke(resp, &self.mw_chain) {
                        Ok(r) => r,
                        Err(e) => InvocationResponse::error(
                            &msg,
                            &format!("Post-invoke middleware execution failure on actor: {}", e),
                        ),
                    }
                }
                Err(e) => {
                    InvocationResponse::error(&msg, &format!("Failed to invoke actor: {}", e))
                }
            }
        } else {
            InvocationResponse::error(
                &msg,
                "Actor received invocation that should have been delivered to a provider",
            )
        }
    }
}
