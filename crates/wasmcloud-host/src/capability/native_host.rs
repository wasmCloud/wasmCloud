use crate::capability::native::NativeCapability;
use crate::control_interface::ctlactor::{ControlInterface, PublishEvent};
use crate::control_interface::events::TerminationReason;
use crate::dispatch::{Invocation, InvocationResponse, ProviderDispatcher, WasccEntity};
use crate::hlreg::HostLocalSystemService;
use crate::messagebus::{EnforceLocalProviderLinks, MessageBus, Subscribe};
use crate::middleware::{run_capability_post_invoke, run_capability_pre_invoke, Middleware};
use crate::{ControlEvent, Result};
use crate::{Host, SYSTEM_ACTOR};
use actix::prelude::*;
use futures::executor::block_on;
use libloading::{Library, Symbol};
use std::env::temp_dir;
use std::fs::File;
use wascap::prelude::KeyPair;
use wascc_codec::capabilities::{
    CapabilityDescriptor, CapabilityProvider, OP_GET_CAPABILITY_DESCRIPTOR,
};

#[derive(Message)]
#[rtype(result = "Result<WasccEntity>")]
pub(crate) struct Initialize {
    pub cap: NativeCapability,
    pub mw_chain: Vec<Box<dyn Middleware>>,
    pub seed: String,
    pub image_ref: Option<String>,
}

struct State {
    cap: NativeCapability,
    mw_chain: Vec<Box<dyn Middleware>>,
    kp: KeyPair,
    library: Option<Library>,
    plugin: Box<dyn CapabilityProvider + 'static>,
    //descriptor: CapabilityDescriptor,
    image_ref: Option<String>,
}

pub(crate) struct NativeCapabilityHost {
    state: Option<State>,
}

impl NativeCapabilityHost {
    pub fn new() -> Self {
        NativeCapabilityHost { state: None }
    }
}

impl Actor for NativeCapabilityHost {
    type Context = SyncContext<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        info!("Native provider host started");
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        if self.state.is_none() {
            //warn!("Stopped a provider host that had no state. Something might be amiss, askew, or perchance awry");
            return;
        }

        let mut state = self.state.as_mut().unwrap();

        /* let cp = ControlInterface::from_hostlocal_registry(&state.kp.public_key());
        cp.do_send(PublishEvent {
            event: ControlEvent::ProviderStopped {
                link_name: state.cap.link_name.to_string(),
                provider_id: state.cap.claims.subject.to_string(),
                contract_id: state
                    .cap
                    .claims
                    .metadata
                    .as_ref()
                    .unwrap()
                    .capid
                    .to_string(),
                reason: TerminationReason::Requested,
            },
        }); */
        state.plugin.stop(); // Tell the provider to clean up, dispose of resources, stop threads, etc
        if let Some(l) = state.library.take() {
            let r = l.close();
            if let Err(e) = r {
                //
            }
        }
    }
}

impl Handler<Initialize> for NativeCapabilityHost {
    type Result = Result<WasccEntity>;

    fn handle(&mut self, msg: Initialize, ctx: &mut Self::Context) -> Self::Result {
        let (library, plugin) = match extrude(&msg.cap) {
            Ok((l, r)) => (l, r),
            Err(e) => {
                error!("Failed to extract plugin from provider: {}", e);
                ctx.stop();
                return Err("Failed to extract plugin from provider".into());
            }
        };
        /*let descriptor = match get_descriptor(&plugin) {
            Ok(d) => d,
            Err(e) => {
                error!("Failed to get descriptor from provider: {}", e);
                ctx.stop();
                return Err("Failed to get descriptor from provider".into());
            }
        }; */
        // Descriptor usage should be deprecated..

        self.state = Some(State {
            cap: msg.cap,
            mw_chain: msg.mw_chain,
            kp: KeyPair::from_seed(&msg.seed)?,
            library,
            plugin,
            image_ref: msg.image_ref,
        });
        let state = self.state.as_ref().unwrap();

        let b = MessageBus::from_hostlocal_registry(&state.kp.public_key());
        let b2 = b.clone();
        let entity = WasccEntity::Capability {
            id: state.cap.claims.subject.to_string(),
            contract_id: state
                .cap
                .claims
                .metadata
                .as_ref()
                .unwrap()
                .capid
                .to_string(),
            link: state.cap.link_name.to_string(),
        };

        let nativedispatch = ProviderDispatcher::new(
            b.clone().recipient(),
            KeyPair::from_seed(&state.kp.seed().unwrap()).unwrap(),
            entity.clone(),
        );
        if let Err(e) = state.plugin.configure_dispatch(Box::new(nativedispatch)) {
            error!(
                "Failed to configure provider dispatcher: {}, provider stopping.",
                e
            );
            ctx.stop();
            return Err(e);
        }
        let url = entity.url().to_string();
        let submsg = Subscribe {
            interest: entity.clone(),
            subscriber: ctx.address().recipient(),
        };
        let _ = block_on(async move {
            if let Err(e) = b.send(submsg).await {
                error!(
                    "Native capability provider failed to subscribe to bus: {}",
                    e
                );
                ctx.stop();
            } else {
            }
        });
        let epl = EnforceLocalProviderLinks {
            provider_id: state.cap.claims.subject.to_string(),
            link_name: state.cap.link_name.to_string(),
        };
        let _ = block_on(async move {
            // If the target provider for any known links involving this provider
            // are present, perform the bind actor func call
            let _ = b2.send(epl).await;
        });
        let cp = ControlInterface::from_hostlocal_registry(&state.kp.public_key());
        cp.do_send(PublishEvent {
            event: ControlEvent::ProviderStarted {
                link_name: state.cap.link_name.to_string(),
                provider_id: state.cap.claims.subject.to_string(),
                contract_id: state
                    .cap
                    .claims
                    .metadata
                    .as_ref()
                    .unwrap()
                    .capid
                    .to_string(),
                image_ref: state.image_ref.clone(),
            },
        });
        info!("Native Capability Provider '{}' ready", url);

        Ok(entity)
    }
}

impl Handler<Invocation> for NativeCapabilityHost {
    type Result = InvocationResponse;

    /// Receives an invocation from any source, validating the anti-forgery token
    /// and that the destination matches this process. If those checks pass, runs
    /// the capability provider pre-invoke middleware, invokes the operation on the native
    /// plugin, then runs the provider post-invoke middleware.
    fn handle(&mut self, inv: Invocation, _ctx: &mut Self::Context) -> Self::Result {
        let state = self.state.as_ref().unwrap();
        trace!(
            "Provider {} handling invocation operation '{}'",
            state.cap.claims.subject,
            inv.operation
        );
        if let WasccEntity::Actor(ref s) = inv.origin {
            if let WasccEntity::Capability { id, .. } = &inv.target {
                if id != &state.cap.id() {
                    return InvocationResponse::error(
                        &inv,
                        "Invocation target ID did not match provider ID",
                    );
                }
                if let Err(e) = run_capability_pre_invoke(&inv, &state.mw_chain) {
                    return InvocationResponse::error(
                        &inv,
                        &format!("Capability middleware pre-invoke failure: {}", e),
                    );
                }

                match state.plugin.handle_call(&s, &inv.operation, &inv.msg) {
                    Ok(msg) => {
                        let ir = InvocationResponse::success(&inv, msg);
                        match run_capability_post_invoke(ir, &state.mw_chain) {
                            Ok(r) => r,
                            Err(e) => InvocationResponse::error(
                                &inv,
                                &format!("Capability middleware post-invoke failure: {}", e),
                            ),
                        }
                    }
                    Err(e) => InvocationResponse::error(&inv, &format!("{}", e)),
                }
            } else {
                InvocationResponse::error(&inv, "Invocation sent to the wrong target")
            }
        } else {
            InvocationResponse::error(&inv, "Attempt to invoke capability from non-actor origin")
        }
    }
}

fn extrude(
    cap: &NativeCapability,
) -> Result<(Option<Library>, Box<dyn CapabilityProvider + 'static>)> {
    use std::io::Write;
    if let Some(ref bytes) = cap.native_bytes {
        let path = temp_dir();
        let path = path.join("wasmcloudcache");
        let path = path.join(&cap.claims.subject);
        let path = path.join(format!(
            "{}",
            cap.claims.metadata.as_ref().unwrap().rev.unwrap_or(0)
        ));
        ::std::fs::create_dir_all(&path)?;
        let target = Host::native_target();
        let path = path.join(&target);
        // If this file is already on disk, some other host has probably
        // created it so don't over-write
        if !path.exists() {
            let mut tf = File::create(&path)?;
            tf.write_all(&bytes)?;
        }
        type PluginCreate = unsafe fn() -> *mut dyn CapabilityProvider;
        let library = Library::new(&path)?;

        let plugin = unsafe {
            let constructor: Symbol<PluginCreate> = library.get(b"__capability_provider_create")?;
            let boxed_raw = constructor();

            Box::from_raw(boxed_raw)
        };
        Ok((Some(library), plugin))
    } else {
        Ok((None, cap.plugin.clone().unwrap()))
    }
}

fn get_descriptor(plugin: &Box<dyn CapabilityProvider>) -> Result<CapabilityDescriptor> {
    if let Ok(v) = plugin.handle_call(SYSTEM_ACTOR, OP_GET_CAPABILITY_DESCRIPTOR, &[]) {
        match crate::generated::core::deserialize::<CapabilityDescriptor>(&v) {
            Ok(c) => Ok(c),
            Err(e) => Err(format!("Failed to deserialize descriptor: {}", e).into()),
        }
    } else {
        Err("Failed to invoke GetCapabilityDescriptor".into())
    }
}

#[cfg(test)]
mod test {
    use crate::capability::extras::{ExtrasCapabilityProvider, OP_REQUEST_GUID};
    use crate::capability::native::NativeCapability;
    use crate::capability::native_host::NativeCapabilityHost;
    use crate::dispatch::{Invocation, WasccEntity};
    use crate::generated::extras::{GeneratorRequest, GeneratorResult};
    use crate::SYSTEM_ACTOR;
    use actix::prelude::*;
    use wascap::prelude::KeyPair;

    #[actix_rt::test]
    async fn test_extras_actor() {
        let kp = KeyPair::new_server();
        let seed = kp.seed().unwrap();
        let extras = ExtrasCapabilityProvider::default();
        let claims = crate::capability::extras::get_claims();
        let cap =
            NativeCapability::from_instance(extras, Some("default".to_string()), claims).unwrap();
        let extras = SyncArbiter::start(1, move || NativeCapabilityHost::new());
        let init = crate::capability::native_host::Initialize {
            cap,
            mw_chain: vec![],
            seed,
            image_ref: None,
        };
        let _ = extras.send(init).await.unwrap();

        let req = GeneratorRequest {
            guid: true,
            sequence: false,
            random: false,
            min: 0,
            max: 0,
        };
        let inv = Invocation::new(
            &kp,
            WasccEntity::Actor(SYSTEM_ACTOR.to_string()),
            WasccEntity::Capability {
                id: "VDHPKGFKDI34Y4RN4PWWZHRYZ6373HYRSNNEM4UTDLLOGO5B37TSVREP".to_string(),
                contract_id: "wascc:extras".to_string(),
                link: "default".to_string(),
            },
            OP_REQUEST_GUID,
            crate::generated::extras::serialize(&req).unwrap(),
        );
        let ir = extras.send(inv).await.unwrap();
        assert!(ir.error.is_none());
        let gen_r: GeneratorResult = crate::generated::extras::deserialize(&ir.msg).unwrap();
        assert!(gen_r.guid.is_some());
    }
}
