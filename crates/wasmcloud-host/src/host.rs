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
use crate::{ControlEvent, HostManifest, NativeCapability, WasmCloudEntity};
use crate::{Result, SYSTEM_ACTOR};
use provider_archive::ProviderArchive;
use std::time::Duration;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use wascap::prelude::KeyPair;

/// A host builder provides a convenient, fluid syntax for setting initial configuration
/// and tuning parameters for a wasmCloud host
pub struct HostBuilder {
    labels: HashMap<String, String>,
    authorizer: Box<dyn Authorizer + 'static>,
    namespace: String,
    rpc_timeout: Duration,
    allow_latest: bool,
    allowed_insecure: Vec<String>,
    rpc_client: Option<nats::asynk::Connection>,
    cplane_client: Option<nats::asynk::Connection>,
    allow_live_update: bool,
    lattice_cache_provider_ref: Option<String>,
    strict_update_check: bool,
}

impl Default for HostBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl HostBuilder {
    /// Creates a new host builder
    pub fn new() -> HostBuilder {
        HostBuilder {
            labels: crate::host_controller::detect_core_host_labels(),
            authorizer: Box::new(crate::auth::DefaultAuthorizer::new()),
            allow_latest: false,
            allowed_insecure: vec![],
            namespace: "default".to_string(),
            rpc_timeout: Duration::from_secs(2),
            rpc_client: None,
            cplane_client: None,
            allow_live_update: false,
            lattice_cache_provider_ref: None,
            strict_update_check: true,
        }
    }

    /// Indicates that live-updating (hot swapping) of actors at runtime is allowed. The default is to deny
    pub fn enable_live_updates(self) -> HostBuilder {
        HostBuilder {
            allow_live_update: true,
            ..self
        }
    }

    /// Disables strict update checks at runtime. Strict update checks require that the replacement
    /// actor's claims must match _exactly_ the claims of the actor being replaced during update. Strict
    /// updates are enabled by default
    pub fn disable_strict_update_check(self) -> HostBuilder {
        HostBuilder {
            strict_update_check: false,
            ..self
        }
    }

    /// Build the host with an RPC client by providing an instance of a NATS connection. The presence
    /// of an RPC client will automatically enable lattice clustering. RPC is off by default
    pub fn with_rpc_client(self, client: nats::asynk::Connection) -> HostBuilder {
        HostBuilder {
            rpc_client: Some(client),
            ..self
        }
    }

    /// Enables remote control of the host via the lattice control protocol by providing an instance
    /// of a NATS connection through which control commands flow. Remote control of hosts is
    /// disabled by default.
    pub fn with_control_client(self, client: nats::asynk::Connection) -> HostBuilder {
        HostBuilder {
            cplane_client: Some(client),
            ..self
        }
    }

    /// Provides an additional layer of runtime security by providing the host with an
    /// instance of an implementor of the [crate::Authorizer] trait.
    pub fn with_authorizer(self, authorizer: impl Authorizer + 'static) -> HostBuilder {
        HostBuilder {
            authorizer: Box::new(authorizer),
            ..self
        }
    }

    /// When running with lattice enabled, a namespace prefix can be provided to allow
    /// multiple lattices to co-exist within the same account space on a NATS server. This
    /// allows for multi-tenancy but can be a security risk if configured incorrectly
    pub fn with_namespace(self, namespace: &str) -> HostBuilder {
        HostBuilder {
            namespace: namespace.to_string(),
            ..self
        }
    }

    /// Sets the timeout of default RPC invocations across the lattice. This value only
    /// carries meaning if an RPC client has been supplied
    pub fn with_rpc_timeout(self, rpc_timeout: Duration) -> HostBuilder {
        HostBuilder {
            rpc_timeout,
            ..self
        }
    }

    /// Overrides the default lattice cache provider with the key-value capability
    /// provider indicated by the OCI image reference. Note that your host must
    /// have connectivity to and authorization for the OCI URL provided or the host
    /// will not start
    pub fn with_lattice_cache_provider(self, provider_ref: &str) -> HostBuilder {
        HostBuilder {
            lattice_cache_provider_ref: Some(provider_ref.to_string()),
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

    /// Allows the host to pull actor and capability provider images from these registries without
    /// using a secure (SSL/TLS) connection. This option is empty by default and we recommend
    /// it not be used in production environments. For local testing, supplying ["localhost:5000"]
    /// as an argument for the local docker reigstry will allow for http connections to that registry.
    pub fn oci_allow_insecure(self, allowed_insecure: Vec<String>) -> HostBuilder {
        HostBuilder {
            allowed_insecure,
            ..self
        }
    }

    /// Adds a custom label and value pair to the host. Label-value pairs are used during
    /// scheduler auctions to determine if a host is compatible with a given scheduling request.
    /// All hosts automatically come with the following built-in system labels: `hostcore.arch`,
    /// `hostcore.os`, `hostcore.osfamily`
    pub fn with_label(self, key: &str, value: &str) -> HostBuilder {
        let mut hm = self.labels.clone();
        if !hm.contains_key(key) {
            hm.insert(key.to_string(), value.to_string());
        }
        HostBuilder { labels: hm, ..self }
    }

    /// Constructs an instance of a wasmCloud host. Note that this will not _start_ the host. You
    /// will need to invoke the `start` function after building a new host
    pub fn build(self) -> Host {
        let kp = KeyPair::new_server();
        Host {
            labels: self.labels,
            authorizer: self.authorizer,
            id: kp.public_key(),
            allow_latest: self.allow_latest,
            allowed_insecure: self.allowed_insecure,
            kp,
            rpc_timeout: self.rpc_timeout,
            namespace: self.namespace,
            rpc_client: self.rpc_client,
            cplane_client: self.cplane_client,
            allow_live_updates: self.allow_live_update,
            lattice_cache_provider_ref: self.lattice_cache_provider_ref,
            strict_update_check: self.strict_update_check,
            started: Arc::new(RwLock::new(false)),
        }
    }
}

/// A wasmCloud `Host` is a secure runtime container responsible for scheduling
/// actors and capability providers, configuring the links between them, and facilitating
/// secure function call dispatch between actors and capabilities.
pub struct Host {
    labels: HashMap<String, String>,
    authorizer: Box<dyn Authorizer + 'static>,
    id: String,
    allow_latest: bool,
    allowed_insecure: Vec<String>,
    kp: KeyPair,
    namespace: String,
    rpc_timeout: Duration,
    cplane_client: Option<nats::asynk::Connection>,
    rpc_client: Option<nats::asynk::Connection>,
    allow_live_updates: bool,
    lattice_cache_provider_ref: Option<String>,
    strict_update_check: bool,
    started: Arc<RwLock<bool>>,
}

impl Host {
    /// Starts the host's actor system. This call is non-blocking, so it is up to the consumer
    /// to provide some form of parking or waiting (e.g. wait for a Ctrl-C signal).
    pub async fn start(&self) -> Result<()> {
        let mb = MessageBus::from_hostlocal_registry(&self.kp.public_key());
        let init = crate::messagebus::Initialize {
            nc: self.rpc_client.clone(),
            namespace: Some(self.namespace.to_string()),
            key: KeyPair::from_seed(&self.kp.seed()?)?,
            auth: self.authorizer.clone(),
            rpc_timeout: self.rpc_timeout,
        };
        mb.send(init).await?;

        let hc = HostController::from_hostlocal_registry(&self.kp.public_key());
        hc.send(crate::host_controller::Initialize {
            labels: self.labels.clone(),
            auth: self.authorizer.clone(),
            kp: KeyPair::from_seed(&self.kp.seed()?)?,
            allow_live_updates: self.allow_live_updates,
            allow_latest: self.allow_latest,
            allowed_insecure: self.allowed_insecure.clone(),
            lattice_cache_provider: self.lattice_cache_provider_ref.clone(),
            strict_update_check: self.strict_update_check,
        })
        .await?;

        // Start control interface
        let cp = ControlInterface::from_hostlocal_registry(&self.kp.public_key());
        cp.send(crate::control_interface::ctlactor::Initialize {
            client: self.cplane_client.clone(),
            control_options: ControlOptions {
                host_labels: self.labels.clone(),
                oci_allow_latest: self.allow_latest,
                oci_allowed_insecure: self.allowed_insecure.clone(),
                ..Default::default()
            },
            key: KeyPair::from_seed(&self.kp.seed()?)?,
            ns_prefix: self.namespace.to_string(),
        })
        .await?;

        *self.started.write().unwrap() = true;

        let _ = cp
            .send(PublishEvent {
                event: ControlEvent::HostStarted,
            })
            .await;

        Ok(())
    }

    /// Stops a running host. Be aware that this function may terminate before the host has
    /// finished disposing of all of its resources.
    pub async fn stop(&self) {
        let cp = ControlInterface::from_hostlocal_registry(&self.id);
        let _ = cp
            .send(PublishEvent {
                event: ControlEvent::HostStopped,
            })
            .await;
        *self.started.write().unwrap() = false;
        System::current().stop();
    }

    /// Returns the unique public key (a 56-character upppercase string beginning with the letter `N`) of this host.
    /// The host's private key is used to securely sign invocations so that remote hosts can perform
    /// anti-forgery checks
    pub fn id(&self) -> String {
        self.id.to_string()
    }

    /// Starts a native (non-portable dynamically linked library plugin) capability provider and preps
    /// it for execution in the host
    pub async fn start_native_capability(&self, capability: crate::NativeCapability) -> Result<()> {
        self.ensure_started()?;
        let hc = HostController::from_hostlocal_registry(&self.id);
        let _ = hc
            .send(StartProvider {
                provider: capability,
                image_ref: None,
            })
            .await??;

        Ok(())
    }

    /// Instructs the host to download a capability provider from an OCI registry and start
    /// it. The wasmCloud host caches binary images retrieved from OCI registries (because
    /// images are assumed to be immutable this kind of caching is acceptable). If you need to
    /// purge the OCI image cache, remove the `wasmcloud_cache` directory from your environment's
    /// `TEMP` directory.
    pub async fn start_capability_from_registry(
        &self,
        cap_ref: &str,
        link_name: Option<String>,
    ) -> Result<()> {
        self.ensure_started()?;
        let hc = HostController::from_hostlocal_registry(&self.id);
        let bytes = fetch_oci_bytes(cap_ref, self.allow_latest, &self.allowed_insecure).await?;
        let par = ProviderArchive::try_load(&bytes)?;
        let nc = NativeCapability::from_archive(&par, link_name)?;
        hc.send(StartProvider {
            provider: nc,
            image_ref: Some(cap_ref.to_string()),
        })
        .await??;
        Ok(())
    }

    /// Instructs the runtime host to start an actor.
    pub async fn start_actor(&self, actor: crate::Actor) -> Result<()> {
        self.ensure_started()?;
        let hc = HostController::from_hostlocal_registry(&self.id);

        hc.send(StartActor {
            actor,
            image_ref: None,
        })
        .await??;
        Ok(())
    }

    /// Instructs the runtime host to download the actor from the indicated OCI registry and
    /// start the actor. This call will fail if the host cannot communicate with or finish
    /// downloading the indicated OCI image
    pub async fn start_actor_from_registry(&self, actor_ref: &str) -> Result<()> {
        self.ensure_started()?;
        let hc = HostController::from_hostlocal_registry(&self.id);
        let bytes = fetch_oci_bytes(actor_ref, self.allow_latest, &self.allowed_insecure).await?;
        let actor = crate::Actor::from_slice(&bytes)?;
        hc.send(StartActor {
            actor,
            image_ref: Some(actor_ref.to_string()),
        })
        .await??;
        Ok(())
    }

    /// Stops a running actor in the host. This call is assumed to be idempotent and as such will
    /// not fail if you attempt to stop an actor that is not running (though this may result
    /// in errors or warnings in log output)
    pub async fn stop_actor(&self, actor_ref: &str) -> Result<()> {
        self.ensure_started()?;
        let hc = HostController::from_hostlocal_registry(&self.id);
        hc.send(StopActor {
            actor_ref: actor_ref.to_string(),
        })
        .await?;

        Ok(())
    }

    /// Stops a running capability provider. This call will not fail if the indicated provider
    /// is not currently running, and this call may terminate before the provider has finished
    /// shutting down cleanly
    pub async fn stop_provider(
        &self,
        provider_ref: &str,
        contract_id: &str,
        link: Option<String>,
    ) -> Result<()> {
        self.ensure_started()?;
        let hc = HostController::from_hostlocal_registry(&self.id);
        let link_name = link.unwrap_or_else(|| "default".to_string());
        hc.send(StopProvider {
            provider_ref: provider_ref.to_string(),
            contract_id: contract_id.to_string(),
            link_name,
        })
        .await?;
        Ok(())
    }

    /// Retrieves the list of all actors within this host. This function call does _not_
    /// include any actors remotely running in a connected lattice
    pub async fn get_actors(&self) -> Result<Vec<String>> {
        self.ensure_started()?;
        let b = MessageBus::from_hostlocal_registry(&self.id);
        Ok(b.send(QueryActors {}).await?.results)
    }

    /// Retrieves the list of capability providers running in the host. This function does
    /// _not_ include any capability providers that may be remotely running in a connected
    /// lattice
    pub async fn get_providers(&self) -> Result<Vec<String>> {
        self.ensure_started()?;
        let b = MessageBus::from_hostlocal_registry(&self.id);
        Ok(b.send(QueryProviders {}).await?.results)
    }

    /// Perform a raw waPC-style invocation on the given actor by supplying an operation
    /// string and a payload of raw bytes, resulting in a payload of raw response bytes. It
    /// is entirely up to the actor as to how it responds to unrecognized operations. This
    /// operation will also be checked against the authorization system if a custom
    /// authorization plugin has been supplied to this host. You may supply either the actor's
    /// public key or the actor's registered call alias. This call will fail if you attempt to
    /// invoke a non-existent actor or call alias.
    pub async fn call_actor(&self, actor: &str, operation: &str, msg: &[u8]) -> Result<Vec<u8>> {
        self.ensure_started()?;
        let b = MessageBus::from_hostlocal_registry(&self.id);
        let target = if actor.len() == 56 && actor.starts_with('M') {
            WasmCloudEntity::Actor(actor.to_string())
        } else if let Some(pk) = crate::dispatch::lookup_call_alias(&b, actor).await {
            WasmCloudEntity::Actor(pk)
        } else {
            return Err("Specified actor was not a public key or a known call alias.".into());
        };

        let inv = Invocation::new(
            &self.kp,
            WasmCloudEntity::Actor(SYSTEM_ACTOR.to_string()),
            target,
            operation,
            msg.to_vec(),
        );
        let b = MessageBus::from_hostlocal_registry(&self.id);
        let ir: InvocationResponse = b.send(inv).await?;

        if let Some(e) = ir.error {
            Err(format!("Invocation failure: {}", e).into())
        } else {
            Ok(ir.msg)
        }
    }

    /// Links are a self-standing, durable entity within a lattice and host runtime. A link
    /// defines a set of configuration values that apply to an actor and a capability provider indicated
    /// by the provider's contract ID, public key, and link name. You can set a link before or
    /// after either the actor or provider are started. Links are automatically established
    /// when both parties are present in a lattice, and re-established if a party temporarily
    /// leaves the lattice and rejoins (which can happen during a crash or a partition event). Link
    /// data is exactly as durable as your choice of lattice cache provider
    pub async fn set_link(
        &self,
        actor: &str,
        contract_id: &str,
        link_name: Option<String>,
        provider_id: String,
        values: HashMap<String, String>,
    ) -> Result<()> {
        self.ensure_started()?;
        let bus = MessageBus::from_hostlocal_registry(&self.id);
        bus.send(AdvertiseLink {
            contract_id: contract_id.to_string(),
            actor: actor.to_string(),
            link_name: link_name.unwrap_or_else(|| "default".to_string()),
            provider_id,
            values,
        })
        .await?
    }

    /// Apply a number of instructions from a manifest file to the runtime host. A manifest
    /// file can contain a list of actors, capability providers, and link definitions that will
    /// be added to a host upon ingestion. Manifest application is _not_ idempotent, so
    /// repeated application of multiple manifests may not always produce the same runtime
    /// host state
    pub async fn apply_manifest(&self, manifest: HostManifest) -> Result<()> {
        self.ensure_started()?;
        let host_id = self.kp.public_key();
        let hc = HostController::from_hostlocal_registry(&host_id);
        let bus = MessageBus::from_hostlocal_registry(&host_id);

        if !manifest.labels.is_empty() {
            let mut labels = manifest.labels.clone();
            // getting an iterator of this const produces `&&str` which is a super annoying type
            #[allow(clippy::needless_range_loop)]
            for x in 0..RESTRICTED_LABELS.len() {
                labels.remove(RESTRICTED_LABELS[x]);
            }
            hc.send(SetLabels { labels }).await?;
        }

        for msg in crate::manifest::generate_actor_start_messages(
            &manifest,
            self.allow_latest,
            &self.allowed_insecure,
        )
        .await
        {
            let _ = hc.send(msg).await?;
        }
        for msg in crate::manifest::generate_provider_start_messages(
            &manifest,
            self.allow_latest,
            &self.allowed_insecure,
        )
        .await
        {
            let _ = hc.send(msg).await?;
        }
        for msg in crate::manifest::generate_adv_link_messages(&manifest).await {
            debug!(
                "Advertising {}:{}:{}",
                msg.actor, msg.link_name, msg.provider_id
            );
            let _ = bus.send(msg).await?;
        }

        Ok(())
    }

    fn ensure_started(&self) -> Result<()> {
        if *self.started.read().unwrap() == false {
            return Err("Activity cannot be performed, host has not been started".into());
        }
        if System::try_current().is_none() {
            return Err("No actix rt system is running. Cannot perform host activity.".into());
        }
        Ok(())
    }

    pub(crate) fn native_target() -> String {
        format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
    }
}

#[cfg(test)]
mod test {
    use crate::HostBuilder;

    #[test]
    fn is_send() {
        let h = HostBuilder::new().build();
        assert_is_send(h);
    }

    fn assert_is_send<T: Send>(_input: T) {}
}
