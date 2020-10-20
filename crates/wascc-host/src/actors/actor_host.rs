use crate::actors::WasccActor;
use crate::dispatch::{Invocation, InvocationResponse, WasccEntity};
use crate::messagebus::{MessageBus, Subscribe};
use crate::middleware::{run_actor_post_invoke, run_actor_pre_invoke, Middleware};
use crate::Result;
use actix::prelude::*;
use futures::executor::block_on;
use wapc::{WapcHost, WasiParams};
use wascap::prelude::{Claims, KeyPair};

pub(crate) struct ActorHost {
    guest_module: WapcHost,
    claims: Claims<wascap::jwt::Actor>,
    mw_chain: Vec<Box<dyn Middleware>>,
}

impl ActorHost {
    pub fn new(
        actor_bytes: Vec<u8>,
        wasi: Option<WasiParams>,
        mw_chain: Vec<Box<dyn Middleware>>,
        signing_seed: String,
    ) -> ActorHost {
        let buf = actor_bytes.clone();
        let actor = WasccActor::from_slice(&buf).unwrap();

        #[cfg(feature = "wasmtime")]
        let engine = wasmtime_provider::WasmtimeEngineProvider::new(&buf, wasi);
        #[cfg(feature = "wasm3")]
        let engine = wasm3_provider::Wasm3EngineProvider::new(&buf);

        let c = actor.token.claims.clone();

        let guest = WapcHost::new(Box::new(engine), move |_id, bd, ns, op, payload| {
            crate::dispatch::wapc_host_callback(
                KeyPair::from_seed(&signing_seed).unwrap(),
                actor.token.claims.clone(),
                bd,
                ns,
                op,
                payload,
            )
        });

        match guest {
            Ok(g) => ActorHost {
                guest_module: g,
                claims: c,
                mw_chain,
            },
            Err(e) => {
                error!("Failed to instantiate waPC host: {}", e);
                panic!();
            }
        }
    }
}

impl Actor for ActorHost {
    type Context = SyncContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!("Actor {} started", &self.claims.subject);
        println!("Actor {} started", &self.claims.subject);
        //TODO: make this value configurable
        let entity = WasccEntity::Actor(self.claims.subject.to_string());
        let b = MessageBus::from_registry();
        let _ = block_on(async move {
            b.send(Subscribe {
                interest: entity,
                subscriber: ctx.address().recipient(),
            })
            .await
        });
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        println!("Actor {} stopped", &self.claims.subject);
    }
}

impl Handler<Invocation> for ActorHost {
    type Result = InvocationResponse;

    /// Receives an invocation from any source. This will execute the full pre-exec
    /// middleware chain, perform the requested operation, and then perform the full
    /// post-exec middleware chain, assuming no errors indicate a pre-emptive halt
    fn handle(&mut self, msg: Invocation, ctx: &mut Self::Context) -> Self::Result {
        println!("Actor being invoked");
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
