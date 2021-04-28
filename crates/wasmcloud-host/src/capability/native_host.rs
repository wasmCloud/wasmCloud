use crate::capability::native::{normalize_link_name, NativeCapability};
use crate::control_interface::ctlactor::{ControlInterface, PublishEvent};
use actix::dev::MessageResponse;

use crate::dispatch::OP_HALT;
use crate::dispatch::{Invocation, InvocationResponse, ProviderDispatcher, WasmCloudEntity};
use crate::hlreg::HostLocalSystemService;
use crate::messagebus::{EnforceLocalProviderLinks, MessageBus, Subscribe};
use crate::middleware::{run_capability_post_invoke, run_capability_pre_invoke, Middleware};
use crate::Result;
use actix::prelude::*;
use futures::executor::block_on;
use libloading::{Library, Symbol};
use wascap::prelude::KeyPair;
use wasmcloud_control_interface::events::ControlEvent;
use wasmcloud_provider_core::capabilities::CapabilityProvider;

#[derive(Message)]
#[rtype(result = "Result<WasmCloudEntity>")]
pub(crate) struct Initialize {
    pub cap: NativeCapability,
    pub mw_chain: Vec<Box<dyn Middleware>>,
    pub seed: String,
    pub image_ref: Option<String>,
}

#[derive(Message)]
#[rtype(result = "IdentityResponse")]
pub(crate) struct GetIdentity {}

pub(crate) struct IdentityResponse {
    pub image_ref: Option<String>,
    pub name: String,
    pub revision: i32,
}

impl<A, M> MessageResponse<A, M> for IdentityResponse
where
    A: Actor,
    M: Message<Result = IdentityResponse>,
{
    fn handle(self, _: &mut A::Context, tx: Option<actix::dev::OneshotSender<Self>>) {
        if let Some(tx) = tx {
            if tx.send(self).is_err() {
                error!("send error (IdentityResponse)");
            }
        }
    }
}

struct State {
    cap: NativeCapability,
    mw_chain: Vec<Box<dyn Middleware>>,
    kp: KeyPair,
    library: Option<Library>,
    plugin: Box<dyn CapabilityProvider + 'static>,
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
        let state = self.state.as_mut().unwrap();

        state.plugin.stop(); // Tell the provider to clean up, dispose of resources, stop threads, etc
        if let Some(l) = state.library.take() {
            let r = l.close();
            if let Err(_e) = r {
                //
            }
        }
    }
}

impl Handler<Initialize> for NativeCapabilityHost {
    type Result = Result<WasmCloudEntity>;

    fn handle(&mut self, msg: Initialize, ctx: &mut Self::Context) -> Self::Result {
        let (library, plugin) = match extrude(&msg.cap) {
            Ok((l, r)) => (l, r),
            Err(e) => {
                error!("Failed to extract plugin from provider: {}", e);
                ctx.stop();
                return Err("Failed to extract plugin from provider".into());
            }
        };
        // NOTE: used to invoke get descriptor here, but we no longer obtain that information
        // from the provider at runtime, it's obtained from the now-mandatory (0.15.0+) claims

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
        let link_name = normalize_link_name(state.cap.link_name.to_string());
        let entity = WasmCloudEntity::Capability {
            id: state.cap.claims.subject.to_string(),
            contract_id: state
                .cap
                .claims
                .metadata
                .as_ref()
                .unwrap()
                .capid
                .to_string(),
            link_name: link_name.to_string(),
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
        let url = entity.url();
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
            link_name: link_name.to_string(),
        };
        let _ = block_on(async move {
            // If the target provider for any known links involving this provider
            // are present, perform the bind actor func call
            let _ = b2.send(epl).await;
        });
        let cp = ControlInterface::from_hostlocal_registry(&state.kp.public_key());
        cp.do_send(PublishEvent {
            event: ControlEvent::ProviderStarted {
                link_name,
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

impl Handler<GetIdentity> for NativeCapabilityHost {
    type Result = IdentityResponse;

    fn handle(&mut self, _q: GetIdentity, _ctx: &mut Self::Context) -> Self::Result {
        let state = self.state.as_ref().unwrap();
        IdentityResponse {
            image_ref: state.image_ref.clone(),
            name: state.cap.claims.name(),
            revision: state
                .cap
                .claims
                .metadata
                .as_ref()
                .map(|md| md.rev.unwrap_or(0))
                .unwrap_or(0),
        }
    }
}

impl Handler<Invocation> for NativeCapabilityHost {
    type Result = InvocationResponse;

    /// Receives an invocation from any source, validating the anti-forgery token
    /// and that the destination matches this process. If those checks pass, runs
    /// the capability provider pre-invoke middleware, invokes the operation on the native
    /// plugin, then runs the provider post-invoke middleware.
    fn handle(&mut self, inv: Invocation, ctx: &mut Self::Context) -> Self::Result {
        let state = self.state.as_ref().unwrap();
        if inv.origin == inv.target && inv.operation == OP_HALT {
            info!(
                "Received explicit halt instruction. Provider {} shutting down",
                state.cap.claims.subject
            );
            ctx.stop();
            return InvocationResponse::success(&inv, vec![]);
        }
        trace!(
            "Provider {} handling invocation operation '{}'",
            state.cap.claims.subject,
            inv.operation
        );
        if let WasmCloudEntity::Actor(ref s) = inv.origin {
            if let WasmCloudEntity::Capability { id, .. } = &inv.target {
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

pub(crate) fn write_provider_to_disk(cap: &NativeCapability) -> Result<()> {
    if let Some(ref bytes) = cap.native_bytes {
        use std::io::Write;
        let path = cap.cache_path();
        let mut parent_dir = path.clone();
        parent_dir.pop();
        std::fs::create_dir_all(parent_dir)?;
        debug!("Caching provider to {}", path.display());
        let mut file = std::fs::File::create(&path)?;
        Ok(file.write_all(&bytes)?)
    } else {
        Err("No bytes to cache".into())
    }
}

fn extrude(
    cap: &NativeCapability,
) -> Result<(Option<Library>, Box<dyn CapabilityProvider + 'static>)> {
    if cap.native_bytes.is_some() {
        let path = cap.cache_path();
        // If this file is already on disk, don't overwrite
        if path.exists() {
            debug!("Using cache at {}", path.display());
        } else {
            write_provider_to_disk(cap)?;
        }
        #[cfg(target_os = "linux")]
        let library: Library = {
            // On linux the library must be loaded with `RTLD_NOW | RTLD_NODELETE` to fix a SIGSEGV
            // that happens when a thread terminates AFTER a library is unloaded. This has the
            // effect of keeping the library loaded in-memory until the process ends.
            //
            // https://github.com/nagisa/rust_libloading/issues/41
            unsafe { ::libloading::os::unix::Library::open(Some(&path), 0x2 | 0x1000)? }.into()
        };
        #[cfg(not(target_os = "linux"))]
        let library: Library = unsafe { Library::new(&path)? };

        type PluginCreate = unsafe fn() -> *mut dyn CapabilityProvider;
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

#[cfg(test)]
mod test {
    use crate::capability::extras::{
        ExtrasCapabilityProvider, OP_REQUEST_GUID, OP_REQUEST_RANDOM, OP_REQUEST_SEQUENCE,
    };
    use crate::capability::native::NativeCapability;
    use crate::capability::native_host::NativeCapabilityHost;
    use crate::dispatch::{Invocation, WasmCloudEntity};
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

        // Test generating a guid
        let req = GeneratorRequest {
            guid: true,
            sequence: false,
            random: false,
            min: 0,
            max: 0,
        };
        let inv = Invocation::new(
            &kp,
            WasmCloudEntity::Actor(SYSTEM_ACTOR.to_string()),
            WasmCloudEntity::Capability {
                id: "VDHPKGFKDI34Y4RN4PWWZHRYZ6373HYRSNNEM4UTDLLOGO5B37TSVREP".to_string(),
                contract_id: "wasmcloud:extras".to_string(),
                link_name: "default".to_string(),
            },
            OP_REQUEST_GUID,
            crate::generated::core::serialize(&req).unwrap(),
        );
        let ir = extras.send(inv).await.unwrap();
        assert!(ir.error.is_none());
        let gen_r: GeneratorResult = crate::generated::core::deserialize(&ir.msg).unwrap();
        assert!(gen_r.guid.is_some());

        // Test generating a random number
        let req = GeneratorRequest {
            guid: false,
            sequence: false,
            random: true,
            min: 1,
            max: 256,
        };
        let inv = Invocation::new(
            &kp,
            WasmCloudEntity::Actor(SYSTEM_ACTOR.to_string()),
            WasmCloudEntity::Capability {
                id: "VDHPKGFKDI34Y4RN4PWWZHRYZ6373HYRSNNEM4UTDLLOGO5B37TSVREP".to_string(),
                contract_id: "wasmcloud:extras".to_string(),
                link_name: "default".to_string(),
            },
            OP_REQUEST_RANDOM,
            crate::generated::core::serialize(&req).unwrap(),
        );
        let ir = extras.send(inv).await.unwrap();
        assert!(ir.error.is_none());
        let gen_r: GeneratorResult = crate::generated::core::deserialize(&ir.msg).unwrap();
        assert_ne!(gen_r.random_number, 0);

        // Test generating a sequence of 0-2
        let req = GeneratorRequest {
            guid: false,
            sequence: true,
            random: false,
            min: 0,
            max: 0,
        };
        let inv = Invocation::new(
            &kp,
            WasmCloudEntity::Actor(SYSTEM_ACTOR.to_string()),
            WasmCloudEntity::Capability {
                id: "VDHPKGFKDI34Y4RN4PWWZHRYZ6373HYRSNNEM4UTDLLOGO5B37TSVREP".to_string(),
                contract_id: "wasmcloud:extras".to_string(),
                link_name: "default".to_string(),
            },
            OP_REQUEST_SEQUENCE,
            crate::generated::core::serialize(&req).unwrap(),
        );
        let ir = extras.send(inv.clone()).await.unwrap();
        assert!(ir.error.is_none());
        let gen_r: GeneratorResult = crate::generated::core::deserialize(&ir.msg).unwrap();
        assert_eq!(gen_r.sequence_number, 0);
        let ir = extras.send(inv.clone()).await.unwrap();
        assert!(ir.error.is_none());
        let gen_r: GeneratorResult = crate::generated::core::deserialize(&ir.msg).unwrap();
        assert_eq!(gen_r.sequence_number, 1);
        let ir = extras.send(inv).await.unwrap();
        assert!(ir.error.is_none());
        let gen_r: GeneratorResult = crate::generated::core::deserialize(&ir.msg).unwrap();
        assert_eq!(gen_r.sequence_number, 2);
    }
}
