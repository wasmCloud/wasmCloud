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
use crate::control_plane::cpactor::{ControlOptions, ControlPlane, PublishEvent};
use crate::control_plane::ControlPlaneProvider;
use crate::dispatch::{Invocation, InvocationResponse};
use crate::host_controller::{
    GetHostID, HostController, MintInvocationRequest, SetLabels, StartActor, StartProvider,
    StopActor, StopProvider, RESTRICTED_LABELS,
};
use crate::messagebus::{QueryActors, QueryProviders};
use crate::oci::fetch_oci_bytes;
use crate::{ControlEvent, HostManifest, WasccEntity};
use crate::{Result, SYSTEM_ACTOR};
use provider_archive::ProviderArchive;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

pub struct HostBuilder {
    labels: HashMap<String, String>,
    authorizer: Box<dyn Authorizer + 'static>,
    allow_latest: bool,
}

impl HostBuilder {
    pub fn new() -> HostBuilder {
        HostBuilder {
            labels: crate::host_controller::detect_core_host_labels(),
            authorizer: Box::new(crate::auth::DefaultAuthorizer::new()),
            allow_latest: false,
        }
    }

    pub fn with_authorizer(self, authorizer: impl Authorizer + 'static) -> HostBuilder {
        HostBuilder {
            authorizer: Box::new(authorizer),
            ..self
        }
    }

    /// Consulted when a host runtime needs to download an image from an OCI registry,
    /// this option enables the use of images tagged 'latest'. The default is `false` to prevent
    /// accidental mutation of images, close potential attack vectors, and prevent against
    /// inconsistencies when running as part of a distributed system. Also keep in mind that if you
    /// do enable images to run as 'latest', it may interfere with live update/hot swap
    /// functionality
    pub fn oci_allow_latest(self) -> HostBuilder {
        HostBuilder {
            allow_latest: true,
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
            allow_latest: self.allow_latest,
        }
    }
}

pub struct Host {
    labels: HashMap<String, String>,
    authorizer: Box<dyn Authorizer + 'static>,
    id: RefCell<String>,
    allow_latest: bool,
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
            cp.send(crate::control_plane::cpactor::Initialize {
                provider: lattice_control,
                control_options: ControlOptions {
                    host_labels: self.labels.clone(),
                    oci_allow_latest: self.allow_latest,
                    ..Default::default()
                },
            })
            .await?;
        }

        Ok(())
    }

    pub async fn stop(&self) {
        let cp = ControlPlane::from_registry();
        let _ = cp
            .send(PublishEvent {
                event: ControlEvent::HostStopped,
            })
            .await;
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
        let bytes = fetch_oci_bytes(cap_ref, self.allow_latest).await?;
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
        let bytes = fetch_oci_bytes(actor_ref, self.allow_latest).await?;
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

    pub async fn apply_manifest(&self, manifest: HostManifest) -> Result<()> {
        let hc = HostController::from_registry();
        let bus = MessageBus::from_registry();

        if manifest.labels.len() > 0 {
            let mut labels = manifest.labels.clone();
            for x in 0..RESTRICTED_LABELS.len() {
                labels.remove(RESTRICTED_LABELS[x]); // getting an iterator of this const produces `&&str` which is a super annoying type
            }
            hc.send(SetLabels { labels }).await?;
        }

        for msg in
            crate::manifest::generate_actor_start_messages(&manifest, self.allow_latest).await
        {
            let _ = hc.send(msg).await?;
        }
        for msg in
            crate::manifest::generate_provider_start_messages(&manifest, self.allow_latest).await
        {
            let _ = hc.send(msg).await?;
        }
        for msg in crate::manifest::generate_adv_binding_messages(&manifest).await {
            let _ = bus.send(msg).await?;
        }

        Ok(())
    }

    pub(crate) fn native_target() -> String {
        format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
    }
}
