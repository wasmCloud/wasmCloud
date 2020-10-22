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
use crate::control_plane::actorhost::ControlPlane;
use crate::control_plane::ControlPlaneProvider;
use crate::dispatch::{Invocation, InvocationResponse};
use crate::host_controller::{
    HostController, MintInvocationRequest, SetLabels, StartActor, StartProvider,
};
use crate::messagebus::{QueryActors, QueryProviders};
use crate::WasccEntity;
use crate::{Result, SYSTEM_ACTOR};
use std::collections::HashMap;

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
        }
    }
}

pub struct Host {
    labels: HashMap<String, String>,
    authorizer: Box<dyn Authorizer + 'static>,
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

        // Start control plane
        let cp = ControlPlane::from_registry();
        if let Some(lattice_control) = lattice_control {
            cp.send(crate::control_plane::actorhost::SetProvider {
                provider: lattice_control,
                labels: self.labels.clone(),
            })
            .await?;
        }

        Ok(())
    }

    pub async fn start_native_capability(&self, capability: crate::NativeCapability) -> Result<()> {
        let hc = HostController::from_registry();
        let _ = hc
            .send(StartProvider {
                provider: capability,
            })
            .await??;

        Ok(())
    }

    pub async fn start_actor(&self, actor: crate::Actor) -> Result<()> {
        let hc = HostController::from_registry();

        hc.send(StartActor { actor }).await?;
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
