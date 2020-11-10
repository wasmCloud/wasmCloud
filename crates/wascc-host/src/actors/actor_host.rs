use crate::actors::WasccActor;
use crate::control_plane::actorhost::{ControlPlane, PublishEvent};
use crate::control_plane::events::TerminationReason;
use crate::dispatch::{Invocation, InvocationResponse, WasccEntity};
use crate::messagebus::{MessageBus, PutClaims, Subscribe, Unsubscribe};
use crate::middleware::{run_actor_post_invoke, run_actor_pre_invoke, Middleware};
use crate::{ControlEvent, Result};
use actix::prelude::*;
use futures::executor::block_on;
use wapc::{WapcHost, WasiParams};
use wascap::prelude::{Claims, KeyPair};

pub(crate) struct ActorHost {
    guest_module: WapcHost,
    claims: Claims<wascap::jwt::Actor>,
    mw_chain: Vec<Box<dyn Middleware>>,
    image_ref: Option<String>,
}

impl ActorHost {
    pub fn new(
        actor_bytes: Vec<u8>,
        wasi: Option<WasiParams>,
        mw_chain: Vec<Box<dyn Middleware>>,
        signing_seed: String,
        image_ref: Option<String>,
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
                image_ref,
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

        let entity = WasccEntity::Actor(self.claims.subject.to_string());
        let b = MessageBus::from_registry();
        let b2 = b.clone();
        let recipient = ctx.address().clone().recipient();
        let _ = block_on(async move {
            b.send(Subscribe {
                interest: entity,
                subscriber: recipient,
            })
            .await
        });
        let c = self.claims.clone();
        let _ = block_on(async move {
            if let Err(e) = b2.send(PutClaims { claims: c }).await {
                error!("Actor failed to advertise claims to bus: {}", e);
                ctx.stop();
            }
        });
        let _ = block_on(async move {
            let cp = ControlPlane::from_registry();
            cp.send(PublishEvent {
                event: ControlEvent::ActorStarted {
                    header: Default::default(),
                    actor: self.claims.subject.to_string(),
                    image_ref: self.image_ref.clone(),
                },
            })
            .await
        });
    }

    fn stopped(&mut self, ctx: &mut Self::Context) {
        info!("Actor {} stopped", &self.claims.subject);
        let _ = block_on(async move {
            let cp = ControlPlane::from_registry();
            cp.send(PublishEvent {
                event: ControlEvent::ActorStopped {
                    header: Default::default(),
                    actor: self.claims.subject.to_string(),
                    reason: TerminationReason::Requested,
                },
            })
            .await
        });
    }
}

impl Handler<Invocation> for ActorHost {
    type Result = InvocationResponse;

    /// Receives an invocation from any source. This will execute the full pre-exec
    /// middleware chain, perform the requested operation, and then perform the full
    /// post-exec middleware chain, assuming no errors indicate a pre-emptive halt
    fn handle(&mut self, msg: Invocation, ctx: &mut Self::Context) -> Self::Result {
        trace!(
            "Actor Invocation - From {} to {}: {}",
            msg.origin.url(),
            msg.target.url(),
            msg.operation
        );

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
