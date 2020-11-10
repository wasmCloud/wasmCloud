use crate::capability::native::NativeCapability;
use crate::control_plane::actorhost::{ControlPlane, PublishEvent};
use crate::control_plane::events::TerminationReason;
use crate::dispatch::{Invocation, InvocationResponse, ProviderDispatcher, WasccEntity};
use crate::messagebus::{MessageBus, Subscribe, Unsubscribe};
use crate::middleware::{run_capability_post_invoke, run_capability_pre_invoke, Middleware};
use crate::{errors, Host, SYSTEM_ACTOR};
use crate::{ControlEvent, Result};
use actix::prelude::*;
use futures::executor::block_on;
use libloading::{Library, Symbol};
use std::env::temp_dir;
use std::fs::File;
use std::sync::Arc;
use wascap::prelude::KeyPair;
use wascc_codec::capabilities::{
    CapabilityDescriptor, CapabilityProvider, OP_GET_CAPABILITY_DESCRIPTOR,
};

#[derive(Clone)]
pub(crate) struct NativeCapabilityHostBuilder {
    cap: NativeCapability,
    mw_chain: Vec<Box<dyn Middleware>>,
    image_ref: Option<String>,
    plugin: Box<dyn CapabilityProvider + 'static>,
    library: Arc<Option<Library>>,
    descriptor: CapabilityDescriptor,
}

impl NativeCapabilityHostBuilder {
    pub fn try_new(
        cap: NativeCapability,
        mw_chain: Vec<Box<dyn Middleware>>,
        image_ref: Option<String>,
    ) -> Result<Self> {
        let (library, plugin) = extrude(&cap)?;
        let descriptor = get_descriptor(&plugin)?;
        Ok(NativeCapabilityHostBuilder {
            cap,
            mw_chain,
            plugin,
            image_ref,
            library: Arc::new(library),
            descriptor,
        })
    }

    pub fn build(self, kp: KeyPair) -> NativeCapabilityHost {
        NativeCapabilityHost {
            library: self.library,
            plugin: self.plugin,
            cap: self.cap,
            mw_chain: self.mw_chain,
            kp,
            descriptor: self.descriptor,
            image_ref: self.image_ref,
        }
    }
}

pub(crate) struct NativeCapabilityHost {
    cap: NativeCapability,
    mw_chain: Vec<Box<dyn Middleware>>,
    kp: KeyPair,
    library: Arc<Option<Library>>,
    plugin: Box<dyn CapabilityProvider + 'static>,
    descriptor: CapabilityDescriptor,
    image_ref: Option<String>,
}

fn extrude(
    cap: &NativeCapability,
) -> Result<(Option<Library>, Box<dyn CapabilityProvider + 'static>)> {
    use std::io::Write;
    if let Some(ref bytes) = cap.native_bytes {
        let path = temp_dir();
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

impl Actor for NativeCapabilityHost {
    type Context = SyncContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let b = MessageBus::from_registry();
        let entity = WasccEntity::Capability {
            id: self.cap.claims.subject.to_string(),
            contract_id: self.descriptor.id.to_string(),
            binding: self.cap.binding_name.to_string(),
        };
        info!("Native provider started: {}", entity.url());
        let nativedispatch = ProviderDispatcher::new(
            b.clone().recipient(),
            KeyPair::from_seed(&self.kp.seed().unwrap()).unwrap(),
            entity.clone(),
        );
        if let Err(e) = self.plugin.configure_dispatch(Box::new(nativedispatch)) {
            error!(
                "Failed to configure provider dispatcher: {}, provider stopping.",
                e
            );
            ctx.stop();
        }
        let url = entity.url().to_string();
        let _ = block_on(async move {
            if let Err(e) = b
                .send(Subscribe {
                    interest: entity.clone(),
                    subscriber: ctx.address().recipient(),
                })
                .await
            {
                error!(
                    "Native capability provider failed to subscribe to bus: {}",
                    e
                );
                ctx.stop();
            }
        });
        let cp = ControlPlane::from_registry();
        cp.do_send(PublishEvent {
            event: ControlEvent::ProviderStarted {
                header: Default::default(),
                binding_name: self.cap.binding_name.to_string(),
                provider_id: self.cap.claims.subject.to_string(),
                contract_id: self.descriptor.id.to_string(),
                image_ref: self.image_ref.clone(),
            },
        });
        info!("Native Capability Provider '{}' ready", url);
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        info!(
            "Provider stopped {} - {}",
            &self.cap.claims.subject, self.descriptor.name
        );

        let cp = ControlPlane::from_registry();
        cp.do_send(PublishEvent {
            event: ControlEvent::ProviderStopped {
                header: Default::default(),
                binding_name: self.cap.binding_name.to_string(),
                provider_id: self.cap.claims.subject.to_string(),
                contract_id: self.descriptor.id.to_string(),
                reason: TerminationReason::Requested,
            },
        });
        self.plugin.stop(); // Tell the provider to clean up, dispose of resources, stop threads, etc
    }
}

impl Handler<Invocation> for NativeCapabilityHost {
    type Result = InvocationResponse;

    /// Receives an invocation from any source, validating the anti-forgery token
    /// and that the destination matches this process. If those checks pass, runs
    /// the capability provider pre-invoke middleware, invokes the operation on the native
    /// plugin, then runs the provider post-invoke middleware.
    fn handle(&mut self, inv: Invocation, ctx: &mut Self::Context) -> Self::Result {
        trace!(
            "Provider {} handling {}",
            self.cap.claims.subject,
            inv.operation
        );
        if let WasccEntity::Actor(ref s) = inv.origin {
            if let WasccEntity::Capability {
                id,
                contract_id,
                binding,
            } = &inv.target
            {
                if id != &self.cap.id() {
                    return InvocationResponse::error(
                        &inv,
                        "Invocation target ID did not match provider ID",
                    );
                }
                if let Err(e) = run_capability_pre_invoke(&inv, &self.mw_chain) {
                    return InvocationResponse::error(
                        &inv,
                        &format!("Capability middleware pre-invoke failure: {}", e),
                    );
                }

                match self.plugin.handle_call(&s, &inv.operation, &inv.msg) {
                    Ok(msg) => {
                        let ir = InvocationResponse::success(&inv, msg);
                        match run_capability_post_invoke(ir, &self.mw_chain) {
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

#[cfg(test)]
mod test {
    use crate::capability::extras::{ExtrasCapabilityProvider, OP_REQUEST_GUID};
    use crate::capability::native::NativeCapability;
    use crate::capability::native_host::{NativeCapabilityHost, NativeCapabilityHostBuilder};
    use crate::dispatch::{Invocation, InvocationResponse, WasccEntity};
    use crate::generated::extras::{GeneratorRequest, GeneratorResult};
    use crate::Result;
    use crate::SYSTEM_ACTOR;
    use actix::prelude::*;
    use std::sync::Arc;
    use wascap::prelude::KeyPair;

    #[actix_rt::test]
    async fn test_extras_actor() {
        let kp = KeyPair::new_server();
        let seed = kp.seed().unwrap();
        let extras = SyncArbiter::start(1, move || {
            let key = KeyPair::from_seed(&seed).unwrap();
            let extras = ExtrasCapabilityProvider::default();
            let claims = crate::capability::extras::get_claims();
            let cap = NativeCapability::from_instance(extras, Some("default".to_string()), claims)
                .unwrap();
            NativeCapabilityHostBuilder::try_new(cap, vec![], None)
                .unwrap()
                .build(key)
        });

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
                binding: "default".to_string(),
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
