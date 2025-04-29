//! Implementation of the [ConfigManager] trait for NATS JetStream KV [Store].

use std::{collections::HashMap, sync::Arc};

use anyhow::{anyhow, ensure, Context as _};
use async_nats::jetstream::kv::{Entry as KvEntry, Operation, Store};
use bytes::Bytes;
use futures::{StreamExt as _, TryStreamExt as _};
use tokio::{
    sync::watch::{self, Receiver},
    task::JoinSet,
};
use tracing::{debug, error, instrument, warn};

use crate::{
    config::ConfigManager,
    store::StoreManager,
    wasmbus::{
        claims::{Claims, StoredClaims},
        ComponentSpecification,
    },
};

#[async_trait::async_trait]
impl StoreManager for Store {
    #[instrument(level = "debug", skip(self))]
    async fn get(&self, key: &str) -> anyhow::Result<Option<Bytes>> {
        self.get(key)
            .await
            .map_err(|err| anyhow::anyhow!("Failed to get config: {}", err))
    }

    #[instrument(level = "debug", skip(self, value))]
    async fn put(&self, key: &str, value: Bytes) -> anyhow::Result<()> {
        self.put(key, value)
            .await
            .map(|_| ())
            .map_err(|err| anyhow::anyhow!("Failed to set config: {}", err))
    }

    #[instrument(level = "debug", skip(self))]
    async fn del(&self, key: &str) -> anyhow::Result<()> {
        self.purge(key)
            .await
            .map_err(|err| anyhow::anyhow!("Failed to delete config: {}", err))
    }
}

#[async_trait::async_trait]
impl ConfigManager for Store {
    /// Watch the key in the JetStream bucket for changes. This will return a channel that will
    /// receive updates to the config as they happen.
    async fn watch(&self, name: &str) -> anyhow::Result<Receiver<HashMap<String, String>>> {
        let config: HashMap<String, String> = match self.get(name).await {
            Ok(Some(data)) => serde_json::from_slice(&data)
                .context("Data corruption error, unable to decode data from store")?,
            Ok(None) => return Err(anyhow::anyhow!("Config {} does not exist", name)),
            Err(e) => return Err(anyhow::anyhow!("Error fetching config {}: {}", name, e)),
        };

        let (tx, rx) = watch::channel(config);
        // Since we're starting a task, we need to own this data
        let name = name.to_owned();
        let mut watcher = self.watch(&name).await.context("Failed to watch config")?;

        tokio::spawn(async move {
            loop {
                if tx.is_closed() {
                    warn!(%name, "config watch channel closed, aborting watch");
                    return;
                }

                match watcher.try_next().await {
                    Ok(Some(entry))
                        if matches!(entry.operation, Operation::Delete | Operation::Purge) =>
                    {
                        // NOTE(thomastaylor312): We should probably do something and notify something up
                        // the chain if we get a delete or purge event of a config that is still being used.
                        // For now we just zero it out
                        tx.send_replace(HashMap::new());
                    }
                    Ok(Some(entry)) => {
                        let config: HashMap<String, String> = match serde_json::from_slice(
                            &entry.value,
                        ) {
                            Ok(config) => config,
                            Err(e) => {
                                error!(%name, error = %e, "Error decoding config from store during watch");
                                continue;
                            }
                        };
                        tx.send_if_modified(|current| {
                            if current == &config {
                                false
                            } else {
                                *current = config;
                                true
                            }
                        });
                    }
                    Ok(None) => {
                        error!(%name, "Watcher for config has closed");
                        return;
                    }
                    Err(e) => {
                        error!(%name, error = %e, "Error reading from watcher for config. Will wait for next entry");
                        continue;
                    }
                }
            }
        });

        Ok(rx)
    }
}

/// This is an extra implementation for the host to process entries coming from a JetStream bucket.
impl crate::wasmbus::Host {
    #[instrument(level = "trace", skip_all)]
    pub(crate) async fn process_entry(
        &self,
        KvEntry {
            key,
            value,
            operation,
            ..
        }: KvEntry,
    ) {
        let key_id = key.split_once('_');
        let res = match (operation, key_id) {
            (Operation::Put, Some(("COMPONENT", id))) => {
                self.process_component_spec_put(id, value).await
            }
            (Operation::Delete, Some(("COMPONENT", id))) => {
                self.process_component_spec_delete(id).await
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

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn process_component_spec_put(
        &self,
        id: impl AsRef<str>,
        value: impl AsRef<[u8]>,
    ) -> anyhow::Result<()> {
        let id = id.as_ref();
        debug!(id, "process component spec put");

        let spec: ComponentSpecification = serde_json::from_slice(value.as_ref())
            .context("failed to deserialize component specification")?;
        self.update_host_with_spec(id, &spec)
            .await
            .context("failed to update component spec")?;

        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn process_component_spec_delete(
        &self,
        id: impl AsRef<str>,
    ) -> anyhow::Result<()> {
        debug!(id = id.as_ref(), "process component delete");
        self.delete_component_spec(id).await
    }

    #[instrument(level = "debug", skip_all)]
    /// Process claims being put into the JetStream data store.
    ///
    /// Notably this updates the host map but does not call [Self::store_claims], which
    /// would cause an infinite loop.
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
            Claims::Provider(claims) => self.store_provider_claims(claims).await,
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
            Claims::Component(_) => self.delete_component_claims(pubkey).await,
            Claims::Provider(_) => self.delete_provider_claims(pubkey).await,
        }
    }
}

//TODO(brooksmtownsend): Make sure that the configbundle accomplishes this for config
/// Watch the JetStream bucket for changes to the ComponentSpec and claims data
pub async fn data_watch(
    tasks: &mut JoinSet<anyhow::Result<()>>,
    store: Store,
    host: Arc<crate::wasmbus::Host>,
) -> anyhow::Result<()> {
    tasks.spawn({
        let host = Arc::clone(&host);
        let data = store.clone();
        async move {
            // Setup data watch first
            let data_watch = data
                .watch_all()
                .await
                .context("failed to watch lattice data bucket")?;

            // Process existing data without emitting events
            data.keys()
                .await
                .context("failed to read keys of lattice data bucket")?
                .map_err(|e| anyhow!(e).context("failed to read lattice data stream"))
                .try_filter_map(|key| async {
                    data.entry(key)
                        .await
                        .context("failed to get entry in lattice data bucket")
                })
                .for_each(|entry| async {
                    match entry {
                        Ok(entry) => host.process_entry(entry).await,
                        Err(err) => {
                            error!(%err, "failed to read entry from lattice data bucket")
                        }
                    }
                })
                .await;
            // TODO(brooksmtownsend): Do we need this?
            // let mut data_watch = Abortable::new(data_watch, data_watch_abort_reg);
            data_watch
                // .by_ref()
                .for_each({
                    let host = Arc::clone(&host);
                    move |entry| {
                        let host = Arc::clone(&host);
                        async move {
                            match entry {
                                Err(error) => {
                                    error!("failed to watch lattice data bucket: {error}");
                                }
                                Ok(entry) => host.process_entry(entry).await,
                            }
                        }
                    }
                })
                .await;
            let deadline = { *host.stop_rx.borrow() };
            host.stop_tx.send_replace(deadline);
            // if data_watch.is_aborted() {
            //     info!("data watch task gracefully stopped");
            // } else {
            //     error!("data watch task unexpectedly stopped");
            // }
            Ok(())
        }
    });

    Ok(())
}
