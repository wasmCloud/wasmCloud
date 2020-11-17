use crate::actors::WasccActor;
use crate::control_plane::cpactor::{ControlPlane, PublishEvent};
use crate::control_plane::events::TerminationReason;
use crate::dispatch::{Invocation, InvocationResponse, WasccEntity};
use crate::hlreg::HostLocalSystemService;
use crate::messagebus::{MessageBus, PutClaims, Subscribe, Unsubscribe};
use crate::middleware::{run_actor_post_invoke, run_actor_pre_invoke, Middleware};
use crate::{ControlEvent, Result};
use actix::prelude::*;
use futures::executor::block_on;
use wapc::{WapcHost, WasiParams};
use wascap::prelude::{Claims, KeyPair};

#[derive(Default)]
pub(crate) struct ActorHost {
    state: Option<State>,
}

struct State {
    guest_module: WapcHost,
    claims: Claims<wascap::jwt::Actor>,
    mw_chain: Vec<Box<dyn Middleware>>,
    image_ref: Option<String>,
    host_id: String,
}

#[derive(Message)]
#[rtype(result = "Result<()>")]
pub(crate) struct Initialize {
    pub actor_bytes: Vec<u8>,
    pub wasi: Option<WasiParams>,
    pub mw_chain: Vec<Box<dyn Middleware>>,
    pub signing_seed: String,
    pub image_ref: Option<String>,
    pub host_id: String,
}

impl Handler<Initialize> for ActorHost {
    type Result = Result<()>;

    fn handle(&mut self, msg: Initialize, ctx: &mut Self::Context) -> Self::Result {
        let buf = msg.actor_bytes.clone();
        let actor = WasccActor::from_slice(&buf)?;

        #[cfg(feature = "wasmtime")]
        let engine = wasmtime_provider::WasmtimeEngineProvider::new(&buf, wasi);
        #[cfg(feature = "wasm3")]
        let engine = wasm3_provider::Wasm3EngineProvider::new(&buf);

        let c = actor.token.claims.clone();
        let c2 = c.clone();
        let c3 = c.clone(); // TODO: I can't believe I have to do this to make the [censored] borrow checker happy
        let seed = msg.signing_seed.to_string();

        let guest = WapcHost::new(Box::new(engine), move |_id, bd, ns, op, payload| {
            crate::dispatch::wapc_host_callback(
                KeyPair::from_seed(&seed).unwrap(),
                c2.clone(),
                bd,
                ns,
                op,
                payload,
            )
        });

        match guest {
            Ok(g) => {
                let c = c3.clone();
                let entity = WasccEntity::Actor(c.subject.to_string());
                let b = MessageBus::from_hostlocal_registry(&msg.host_id);
                let b2 = b.clone();
                let recipient = ctx.address().clone().recipient();
                let _ = block_on(async move {
                    b.send(Subscribe {
                        interest: entity,
                        subscriber: recipient,
                    })
                    .await
                });
                let pc = PutClaims { claims: c.clone() };
                let r = block_on(async move {
                    if let Err(e) = b2.send(pc).await {
                        error!("Actor failed to advertise claims to bus: {}", e);
                        ctx.stop();
                        Err("Failed to advertise bus claims".into())
                    } else {
                        Ok(())
                    }
                });
                if r.is_err() {
                    return r;
                }
                let pe = PublishEvent {
                    event: ControlEvent::ActorStarted {
                        actor: c.subject.to_string(),
                        image_ref: msg.image_ref.clone(),
                    },
                };
                let host_id = msg.host_id.to_string();
                let hid = msg.host_id.to_string();
                let _ = block_on(async move {
                    let cp = ControlPlane::from_hostlocal_registry(&host_id);
                    cp.send(pe).await
                });
                self.state = Some(State {
                    guest_module: g,
                    claims: c.clone(),
                    mw_chain: msg.mw_chain,
                    image_ref: msg.image_ref,
                    host_id: hid,
                });
                Ok(())
            }
            Err(e) => {
                error!(
                    "Failed to create a WebAssembly host for actor {}",
                    actor.token.claims.subject
                );
                ctx.stop();
                Err("Failed to create a raw WebAssembly host".into())
            }
        }
    }
}

impl Actor for ActorHost {
    type Context = SyncContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        info!(
            "Actor {} started",
            &self.state.as_ref().unwrap().claims.subject
        );
    }

    fn stopped(&mut self, ctx: &mut Self::Context) {
        let state = self.state.as_ref().unwrap();
        info!("Actor {} stopped", &state.claims.subject);
        let _ = block_on(async move {
            let cp = ControlPlane::from_hostlocal_registry(&state.host_id);
            cp.send(PublishEvent {
                event: ControlEvent::ActorStopped {
                    actor: state.claims.subject.to_string(),
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
        let state = self.state.as_ref().unwrap();

        trace!(
            "Actor Invocation - From {} to {}: {}",
            msg.origin.url(),
            msg.target.url(),
            msg.operation
        );
        println!(
            "Actor Invocation - From {} to {}: {}",
            msg.origin.url(),
            msg.target.url(),
            msg.operation
        );

        if let WasccEntity::Actor(ref target) = msg.target {
            if run_actor_pre_invoke(&msg, &state.mw_chain).is_err() {
                return InvocationResponse::error(
                    &msg,
                    "Pre-invoke middleware execution failure on actor",
                );
            }
            match state.guest_module.call(&msg.operation, &msg.msg) {
                Ok(v) => {
                    let resp = InvocationResponse::success(&msg, v);
                    match run_actor_post_invoke(resp, &state.mw_chain) {
                        Ok(r) => {
                            println!("All good {:?}", r);
                            r
                        }
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
