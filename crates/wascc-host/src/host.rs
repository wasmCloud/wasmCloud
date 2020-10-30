use crate::messagebus::{
    AdvertiseBinding, LatticeProvider, MessageBus, SetAuthorizer, SetProvider,
};
use std::thread;
use wapc::WebAssemblyEngineProvider;

use actix::prelude::*;

use crate::auth::Authorizer;
use crate::capability::extras::ExtrasCapabilityProvider;
use crate::capability::native::NativeCapability;
use crate::capability::native_host::NativeCapabilityHost;
use crate::control_plane::actorhost::{ControlPlane, PublishEvent};
use crate::control_plane::ControlPlaneProvider;
use crate::dispatch::{Invocation, InvocationResponse};
use crate::host_controller::{
    GetHostID, HostController, MintInvocationRequest, SetLabels, StartActor, StartProvider,
    StopActor, StopProvider,
};
use crate::messagebus::{QueryActors, QueryProviders};
use crate::oci::fetch_oci_bytes;
use crate::{ControlEvent, WasccEntity};
use crate::{Result, SYSTEM_ACTOR};
use provider_archive::ProviderArchive;
use std::cell::RefCell;
use std::collections::HashMap;
use crate::control_plane::events::TerminationReason;

pub struct HostBuilder {
    labels: HashMap<String, String>,
    authorizer: Box<dyn Authorizer + 'static>,
}

impl HostBuilder {
    pub fn new() -> HostBuilder {
        HostBuilder {
            labels: crate::host_controller::detect_core_host_labels(),
            authorizer: Box::new(crate::auth::DefaultAuthorizer::new()),
        }
    }

    pub fn with_authorizer(self, authorizer: impl Authorizer + 'static) -> HostBuilder {
        HostBuilder {
            authorizer: Box::new(authorizer),
            ..self
        }
    }

    pub fn with_label(self, key: &str, value: &str) -> HostBuilder {
        let mut hm = self.labels.clone();
        if !hm.contains_key(key) {
            hm.insert(key.to_string(), value.to_string());
        }
        HostBuilder { labels: hm, ..self }
    }

    pub fn build(self) -> Host {
        Host {
            labels: self.labels,
            authorizer: self.authorizer,
            id: RefCell::new("".to_string()),
        }
    }
}

pub struct Host {
    labels: HashMap<String, String>,
    authorizer: Box<dyn Authorizer + 'static>,
    id: RefCell<String>,
}

impl Host {
    /// Starts the host's actor system. This call is non-blocking, so it is up to the consumer
    /// to provide some form of parking or waiting (e.g. wait for a Ctrl-C signal).
    pub async fn start(
        &self,
        lattice_rpc: Option<Box<dyn LatticeProvider + 'static>>,
        lattice_control: Option<Box<dyn ControlPlaneProvider + 'static>>,
    ) -> Result<()> {
        let mb = MessageBus::from_registry();
        if let Some(l) = lattice_rpc {
            mb.send(SetProvider { provider: l }).await?;
        }
        // message bus authorizes invocations, host controller authorizes loads
        mb.send(SetAuthorizer {
            auth: self.authorizer.clone(),
        })
        .await?;

        let hc = HostController::from_registry();
        let hc2 = hc.clone();
        hc2.send(SetAuthorizer {
            auth: self.authorizer.clone(),
        })
        .await?;
        hc.send(SetLabels {
            labels: self.labels.clone(),
        })
        .await?;
        *self.id.borrow_mut() = hc.send(GetHostID {}).await?;

        // Start control plane
        if let Some(lattice_control) = lattice_control {
            let cp = ControlPlane::from_registry();
            cp.send(crate::control_plane::actorhost::SetProvider {
                provider: lattice_control,
                labels: self.labels.clone(),
            })
            .await?;
        }

        Ok(())
    }

    pub async fn stop(&self) {
        let cp = ControlPlane::from_registry();
        let _ = cp.send(PublishEvent {
            event: ControlEvent::HostStopped {
                reason: TerminationReason::Requested,
                header: Default::default(),
            },
        }).await;
        System::current().stop();
    }

    pub fn id(&self) -> String {
        self.id.borrow().to_string()
    }

    pub async fn start_native_capability(&self, capability: crate::NativeCapability) -> Result<()> {
        let hc = HostController::from_registry();
        let _ = hc
            .send(StartProvider {
                provider: capability,
                image_ref: None,
            })
            .await??;

        Ok(())
    }

    pub async fn start_capability_from_registry(
        &self,
        cap_ref: &str,
        binding_name: Option<String>,
    ) -> Result<()> {
        let hc = HostController::from_registry();
        let bytes = fetch_oci_bytes(cap_ref).await?;
        let par = ProviderArchive::try_load(&bytes)?;
        let nc = NativeCapability::from_archive(&par, binding_name)?;
        hc.send(StartProvider {
            provider: nc,
            image_ref: Some(cap_ref.to_string()),
        })
        .await??;
        Ok(())
    }

    pub async fn start_actor(&self, actor: crate::Actor) -> Result<()> {
        let hc = HostController::from_registry();

        hc.send(StartActor {
            actor,
            image_ref: None,
        })
        .await??;
        Ok(())
    }

    pub async fn start_actor_from_registry(&self, actor_ref: &str) -> Result<()> {
        let hc = HostController::from_registry();
        let bytes = fetch_oci_bytes(actor_ref).await?;
        let actor = crate::Actor::from_slice(&bytes)?;
        hc.send(StartActor {
            actor,
            image_ref: Some(actor_ref.to_string()),
        })
        .await??;
        Ok(())
    }

    pub async fn stop_actor(&self, actor_ref: &str) -> Result<()> {
        let hc = HostController::from_registry();
        hc.send(StopActor {
            actor_ref: actor_ref.to_string(),
        })
        .await?;

        Ok(())
    }

    pub async fn stop_provider(
        &self,
        provider_ref: &str,
        contract_id: &str,
        binding: Option<String>,
    ) -> Result<()> {
        let hc = HostController::from_registry();
        let binding = binding.unwrap_or("default".to_string());
        hc.send(StopProvider {
            provider_ref: provider_ref.to_string(),
            contract_id: contract_id.to_string(),
            binding,
        })
        .await?;
        Ok(())
    }

    pub async fn get_actors(&self) -> Result<Vec<String>> {
        let b = MessageBus::from_registry();
        Ok(b.send(QueryActors {}).await?.results)
    }

    pub async fn get_providers(&self) -> Result<Vec<String>> {
        let b = MessageBus::from_registry();
        Ok(b.send(QueryProviders {}).await?.results)
    }

    pub async fn call_actor(&self, actor: &str, operation: &str, msg: &[u8]) -> Result<Vec<u8>> {
        let hc = HostController::from_registry();
        let inv = hc
            .send(MintInvocationRequest {
                op: operation.to_string(),
                target: WasccEntity::Actor(actor.to_string()),
                msg: msg.to_vec(),
                origin: WasccEntity::Actor(SYSTEM_ACTOR.to_string()),
            })
            .await?;
        let b = MessageBus::from_registry();
        let ir = b.send(inv).await?;
        Ok(ir.msg)
    }

    pub async fn set_binding(
        &self,
        actor: &str,
        contract_id: &str,
        binding_name: Option<String>,
        provider_id: String,
        values: HashMap<String, String>,
    ) -> Result<()> {
        let bus = MessageBus::from_registry();
        bus.send(AdvertiseBinding {
            contract_id: contract_id.to_string(),
            actor: actor.to_string(),
            binding_name: binding_name.unwrap_or("default".to_string()),
            provider_id,
            values,
        })
        .await?
    }

    pub(crate) fn native_target() -> String {
        format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
    }
}
