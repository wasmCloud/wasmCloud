//! Host interactions with JetStream, including processing of KV entries and
//! storing/retrieving component specifications.

use anyhow::{anyhow, ensure, Context as _};
use async_nats::jetstream::kv::{Entry as KvEntry, Operation, Store};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, instrument, warn};
use wasmcloud_control_interface::Link;

use crate::wasmbus::claims::{Claims, StoredClaims};
use crate::wasmbus::component_import_links;

#[derive(Debug, Serialize, Deserialize, Default)]
/// The specification of a component that is or did run in the lattice. This contains all of the information necessary to
/// instantiate a component in the lattice (url and digest) as well as configuration and links in order to facilitate
/// runtime execution of the component. Each `import` in a component's WIT world will need a corresponding link for the
/// host runtime to route messages to the correct component.
pub struct ComponentSpecification {
    /// The URL of the component, file, OCI, or otherwise
    pub(crate) url: String,
    /// All outbound links from this component to other components, used for routing when calling a component `import`
    #[serde(default)]
    pub(crate) links: Vec<Link>,
    ////
    // Possible additions in the future, left in as comments to facilitate discussion
    ////
    // /// The claims embedded in the component, if present
    // claims: Option<Claims>,
    // /// SHA256 digest of the component, used for checking uniqueness of component IDs
    // digest: String
    // /// (Advanced) Additional routing topics to subscribe on in addition to the component ID.
    // routing_groups: Vec<String>,
}

impl ComponentSpecification {
    /// Create a new empty component specification with the given ID and URL
    pub fn new(url: impl AsRef<str>) -> Self {
        Self {
            url: url.as_ref().to_string(),
            links: Vec::new(),
        }
    }
}

impl super::Host {
    /// Retrieve a component specification based on the provided ID. The outer Result is for errors
    /// accessing the store, and the inner option indicates if the spec exists.
    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn get_component_spec(
        &self,
        id: &str,
    ) -> anyhow::Result<Option<ComponentSpecification>> {
        let key = format!("COMPONENT_{id}");
        let spec = self
            .data
            .get(key)
            .await
            .context("failed to get component spec")?
            .map(|spec_bytes| serde_json::from_slice(&spec_bytes))
            .transpose()
            .context(format!(
                "failed to deserialize stored component specification for {id}"
            ))?;
        Ok(spec)
    }

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn store_component_spec(
        &self,
        id: impl AsRef<str>,
        spec: &ComponentSpecification,
    ) -> anyhow::Result<()> {
        let id = id.as_ref();
        let key = format!("COMPONENT_{id}");
        let bytes = serde_json::to_vec(spec)
            .context("failed to serialize component spec")?
            .into();
        self.data
            .put(key, bytes)
            .await
            .context("failed to put component spec")?;
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn process_component_spec_put(
        &self,
        id: impl AsRef<str>,
        value: impl AsRef<[u8]>,
        _publish: bool,
    ) -> anyhow::Result<()> {
        let id = id.as_ref();
        debug!(id, "process component spec put");

        let spec: ComponentSpecification = serde_json::from_slice(value.as_ref())
            .context("failed to deserialize component specification")?;

        // Compute all new links that do not exist in the host map, which we'll use to
        // publish to any running providers that are the source or target of the link.
        // Computing this ahead of time is a tradeoff to hold only one lock at the cost of
        // allocating an extra Vec. This may be a good place to optimize allocations.
        let new_links = {
            let all_links = self.links.read().await;
            spec.links
                .iter()
                .filter(|spec_link| {
                    // Retain only links that do not exist in the host map
                    !all_links
                        .iter()
                        .filter_map(|(source_id, links)| {
                            // Only consider links that are either the source or target of the new link
                            if source_id == spec_link.source_id() || source_id == spec_link.target()
                            {
                                Some(links)
                            } else {
                                None
                            }
                        })
                        .flatten()
                        .any(|host_link| *spec_link == host_link)
                })
                .collect::<Vec<_>>()
        };

        {
            // Acquire lock once in this block to avoid continually trying to acquire it.
            let providers = self.providers.read().await;
            // For every new link, if a provider is running on this host as the source or target,
            // send the link to the provider for handling based on the xkey public key.
            for link in new_links {
                if let Some(provider) = providers.get(link.source_id()) {
                    if let Err(e) = self.put_provider_link(provider, link).await {
                        error!(?e, "failed to put provider link");
                    }
                }
                if let Some(provider) = providers.get(link.target()) {
                    if let Err(e) = self.put_provider_link(provider, link).await {
                        error!(?e, "failed to put provider link");
                    }
                }
            }
        }

        // If the component is already running, update the links
        if let Some(component) = self.components.write().await.get(id) {
            *component.handler.instance_links.write().await = component_import_links(&spec.links);
            // NOTE(brooksmtownsend): We can consider updating the component if the image URL changes
        };

        // Insert the links into host map
        self.links.write().await.insert(id.to_string(), spec.links);

        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn process_component_spec_delete(
        &self,
        id: impl AsRef<str>,
        _value: impl AsRef<[u8]>,
        _publish: bool,
    ) -> anyhow::Result<()> {
        let id = id.as_ref();
        debug!(id, "process component delete");
        // TODO: TBD: stop component if spec deleted?
        if self.components.write().await.get(id).is_some() {
            warn!(
                component_id = id,
                "component spec deleted, but component is still running"
            );
        }
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn process_claims_put(
        &self,
        pubkey: impl AsRef<str>,
        value: impl AsRef<[u8]>,
    ) -> anyhow::Result<()> {
        let pubkey = pubkey.as_ref();

        debug!(pubkey, "process claim entry put");

        let stored_claims: StoredClaims =
            serde_json::from_slice(value.as_ref()).context("failed to decode stored claims")?;
        let claims = Claims::from(stored_claims);

        ensure!(claims.subject() == pubkey, "subject mismatch");
        match claims {
            Claims::Component(claims) => self.store_component_claims(claims).await,
            Claims::Provider(claims) => {
                let mut provider_claims = self.provider_claims.write().await;
                provider_claims.insert(claims.subject.clone(), claims);
                Ok(())
            }
        }
    }

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn process_claims_delete(
        &self,
        pubkey: impl AsRef<str>,
        value: impl AsRef<[u8]>,
    ) -> anyhow::Result<()> {
        let pubkey = pubkey.as_ref();

        debug!(pubkey, "process claim entry deletion");

        let stored_claims: StoredClaims =
            serde_json::from_slice(value.as_ref()).context("failed to decode stored claims")?;
        let claims = Claims::from(stored_claims);

        ensure!(claims.subject() == pubkey, "subject mismatch");

        match claims {
            Claims::Component(claims) => {
                let mut component_claims = self.component_claims.write().await;
                component_claims.remove(&claims.subject);
            }
            Claims::Provider(claims) => {
                let mut provider_claims = self.provider_claims.write().await;
                provider_claims.remove(&claims.subject);
            }
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all)]
    pub(crate) async fn process_entry(
        &self,
        KvEntry {
            key,
            value,
            operation,
            ..
        }: KvEntry,
        publish: bool,
    ) {
        let key_id = key.split_once('_');
        let res = match (operation, key_id) {
            (Operation::Put, Some(("COMPONENT", id))) => {
                self.process_component_spec_put(id, value, publish).await
            }
            (Operation::Delete, Some(("COMPONENT", id))) => {
                self.process_component_spec_delete(id, value, publish).await
            }
            (Operation::Put, Some(("LINKDEF", _id))) => {
                debug!("ignoring deprecated LINKDEF put operation");
                Ok(())
            }
            (Operation::Delete, Some(("LINKDEF", _id))) => {
                debug!("ignoring deprecated LINKDEF delete operation");
                Ok(())
            }
            (Operation::Put, Some(("CLAIMS", pubkey))) => {
                self.process_claims_put(pubkey, value).await
            }
            (Operation::Delete, Some(("CLAIMS", pubkey))) => {
                self.process_claims_delete(pubkey, value).await
            }
            (operation, Some(("REFMAP", id))) => {
                // TODO: process REFMAP entries
                debug!(?operation, id, "ignoring REFMAP entry");
                Ok(())
            }
            _ => {
                warn!(key, ?operation, "unsupported KV bucket entry");
                Ok(())
            }
        };
        if let Err(error) = &res {
            error!(key, ?operation, ?error, "failed to process KV bucket entry");
        }
    }
}

#[instrument(level = "debug", skip_all)]
pub(crate) async fn create_bucket(
    jetstream: &async_nats::jetstream::Context,
    bucket: &str,
) -> anyhow::Result<Store> {
    // Don't create the bucket if it already exists
    if let Ok(store) = jetstream.get_key_value(bucket).await {
        info!(%bucket, "bucket already exists. Skipping creation.");
        return Ok(store);
    }

    match jetstream
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: bucket.to_string(),
            ..Default::default()
        })
        .await
    {
        Ok(store) => {
            info!(%bucket, "created bucket with 1 replica");
            Ok(store)
        }
        Err(err) => Err(anyhow!(err).context(format!("failed to create bucket '{bucket}'"))),
    }
}
