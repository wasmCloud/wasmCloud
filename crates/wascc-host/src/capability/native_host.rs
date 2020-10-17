use crate::capability::native::NativeCapability;
use crate::dispatch::{Invocation, InvocationResponse, WasccEntity};
use crate::errors;
use crate::messagebus::{MessageBus, Subscribe};
use crate::middleware::{run_capability_post_invoke, run_capability_pre_invoke, Middleware};
use crate::Result;
use actix::prelude::*;
use futures::executor::block_on;

pub(crate) struct NativeCapabilityHost {
    cap: NativeCapability,
    mw_chain: Vec<Box<dyn Middleware>>,
}

impl NativeCapabilityHost {
    pub fn new(cap: NativeCapability, mw_chain: Vec<Box<dyn Middleware>>) -> Self {
        NativeCapabilityHost { cap, mw_chain }
    }
}

impl Actor for NativeCapabilityHost {
    type Context = SyncContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let b = MessageBus::from_registry();
        let entity = WasccEntity::Capability {
            id: self.cap.claims.subject.to_string(),
            contract_id: self.cap.contract_id(),
            binding: self.cap.binding_name.to_string(),
        };
        let _ = block_on(async move {
            b.send(Subscribe {
                interest: entity,
                subscriber: ctx.address().recipient(),
            })
            .await
        });
        info!(
            "Native Capability Provider '{}' ready ({}/{})",
            &self.cap.claims.subject,
            &self.cap.contract_id(),
            &self.cap.binding_name
        );
    }
}

impl Handler<Invocation> for NativeCapabilityHost {
    type Result = InvocationResponse;

    /// Receives an invocation from any source, validating the anti-forgery token
    /// and that the destination matches this process. If those checks pass, runs
    /// the capability provider pre-invoke middleware, invokes the operation on the native
    /// plugin, then runs the provider post-invoke middleware.
    fn handle(&mut self, inv: Invocation, ctx: &mut Self::Context) -> Self::Result {
        if inv.validate_antiforgery().is_err() {
            return InvocationResponse::error(
                &inv,
                "Anti-forgery validation failed for invocation.",
            );
        }

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

                match self.cap.plugin.handle_call(&s, &inv.operation, &inv.msg) {
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
    use crate::capability::native_host::NativeCapabilityHost;
    use crate::dispatch::{Invocation, InvocationResponse, WasccEntity};
    use crate::generated::extras::{GeneratorRequest, GeneratorResult};
    use crate::Result;
    use crate::SYSTEM_ACTOR;
    use actix::prelude::*;
    use wascap::prelude::KeyPair;

    #[actix_rt::test]
    async fn test_extras_actor() {
        let extras = SyncArbiter::start(1, || {
            let extras = ExtrasCapabilityProvider::default();
            let claims = crate::capability::extras::get_claims();
            let cap = NativeCapability::from_instance(extras, Some("default".to_string()), claims)
                .unwrap();
            NativeCapabilityHost::new(cap, vec![])
        });
        let kp = KeyPair::new_server();
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
