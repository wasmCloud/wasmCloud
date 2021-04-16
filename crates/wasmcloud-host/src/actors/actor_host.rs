use std::collections::{hash_map::Entry, HashMap};

use crate::{actors::WasmCloudActor, capability::native_host::GetIdentity};
use crate::{
    capability::native_host::IdentityResponse,
    control_interface::ctlactor::{ControlInterface, PublishEvent},
};

use crate::dispatch::OP_HALT;
use crate::dispatch::{Invocation, InvocationResponse, WasmCloudEntity};
use crate::hlreg::HostLocalSystemService;
use crate::host_controller::{HostController, PutOciReference};
use crate::messagebus::{AdvertiseClaims, MessageBus, Subscribe};
use crate::middleware::{run_actor_post_invoke, run_actor_pre_invoke, Middleware};
use crate::{ControlEvent, Result};
use actix::prelude::*;
use futures::executor::block_on;
use log::info;
use wapc::WapcHost;
use wascap::jwt::TokenValidation;
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
    seed: String,
    can_update: bool,
    strict_update_check: bool,
}

#[derive(Message)]
#[rtype(result = "Result<()>")]
pub(crate) struct Initialize {
    pub actor_bytes: Vec<u8>,
    //pub wasi: Option<WasiParams>, Disabling WASI support in actors for now
    pub mw_chain: Vec<Box<dyn Middleware>>,
    pub signing_seed: String,
    pub image_ref: Option<String>,
    pub host_id: String,
    pub can_update: bool,
    pub strict_update_check: bool,
}

#[derive(Message)]
#[rtype(result = "Result<()>")]
pub(crate) struct LiveUpdate {
    pub actor_bytes: Vec<u8>,
    pub image_ref: Option<String>,
}

impl Handler<GetIdentity> for ActorHost {
    type Result = IdentityResponse;

    fn handle(&mut self, _msg: GetIdentity, _ctx: &mut Self::Context) -> Self::Result {
        let state = self.state.as_ref().unwrap();

        IdentityResponse {
            image_ref: state.image_ref.clone(),
            name: state.claims.name(),
            revision: state
                .claims
                .metadata
                .as_ref()
                .map(|md| md.rev.unwrap_or(0))
                .unwrap_or(0),
        }
    }
}

impl Handler<LiveUpdate> for ActorHost {
    type Result = Result<()>;

    fn handle(&mut self, msg: LiveUpdate, ctx: &mut Self::Context) -> Self::Result {
        if self.state.is_none() {
            return Err("Attempted to live update an actor with no existing state".into());
        }
        if !self.state.as_ref().unwrap().can_update {
            error!(
                "Rejecting attempt to update actor ({}) - live updates disabled",
                msg.image_ref
                    .unwrap_or_else(|| "No OCI Ref Supplied".into())
            );
            return Err("Attempt to live update actor denied. Runtime updates for this actor are not enabled".into());
        }

        let actor = WasmCloudActor::from_slice(&msg.actor_bytes)?;
        let public_key = actor.public_key();
        let new_claims = actor.claims();
        // Validate that this update is one that we will allow to take place
        validate_update(
            &new_claims,
            &self.state.as_ref().unwrap().claims,
            self.state.as_ref().unwrap().strict_update_check,
        )?;
        let old_revision = self
            .state
            .as_ref()
            .unwrap()
            .claims
            .metadata
            .as_ref()
            .unwrap_or(&wascap::jwt::Actor::default())
            .rev
            .unwrap_or(0) as u32;
        let new_revision = new_claims
            .metadata
            .as_ref()
            .unwrap_or(&wascap::jwt::Actor::default())
            .rev
            .unwrap_or(0) as u32;
        let pe = PublishEvent {
            event: ControlEvent::ActorUpdateBegan {
                actor: actor.public_key(),
                old_revision,
                new_revision,
            },
        };
        ControlInterface::from_hostlocal_registry(&self.state.as_ref().unwrap().host_id)
            .do_send(pe);
        MessageBus::from_hostlocal_registry(&self.state.as_ref().unwrap().host_id)
            .do_send(AdvertiseClaims { claims: new_claims });

        // Essentially re-starting the actor with a new set of bytes
        let init = Initialize {
            actor_bytes: msg.actor_bytes,
            mw_chain: self.state.as_ref().unwrap().mw_chain.clone(),
            signing_seed: self.state.as_ref().unwrap().seed.clone(),
            image_ref: msg.image_ref.clone(),
            host_id: self.state.as_ref().unwrap().host_id.to_string(),
            can_update: true,
            strict_update_check: true,
        };
        let host_id = init.host_id.to_string();
        let actor = perform_initialization(self, ctx, init);
        match actor {
            Ok(a) => {
                if let Some(oci_ref) = msg.image_ref {
                    HostController::from_hostlocal_registry(&self.state.as_ref().unwrap().host_id)
                        .do_send(PutOciReference {
                            oci_ref,
                            public_key,
                        });
                }
                ControlInterface::from_hostlocal_registry(&host_id).do_send(PublishEvent {
                    event: ControlEvent::ActorUpdateCompleted {
                        actor: a,
                        old_revision,
                        new_revision,
                    },
                });
                Ok(())
            }
            Err(e) => Err(e),
        }
    }
}

impl Handler<Initialize> for ActorHost {
    type Result = Result<()>;

    fn handle(&mut self, msg: Initialize, ctx: &mut Self::Context) -> Self::Result {
        let image_ref = msg.image_ref.clone();
        let actor = perform_initialization(self, ctx, msg);
        match actor {
            Ok(a) => {
                let pe = PublishEvent {
                    event: ControlEvent::ActorStarted {
                        actor: a,
                        image_ref,
                    },
                };
                let host_id = self.state.as_ref().unwrap().host_id.to_string();
                let _ = block_on(async move {
                    let cp = ControlInterface::from_hostlocal_registry(&host_id);
                    let _ = cp.send(pe).await;
                });
                Ok(())
            }
            Err(e) => Err(e),
        }
    }
}

fn perform_initialization(
    me: &mut ActorHost,
    ctx: &mut SyncContext<ActorHost>,
    msg: Initialize,
) -> Result<String> {
    let buf = msg.actor_bytes.clone();
    let actor = WasmCloudActor::from_slice(&buf)?;
    let c = actor.token.claims.clone();
    let jwt = actor.token.jwt.to_string();

    // Ensure that the JWT we found on this actor is valid, not expired, can be used,
    // has a verified signature, etc.
    let tv = wascap::jwt::validate_token::<wascap::jwt::Actor>(&jwt)?;
    assert_validation_result(&tv)?;

    #[cfg(feature = "wasmtime")]
    let engine = {
        info!("Initializing wasmtime engine");
        wasmtime_provider::WasmtimeEngineProvider::new(&buf, None)
    };
    #[cfg(feature = "wasm3")]
    let engine = wasm3_provider::Wasm3EngineProvider::new(&buf);

    let c2 = c.clone();
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
            let entity = WasmCloudEntity::Actor(c.subject.to_string());
            let b = MessageBus::from_hostlocal_registry(&msg.host_id);
            let b2 = b.clone();
            let recipient = ctx.address().recipient();
            let _ = block_on(async move {
                b.send(Subscribe {
                    interest: entity,
                    subscriber: recipient,
                })
                .await
            });
            if !advertise_claims(&c, &b2) {
                ctx.stop();
                return Err("Failed to advertise claims to message bus".into());
            }
            let hid = msg.host_id.to_string();

            me.state = Some(State {
                guest_module: g,
                claims: c.clone(),
                mw_chain: msg.mw_chain,
                image_ref: msg.image_ref,
                host_id: hid,
                seed: msg.signing_seed,
                can_update: msg.can_update,
                strict_update_check: msg.strict_update_check,
            });
            info!(
                "Actor {} initialized",
                &me.state.as_ref().unwrap().claims.subject
            );
            Ok(c.subject)
        }
        Err(_e) => {
            error!(
                "Failed to create a WebAssembly host for actor {}",
                actor.token.claims.subject
            );
            ctx.stop();
            Err("Failed to create a raw WebAssembly host".into())
        }
    }
}

fn advertise_claims(c: &Claims<wascap::jwt::Actor>, bus: &Addr<MessageBus>) -> bool {
    let pc = AdvertiseClaims { claims: c.clone() };
    block_on(async move {
        if let Err(e) = bus.send(pc).await {
            error!("Actor failed to advertise claims to bus: {}", e);
            false
        } else {
            true
        }
    })
}

fn assert_validation_result(tv: &TokenValidation) -> Result<()> {
    if tv.cannot_use_yet {
        error!(
            "Claims validation failure: Cannot be used {}",
            tv.not_before_human
        );
        Err("Actor claims cannot be used yet".into())
    } else if tv.expired {
        error!("Claims validation failure: Expired {}", tv.expires_human);
        Err("Actor claims have expired".into())
    } else if !tv.signature_valid {
        Err("Actor claims token has invalid signature".into())
    } else {
        Ok(())
    }
}

fn validate_update(
    new_claims: &Claims<wascap::jwt::Actor>,
    old_claims: &Claims<wascap::jwt::Actor>,
    strict_update_check: bool,
) -> Result<u32> {
    if let Some(ref new_md) = new_claims.metadata {
        if let Some(ref old_md) = old_claims.metadata {
            if new_claims.subject != old_claims.subject {
                error!(
                    "Rejecting attempt to replace actor {} with actor {} - PKs do not match",
                    old_claims.subject, new_claims.subject
                );
                return Err(
                    "Public keys of old actor and new actor do not match. Update denied.".into(),
                );
            }
            if new_md.rev.unwrap_or(0) <= old_md.rev.unwrap_or(0) {
                return Err(
                    "Cannot live update if the new module is not a higher revision number".into(),
                );
            }
            // False if old and new capabilities are the same list by comparing length and contents
            let claims_changed = !vecs_equal_anyorder(
                old_md.caps.as_ref().unwrap_or(&vec![]),
                new_md.caps.as_ref().unwrap_or(&vec![]),
            );

            if claims_changed && strict_update_check {
                return Err("Strict claims checking does not allow live updated actors to have different capability claims".into());
            } else if claims_changed {
                warn!("Live update warning: new actor has different capability claims than the previous revision.");
            }
        }
    }
    Ok(new_claims
        .metadata
        .as_ref()
        .unwrap_or(&wascap::jwt::Actor::default())
        .rev
        .unwrap_or(0) as u32)
}

fn vecs_equal_anyorder(i1: &[String], i2: &[String]) -> bool {
    fn get_lookup<T: Eq + std::hash::Hash>(iter: impl IntoIterator<Item = T>) -> HashMap<T, usize> {
        let mut lookup = HashMap::<T, usize>::new();
        for value in iter {
            match lookup.entry(value) {
                Entry::Occupied(entry) => {
                    *entry.into_mut() += 1;
                }
                Entry::Vacant(entry) => {
                    entry.insert(0);
                }
            }
        }
        lookup
    }
    get_lookup(i1) == get_lookup(i2)
}

impl Actor for ActorHost {
    type Context = SyncContext<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        info!("Actor started");
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        // NOTE: do not attempt to log asynchronously in a stopped function,
        // resources (including stdout) may not be available
    }
}

impl Handler<Invocation> for ActorHost {
    type Result = InvocationResponse;

    /// Receives an invocation from any source. This will execute the full pre-exec
    /// middleware chain, perform the requested operation, and then perform the full
    /// post-exec middleware chain, assuming no errors indicate a pre-emptive halt
    fn handle(&mut self, msg: Invocation, ctx: &mut Self::Context) -> Self::Result {
        let state = self.state.as_ref().unwrap();
        if msg.origin == msg.target && msg.operation == OP_HALT {
            info!(
                "Received explicit halt instruction. Actor {} shutting down",
                state.claims.subject
            );
            ctx.stop();
            return InvocationResponse::success(&msg, vec![]);
        }

        trace!(
            "Actor Invocation - From {} to {}: {}",
            msg.origin.url(),
            msg.target.url(),
            msg.operation
        );

        if let WasmCloudEntity::Actor(_) = msg.target {
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
                        Ok(r) => r,
                        Err(e) => InvocationResponse::error(
                            &msg,
                            &format!("Post-invoke middleware execution failure on actor: {}", e),
                        ),
                    }
                }
                Err(e) => {
                    error!("Error invoking actor: {} (from {})", e, msg.target_url());
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
