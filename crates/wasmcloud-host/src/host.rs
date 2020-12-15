use crate::{
    messagebus::{AdvertiseLink, MessageBus},
    InvocationResponse,
};

use actix::prelude::*;

use crate::auth::Authorizer;

use crate::control_interface::ctlactor::{ControlInterface, ControlOptions, PublishEvent};

use crate::dispatch::Invocation;
use crate::hlreg::HostLocalSystemService;
use crate::host_controller::{
    HostController, SetLabels, StartActor, StartProvider, StopActor, StopProvider,
    RESTRICTED_LABELS,
};
use crate::messagebus::{QueryActors, QueryProviders};
use crate::oci::fetch_oci_bytes;
use crate::{ControlEvent, HostManifest, NativeCapability, WasccEntity};
use crate::{Result, SYSTEM_ACTOR};
use provider_archive::ProviderArchive;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;
use wascap::prelude::KeyPair;

pub struct HostBuilder {
    labels: HashMap<String, String>,
    authorizer: Box<dyn Authorizer + 'static>,
    namespace: String,
    rpc_timeout: Duration,
    allow_latest: bool,
    rpc_client: Option<nats::asynk::Connection>,
    cplane_client: Option<nats::asynk::Connection>,
    allow_live_update: bool,
}

impl HostBuilder {
    pub fn new() -> HostBuilder {
        HostBuilder {
            labels: crate::host_controller::detect_core_host_labels(),
            authorizer: Box::new(crate::auth::DefaultAuthorizer::new()),
            allow_latest: false,
            namespace: "default".to_string(),
            rpc_timeout: Duration::from_secs(2),
            rpc_client: None,
            cplane_client: None,
            allow_live_update: false,
        }
    }

    pub fn enable_live_updates(self) -> HostBuilder {
        HostBuilder {
            allow_live_update: true,
            ..self
        }
    }

    pub fn with_rpc_client(self, client: nats::asynk::Connection) -> HostBuilder {
        HostBuilder {
            rpc_client: Some(client),
            ..self
        }
    }

    pub fn with_control_client(self, client: nats::asynk::Connection) -> HostBuilder {
        HostBuilder {
            cplane_client: Some(client),
            ..self
        }
    }
    pub fn with_authorizer(self, authorizer: impl Authorizer + 'static) -> HostBuilder {
        HostBuilder {
            authorizer: Box::new(authorizer),
            ..self
        }
    }

    pub fn with_namespace(self, namespace: &str) -> HostBuilder {
        HostBuilder {
            namespace: namespace.to_string(),
            ..self
        }
    }

    pub fn with_rpc_timeout(self, rpc_timeout: Duration) -> HostBuilder {
        HostBuilder {
            rpc_timeout,
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
            kp: RefCell::new(None),
            rpc_timeout: self.rpc_timeout,
            namespace: self.namespace,
            rpc_client: self.rpc_client,
            cplane_client: self.cplane_client,
            allow_live_updates: self.allow_live_update,
        }
    }
}

pub struct Host {
    labels: HashMap<String, String>,
    authorizer: Box<dyn Authorizer + 'static>,
    id: RefCell<String>,
    allow_latest: bool,
    kp: RefCell<Option<KeyPair>>,
    namespace: String,
    rpc_timeout: Duration,
    cplane_client: Option<nats::asynk::Connection>,
    rpc_client: Option<nats::asynk::Connection>,
    allow_live_updates: bool,
}

impl Host {
    /// Starts the host's actor system. This call is non-blocking, so it is up to the consumer
    /// to provide some form of parking or waiting (e.g. wait for a Ctrl-C signal).
    pub async fn start(&self) -> Result<()> {
        let kp = KeyPair::new_server();

        let mb = MessageBus::from_hostlocal_registry(&kp.public_key());
        let init = crate::messagebus::Initialize {
            nc: self.rpc_client.clone(),
            namespace: Some(self.namespace.to_string()),
            key: KeyPair::from_seed(&kp.seed()?)?,
            auth: self.authorizer.clone(),
            rpc_timeout: self.rpc_timeout.clone(),
        };
        mb.send(init).await?;

        let hc = HostController::from_hostlocal_registry(&kp.public_key());
        hc.send(crate::host_controller::Initialize {
            labels: self.labels.clone(),
            auth: self.authorizer.clone(),
            kp: KeyPair::from_seed(&kp.seed()?)?,
            allow_live_updates: self.allow_live_updates,
        })
        .await?;
        *self.id.borrow_mut() = kp.public_key();

        // Start control plane
        let cp = ControlInterface::from_hostlocal_registry(&kp.public_key());
        cp.send(crate::control_interface::ctlactor::Initialize {
            client: self.cplane_client.clone(),
            control_options: ControlOptions {
                host_labels: self.labels.clone(),
                oci_allow_latest: self.allow_latest,
                ..Default::default()
            },
            key: KeyPair::from_seed(&kp.seed()?)?,
            ns_prefix: self.namespace.to_string(),
        })
        .await?;

        let _ = cp
            .send(PublishEvent {
                event: ControlEvent::HostStarted,
            })
            .await;

        *self.kp.borrow_mut() = Some(kp);

        Ok(())
    }

    pub async fn stop(&self) {
        let cp = ControlInterface::from_hostlocal_registry(&self.id.borrow());
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
        let hc = HostController::from_hostlocal_registry(&self.id.borrow());
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
        link_name: Option<String>,
    ) -> Result<()> {
        let hc = HostController::from_hostlocal_registry(&self.id.borrow());
        let bytes = fetch_oci_bytes(cap_ref, self.allow_latest, false).await?;
        let par = ProviderArchive::try_load(&bytes)?;
        let nc = NativeCapability::from_archive(&par, link_name)?;
        hc.send(StartProvider {
            provider: nc,
            image_ref: Some(cap_ref.to_string()),
        })
        .await??;
        Ok(())
    }

    pub async fn start_actor(&self, actor: crate::Actor) -> Result<()> {
        let hc = HostController::from_hostlocal_registry(&self.id.borrow());

        hc.send(StartActor {
            actor,
            image_ref: None,
        })
        .await??;
        Ok(())
    }

    pub async fn start_actor_from_registry(&self, actor_ref: &str) -> Result<()> {
        let hc = HostController::from_hostlocal_registry(&self.id.borrow());
        let bytes = fetch_oci_bytes(actor_ref, self.allow_latest, false).await?;
        let actor = crate::Actor::from_slice(&bytes)?;
        hc.send(StartActor {
            actor,
            image_ref: Some(actor_ref.to_string()),
        })
        .await??;
        Ok(())
    }

    pub async fn stop_actor(&self, actor_ref: &str) -> Result<()> {
        let hc = HostController::from_hostlocal_registry(&self.id.borrow());
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
        link: Option<String>,
    ) -> Result<()> {
        let hc = HostController::from_hostlocal_registry(&self.id.borrow());
        let link_name = link.unwrap_or("default".to_string());
        hc.send(StopProvider {
            provider_ref: provider_ref.to_string(),
            contract_id: contract_id.to_string(),
            link_name,
        })
        .await?;
        Ok(())
    }

    pub async fn get_actors(&self) -> Result<Vec<String>> {
        let b = MessageBus::from_hostlocal_registry(&self.id.borrow());
        Ok(b.send(QueryActors {}).await?.results)
    }

    pub async fn get_providers(&self) -> Result<Vec<String>> {
        let b = MessageBus::from_hostlocal_registry(&self.id.borrow());
        Ok(b.send(QueryProviders {}).await?.results)
    }

    pub async fn call_actor(&self, actor: &str, operation: &str, msg: &[u8]) -> Result<Vec<u8>> {
        let inv = Invocation::new(
            self.kp.borrow().as_ref().unwrap(),
            WasccEntity::Actor(SYSTEM_ACTOR.to_string()),
            WasccEntity::Actor(actor.to_string()),
            operation,
            msg.to_vec(),
        );
        let b = MessageBus::from_hostlocal_registry(&self.id.borrow());
        let ir: InvocationResponse = b.send(inv).await?;

        if let Some(e) = ir.error {
            Err(format!("Invocation failure: {}", e).into())
        } else {
            Ok(ir.msg)
        }
    }

    pub async fn set_link(
        &self,
        actor: &str,
        contract_id: &str,
        link_name: Option<String>,
        provider_id: String,
        values: HashMap<String, String>,
    ) -> Result<()> {
        let bus = MessageBus::from_hostlocal_registry(&self.id.borrow());
        bus.send(AdvertiseLink {
            contract_id: contract_id.to_string(),
            actor: actor.to_string(),
            link_name: link_name.unwrap_or("default".to_string()),
            provider_id,
            values,
        })
        .await?
    }

    pub async fn apply_manifest(&self, manifest: HostManifest) -> Result<()> {
        let host_id = self.kp.borrow().as_ref().unwrap().public_key();
        let hc = HostController::from_hostlocal_registry(&host_id);
        let bus = MessageBus::from_hostlocal_registry(&host_id);

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
        for msg in crate::manifest::generate_adv_link_messages(&manifest).await {
            let _ = bus.send(msg).await?;
        }

        Ok(())
    }

    pub(crate) fn native_target() -> String {
        format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
    }
}
