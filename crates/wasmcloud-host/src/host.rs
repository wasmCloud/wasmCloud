use crate::{
    actors::LiveUpdate,
    host_controller::GetRunningActor,
    messagebus::{
        AdvertiseLink, AdvertiseRemoveLink, GetClaims, LinkDefinition, MessageBus, QueryAllLinks,
        QueryOciReferences,
    },
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
use std::{collections::HashMap, sync::RwLock};
use wascap::prelude::{Claims, KeyPair};

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
    /// disabled by default. For security reasons, you should not use the same NATS connection or
    /// security context for both RPC and lattice control.
    pub fn with_control_client(self, client: nats::asynk::Connection) -> HostBuilder {
        HostBuilder {
            cplane_client: Some(client),
            ..self
        }
    }

    /// Provides an additional layer of runtime security by providing the host with an
    /// instance of an implementor of the [`Authorizer`](trait@Authorizer) trait.
    pub fn with_authorizer(self, authorizer: impl Authorizer + 'static) -> HostBuilder {
        HostBuilder {
            authorizer: Box::new(authorizer),
            ..self
        }
    }

    /// When running with lattice enabled, a namespace prefix can be provided to allow
    /// multiple lattices to co-exist within the same account space on a NATS server. This
    /// allows for multi-tenancy but can be a security risk if configured incorrectly. If you do
    /// not supply a namespace, the string `default` will be used.
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
    /// functionality.
    pub fn oci_allow_latest(self) -> HostBuilder {
        HostBuilder {
            allow_latest: true,
            ..self
        }
    }

    /// Allows the host to pull actor and capability provider images from the supplied list
    /// of valid OCI registries without using a secure (SSL/TLS) connection. This option is empty by default and we recommend
    /// it not be used in production environments. For local testing, make sure you supply the right
    /// host name and port number for the locally running OCI registry.
    pub fn oci_allow_insecure(self, allowed_insecure: Vec<String>) -> HostBuilder {
        HostBuilder {
            allowed_insecure,
            ..self
        }
    }

    /// Adds a custom label and value pair to the host. Label-value pairs are used during
    /// scheduler auctions to determine if a host is compatible with a given scheduling request.
    /// All hosts automatically come with the following built-in system labels: `hostcore.arch`,
    /// `hostcore.os`, `hostcore.osfamily` and you cannot provide your own overridding values
    /// for those.
    pub fn with_label(self, key: &str, value: &str) -> HostBuilder {
        let mut hm = self.labels.clone();
        if !hm.contains_key(key) {
            hm.insert(key.to_string(), value.to_string());
        }
        HostBuilder { labels: hm, ..self }
    }

    /// Constructs an instance of a wasmCloud host. Note that this will not _start_ the host. You
    /// will need to invoke the [`start`](fn@Host::start) function after building a new host.
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
            started: RwLock::new(false),
        }
    }
}

/// A wasmCloud host is a secure runtime responsible for scheduling
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
    started: RwLock<bool>,
}

impl Host {
    /// Starts the host's actor system. This call is non-blocking, so it is up to the consumer
    /// to provide some form of parking or waiting (e.g. wait for a Ctrl-C signal).
    ///
    /// # Examples
    ///
    /// ```
    /// # use log::{info, error};    
    /// # #[actix_rt::main]
    /// # async fn main() {    
    /// # let host = wasmcloud_host::HostBuilder::new().build();
    /// match host.start().await {
    ///    Ok(_) => {    
    ///     // Await a ctrl-c, e.g. actix_rt::signal::ctrl_c().await.unwrap();
    ///     info!("Ctrl-C received, shutting down");
    ///     host.stop().await;
    ///    }
    ///    Err(e) => {
    ///     error!("Failed to start host: {}", e);
    ///    }
    ///}
    ///# }
    /// ```    
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
    /// anti-forgery checks. For more information on the PKI used by wasmCloud, see the documentation for
    /// the [nkeys](https://crates.io/crates/nkeys) crate.
    pub fn id(&self) -> String {
        self.id.to_string()
    }

    /// Starts a native capability provider and preps it for execution in the host.
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
    /// purge the OCI image cache, remove the `wasmcloudcache` and `wasmcloud_ocicache` directories
    /// from the appropriate root folder (which defaults to the environment's `TEMP` directory).
    ///
    /// # Arguments
    /// * `cap_ref` - The OCI reference URL of the capability provider.
    /// * `link_name` - The link name to be used for this capability provider.
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

    /// Instructs the runtime host to start an actor. Note that starting an actor can trigger a
    /// provider initialization if a link definition for this actor already exists in the lattice
    /// cache.
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
    /// start the actor. This call will fail if the host cannot communicate with, fails to
    /// authenticate against, or cannot finish downloading the indicated OCI image.
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
    /// in errors or warnings in log output).
    ///
    /// * `actor_ref` - Either the public key or the OCI reference URL of the actor to stop.
    pub async fn stop_actor(&self, actor_ref: &str) -> Result<()> {
        self.ensure_started()?;
        let hc = HostController::from_hostlocal_registry(&self.id);
        hc.send(StopActor {
            actor_ref: actor_ref.to_string(),
        })
        .await?;

        Ok(())
    }

    /// Updates a running actor with the bytes from a new actor module. Invoking this method will
    /// cause all pending messages inbound to the actor to block while the update is performed. It can take a few
    /// seconds depending on the size of the actor and the underlying engine you're using (e.g. JIT vs. interpreter).
    ///
    /// # Arguments
    /// * `actor_id` - The public key of the actor to update.
    /// * `new_oci_ref` - If applicable, a new OCI reference URL that corresponds to the new version.
    /// * `bytes` - The raw bytes containing the new WebAssembly module.
    ///
    /// # ⚠️ Caveats
    /// * Take care when supplying a value for the `new_oci_ref` parameter. You should be consistent
    /// in how you supply this value. Updating actors that were started from OCI references should
    /// continue to have OCI references, while those started from non-OCI sources should not be given
    /// new, arbitrary OCI references. Failing to keep this consistent could cause unforeseen failed
    /// attempts at subsequent updates.  
    /// * If the new version of the actor is actually in an OCI registry, then the preferred method of
    /// performing a live update is to do so through the lattice control interface and specifying the OCI
    /// URL. This method is less error-prone and can cause less confusion over time.        
    pub async fn update_actor(
        &self,
        actor_id: &str,
        new_oci_ref: Option<String>,
        bytes: &[u8],
    ) -> Result<()> {
        let hc = HostController::from_hostlocal_registry(&self.id);
        let actor = hc
            .send(GetRunningActor {
                actor_id: actor_id.to_string(),
            })
            .await?;
        if let Some(a) = actor {
            a.send(LiveUpdate {
                actor_bytes: bytes.to_vec(),
                image_ref: new_oci_ref,
            })
            .await?
        } else {
            Err(format!(
                "Actor {} not found on this host, live update aborted.",
                actor_id
            )
            .into())
        }
    }

    /// Stops a running capability provider. This call will not fail if the indicated provider
    /// is not currently running. This call is non-blocking and does not wait for the provider
    /// to finish cleaning up its resources before returning.
    ///
    /// # Arguments
    /// * `provider_ref` - The capability provider's public key or OCI reference URL
    /// * `contract_id` - The contract ID of the provider to stop
    /// * `link` - The link name used by the instance of the capability provider
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
    pub async fn actors(&self) -> Result<Vec<String>> {
        self.ensure_started()?;
        let b = MessageBus::from_hostlocal_registry(&self.id);
        Ok(b.send(QueryActors {})
            .await?
            .results
            .iter()
            .filter_map(|e| match e {
                WasmCloudEntity::Actor(s) => Some(s.clone()),
                _ => None,
            })
            .collect())
    }

    /// Retrieves the list of the uniquely identifying information about all capability providers running in the host.
    /// This function does _not_ include any capability providers that may be remotely running in a connected
    /// lattice. The return value is a vector of 3-tuples `(provider_public_key, contract_id, link_name)`
    pub async fn providers(&self) -> Result<Vec<(String, String, String)>> {
        self.ensure_started()?;
        let b = MessageBus::from_hostlocal_registry(&self.id);
        Ok(b.send(QueryProviders {})
            .await?
            .results
            .iter()
            .filter_map(|e| match e {
                WasmCloudEntity::Capability {
                    contract_id,
                    id,
                    link_name,
                } => Some((
                    id.to_string(),
                    contract_id.to_string(),
                    link_name.to_string(),
                )),
                _ => None,
            })
            .collect())
    }

    /// Perform a raw [waPC](https://crates.io/crates/wapc)-style invocation on the given actor by supplying an operation
    /// string and a payload of raw bytes, resulting in a payload of raw response bytes. It
    /// is entirely up to the actor as to how it responds to unrecognized operations. This
    /// operation will also be checked against the authorization system if a custom
    /// authorization plugin has been supplied to this host. This call will fail if you attempt to
    /// invoke a non-existent actor or call alias or the target actor cannot be reached.
    ///
    /// # Arguments
    /// * `actor` - The public key of the actor or its call alias if applicable.
    /// * `operation` - The name of the operation to perform
    /// * `msg` - The raw bytes containing the payload pertaining to this operation.
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

    /// Links are a first-class, durable entity within a lattice and host runtime. A link
    /// defines a set of configuration values that apply to an actor and a capability provider that
    /// is uniquely keyed by the provider's contract ID, public key, and link name.
    ///
    /// You can set a link before or after either the actor or provider are started. Links are automatically established
    /// when both parties are present in a lattice, and re-established if a party temporarily
    /// leaves the lattice and rejoins (which can happen during a crash or a network partition event). The resiliency and
    /// reliability of link definitions within a lattice are inherited from your choice of lattice cache provider.
    ///
    /// # Arguments
    /// * `actor` - Can either be the actor's public key _or_ an OCI reference URL.
    /// * `contract_id` - The contract ID of the link, e.g. `wasmcloud:httpserver`.
    /// * `link_name` - The link name of the capability provider when it was loaded.
    /// * `provider_id` - The public key of the capability provider (e.g. `Vxxxx`)
    /// * `values` - The set of configuration values to give to the capability provider for this link definition.
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

    /// Removes a link definition from the host (and accompanying lattice if connected). This
    /// will remove the link definition from the lattice cache and it will also invoke the "remove actor"
    /// operation on all affected capability providers, giving those providers an opportunity to clean
    /// up resources and terminate any child processes associated with the actor. This call does not
    /// wait for all providers to finish cleaning up associated resources.
    ///
    /// # Arguments
    /// * `actor` - The **public key** of the actor. You cannot supply an OCI reference when removing a link.
    /// * `contract_id` - The contract ID of the capability in question.
    /// * `link_name` - The name of the link used by the capability provider for which the link is to be removed.
    pub async fn remove_link(
        &self,
        actor: &str,
        contract_id: &str,
        link_name: Option<String>,
    ) -> Result<()> {
        self.ensure_started()?;
        let bus = MessageBus::from_hostlocal_registry(&self.id);
        bus.send(AdvertiseRemoveLink {
            actor: actor.to_string(),
            contract_id: contract_id.to_string(),
            link_name: link_name.unwrap_or_else(|| "default".to_string()),
        })
        .await?
    }

    /// Apply a number of declarations from a manifest file to the runtime host. A manifest
    /// file can contain a list of actors, capability providers, custom labels, and link definitions that will
    /// be added to a host upon application. Manifest application is _not_ idempotent, so
    /// repeated application of multiple manifests will almost certainly not result in the same host state.
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

    /// Retrieves the list of all actor claims known to this host. Note that this list is
    /// essentially a grow-only map maintained by the distributed lattice cache. As a result,
    /// the return value of this function contains a list of all claims as seen _since the lattice began_.
    /// It is quite likely that this list can contain references to actors that are no longer in
    /// the lattice. When the host is operating in single-player mode, this list naturally only indicates
    /// actors that have been started since the host was initialized.
    pub async fn actor_claims(&self) -> Result<Vec<Claims<wascap::jwt::Actor>>> {
        self.ensure_started()?;
        let bus = MessageBus::from_hostlocal_registry(&self.kp.public_key());
        let res = bus.send(GetClaims {}).await?;
        Ok(res.claims.into_iter().map(|(_, v)| v).collect())
    }

    /// Retrieves the list of host labels. Some of these labels are automatically populated by the host
    /// at start-time, such as OS family and CPU, and others are manually supplied to the host at build-time
    /// or through a manifest to define custom scheduling rules.
    pub async fn labels(&self) -> HashMap<String, String> {
        self.labels.clone()
    }

    /// Retrieves the list of link definitions as known by the distributed lattice cache. If the host is
    /// in single-player mode, this cache is limited to just that host. In lattice mode, this cache reflects
    /// all of the current link definitions.
    pub async fn link_definitions(&self) -> Result<Vec<LinkDefinition>> {
        self.ensure_started()?;
        let bus = MessageBus::from_hostlocal_registry(&self.kp.public_key());
        let res = bus.send(QueryAllLinks {}).await?;
        Ok(res.links)
    }

    /// Obtains the list of all known OCI references. As with other lattice cache data, this can be thought
    /// of as a grow-only map that can contain references for actors or providers that may no longer be present
    /// within a lattice or host. The returned map's `key` is the OCI reference URL of the entity, and the value
    /// is the public key of that entity, where `Mxxx` are actors and `Vxxx` are capability providers.
    pub async fn oci_references(&self) -> Result<HashMap<String, String>> {
        self.ensure_started()?;
        let bus = MessageBus::from_hostlocal_registry(&self.kp.public_key());
        let res = bus.send(QueryOciReferences {}).await?;
        Ok(res)
    }

    fn ensure_started(&self) -> Result<()> {
        if !*self.started.read().unwrap() {
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
