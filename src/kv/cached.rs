use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

use async_nats::jetstream::kv::{Entry, EntryError, Operation};
use async_nats::{jetstream::kv::Store, Client};
use futures::StreamExt;
use futures::TryStreamExt;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{debug, error, trace, Instrument};

use crate::LinkDefinition;
use crate::Result;

use super::{
    delete_link, ld_hash, ld_hash_raw, put_link, Build, KvStore, CLAIMS_PREFIX, LINKDEF_PREFIX,
    SUBJECT_KEY,
};

type ClaimsMap = HashMap<String, HashMap<String, String>>;

/// A KV store that caches all link definitions and claims in memory as it receives updates from the
/// NATS KV bucket. This store is recommended for use in situations where there are many data
/// lookups (an example of this is Wadm).
#[derive(Clone)]
pub struct CachedKvStore {
    store: Store,
    linkdefs: Arc<RwLock<HashMap<String, LinkDefinition>>>,
    claims: Arc<RwLock<ClaimsMap>>,
    _handle: Arc<WrappedHandle>,
}

impl AsRef<Store> for CachedKvStore {
    fn as_ref(&self) -> &Store {
        &self.store
    }
}

impl Deref for CachedKvStore {
    type Target = Store;

    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

/// A wrapper around a JoinHandle that will abort the task when dropped. This allows it to be
/// wrapped in an Arc and cloned, but doesn't abort the task until the last arc is dropped
struct WrappedHandle {
    handle: JoinHandle<()>,
}

impl Drop for WrappedHandle {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl CachedKvStore {
    /// Create a new KV store with the given configuration. This function will do an initial fetch
    /// of all claims and linkdefs from the store and then start a watcher to keep the cache up to
    /// date. All data fetched from this store will be from the in memory cache
    pub async fn new(nc: Client, lattice_prefix: &str, js_domain: Option<String>) -> Result<Self> {
        let store = super::get_kv_store(nc, lattice_prefix, js_domain).await?;
        let linkdefs = Arc::new(RwLock::new(HashMap::new()));
        let claims = Arc::new(RwLock::new(ClaimsMap::default()));
        let linkdefs_clone = Arc::clone(&linkdefs);
        let claims_clone = Arc::clone(&claims);
        let cloned_store = store.clone();
        let (tx, rx) = tokio::sync::oneshot::channel::<Result<()>>();
        let handle = tokio::spawn(
            async move {
                // We have to create this in here and use the oneshot to return the error because of
                // lifetimes
                let mut watcher = match cloned_store.watch_all().await {
                    // NOTE(thomastaylor312) We are unwrapping the sends here because it only fails
                    // if the rx has hung up. Since we are literally using it down below in the new
                    // function, this shouldn't happen and if it does it is programmer error
                    Ok(w) => w,
                    Err(e) => {
                        error!(error = %e, "Unable to start watcher");
                        tx.send(Err(e.into())).unwrap();
                        return;
                    }
                };
                debug!("Getting initial data from store");
                // Start with an initial list of the data before consuming events from the watcher.
                // This will ensure we have the most up to date data from the watcher (which we
                // started before this step) as well as all entries from the store
                let keys = match cloned_store.keys().await {
                    Ok(k) => k,
                    Err(e) => {
                        error!(error = %e, "Unable to get keys from store");
                        tx.send(Err(e.into())).unwrap();
                        return;
                    }
                };

                let futs = match keys
                    .map_ok(|k| cloned_store.entry(k))
                    .try_collect::<Vec<_>>()
                    .await
                {
                    Ok(f) => f,
                    Err(e) => {
                        error!(error = %e, "Unable to get keys from store");
                        tx.send(Err(e.into())).unwrap();
                        return;
                    }
                };

                let all_entries = match futures::future::join_all(futs)
                    .await
                    .into_iter()
                    .filter_map(|res| res.transpose())
                    .collect::<std::result::Result<Vec<_>, EntryError>>()
                {
                    Ok(entries) => entries,
                    Err(e) => {
                        error!(error = %e, "Unable to get values from store");
                        tx.send(Err(e.into())).unwrap();
                        return;
                    }
                };
                debug!(num_entries = %all_entries.len(), "Finished fetching initial data, adding data to cache");

                tx.send(Ok(())).unwrap();

                for entry in all_entries {
                    handle_entry(entry, Arc::clone(&linkdefs_clone), Arc::clone(&claims_clone)).await;
                }

                trace!("Beginning watch on store");
                while let Some(event) = watcher.next().await {
                    let entry = match event {
                        Ok(en) => en,
                        Err(e) => {
                            error!(error = %e, "Error from latticedata watcher");
                            continue;
                        }
                    };
                    trace!(key = %entry.key, bucket = %entry.bucket, operation = ?entry.operation, "Received entry from watcher, handling");
                    handle_entry(entry, Arc::clone(&linkdefs_clone), Arc::clone(&claims_clone)).in_current_span().await;
                    trace!("Finished handling entry from watcher");
                }
                // NOTE(thomastaylor312): We should probably do something to automatically restart
                // the watch if something fails. But for now this should be ok
                error!("Cache watcher has exited");
            }
            .instrument(tracing::trace_span!("kvstore-watcher", %lattice_prefix)),
        );
        let kvstore = CachedKvStore {
            store,
            linkdefs,
            claims,
            _handle: Arc::new(WrappedHandle { handle }),
        };
        rx.await??;
        Ok(kvstore)
    }
}

#[async_trait::async_trait]
impl Build for CachedKvStore {
    async fn build(nc: Client, lattice_prefix: &str, js_domain: Option<String>) -> Result<Self> {
        CachedKvStore::new(nc, lattice_prefix, js_domain).await
    }
}

#[async_trait::async_trait]
impl KvStore for CachedKvStore {
    /// Return a copy of all link definitions in the store
    // TODO(thomastaylor312): This should probably return a reference to the link definitions, but
    // that involves wrapping this with an owned ReadWriteLockGuard, which is probably overkill for
    // now
    async fn get_links(&self) -> Result<Vec<LinkDefinition>> {
        Ok(self.linkdefs.read().await.values().cloned().collect())
    }

    /// Return a copy of all claims in the store
    // See comment above about get_links
    async fn get_all_claims(&self) -> Result<Vec<HashMap<String, String>>> {
        Ok(self.claims.read().await.values().cloned().collect())
    }

    /// Return a copy of all provider claims in the store
    // See comment above about get_links
    async fn get_provider_claims(&self) -> Result<Vec<HashMap<String, String>>> {
        Ok(self
            .claims
            .read()
            .await
            .iter()
            // V is the first character of a provider nkey
            .filter_map(|(key, values)| key.starts_with('V').then_some(values))
            .cloned()
            .collect())
    }

    /// Return a copy of all actor claims in the store
    // See comment above about get_links
    async fn get_actor_claims(&self) -> Result<Vec<HashMap<String, String>>> {
        Ok(self
            .claims
            .read()
            .await
            .iter()
            // M is the first character of an actor nkey
            .filter_map(|(key, values)| key.starts_with('M').then_some(values))
            .cloned()
            .collect())
    }

    /// A convenience function to get a list of link definitions filtered using the given filter
    /// function
    async fn get_filtered_links<F>(&self, mut filter_fn: F) -> Result<Vec<LinkDefinition>>
    where
        F: FnMut(&LinkDefinition) -> bool + Send,
    {
        Ok(self
            .linkdefs
            .read()
            .await
            .values()
            // We have to pass in manually because this is a technically an &&LinkDefinition
            .filter(|ld| filter_fn(ld))
            .cloned()
            .collect())
    }

    /// Get a link definition for a specific ID (actor_id, contract_id, link_name)
    async fn get_link(
        &self,
        actor_id: &str,
        link_name: &str,
        contract_id: &str,
    ) -> Result<Option<LinkDefinition>> {
        Ok(self
            .linkdefs
            .read()
            .await
            .get(&ld_hash_raw(actor_id, contract_id, link_name))
            .cloned())
    }

    /// Get claims for a specific provider or actor id
    async fn get_claims(&self, id: &str) -> Result<Option<HashMap<String, String>>> {
        Ok(self.claims.read().await.get(id).cloned())
    }

    async fn put_link(&self, ld: LinkDefinition) -> Result<()> {
        put_link(&self.store, &ld).await?;
        // Immediately add the link to the local cache. It will get overwritten by the watcher as
        // soon as the event comes in, but this way a user can immediately get the link they just
        // put if needed
        self.linkdefs.write().await.insert(ld_hash(&ld), ld);
        Ok(())
    }

    async fn delete_link(&self, actor_id: &str, contract_id: &str, link_name: &str) -> Result<()> {
        delete_link(&self.store, actor_id, contract_id, link_name).await?;
        // Immediately delete the link from the local cache. It will get deleted by the watcher as
        // soon as the event comes in, but this way a user that calls get links will see it gone
        // immediately
        self.linkdefs
            .write()
            .await
            .remove(&ld_hash_raw(actor_id, contract_id, link_name));
        Ok(())
    }
}

async fn handle_entry(
    entry: Entry,
    linkdefs: Arc<RwLock<HashMap<String, LinkDefinition>>>,
    claims: Arc<RwLock<ClaimsMap>>,
) {
    if entry.key.starts_with(LINKDEF_PREFIX) {
        handle_linkdef(entry, linkdefs).in_current_span().await;
    } else if entry.key.starts_with(CLAIMS_PREFIX) {
        handle_claim(entry, claims).in_current_span().await;
    } else {
        debug!(key = %entry.key, "Ignoring entry with unrecognized key");
    }
}

async fn handle_linkdef(entry: Entry, linkdefs: Arc<RwLock<HashMap<String, LinkDefinition>>>) {
    match entry.operation {
        Operation::Delete | Operation::Purge => {
            trace!("Handling linkdef delete entry");
            let mut linkdefs = linkdefs.write().await;
            linkdefs.remove(entry.key.trim_start_matches(LINKDEF_PREFIX));
            trace!(num_entries = %linkdefs.len(), "Finished handling linkdef delete entry");
        }
        Operation::Put => {
            trace!("Handling linkdef put entry");
            let ld: LinkDefinition = match serde_json::from_slice(&entry.value) {
                Ok(ld) => ld,
                Err(e) => {
                    error!(error = %e, "Unable to deserialize as link definition");
                    return;
                }
            };
            let key = entry.key.trim_start_matches(LINKDEF_PREFIX).to_owned();
            let mut lds = linkdefs.write().await;
            match lds.insert(key, ld) {
                Some(_) => {
                    trace!("Updated linkdef with new information");
                }
                None => {
                    trace!("Added new linkdef");
                }
            }
            trace!(num_entries = %lds.len(), "Finished handling linkdef put entry");
        }
    }
}

async fn handle_claim(entry: Entry, claims: Arc<RwLock<ClaimsMap>>) {
    match entry.operation {
        Operation::Delete | Operation::Purge => {
            trace!("Handling claim delete entry");
            let mut claims = claims.write().await;
            claims.remove(entry.key.trim_start_matches(CLAIMS_PREFIX));
            trace!(num_entries = %claims.len(), "Finished handling claim delete entry");
        }
        Operation::Put => {
            trace!("Handling claim put entry");
            let json: HashMap<String, String> = match serde_json::from_slice(&entry.value) {
                Ok(j) => j,
                Err(e) => {
                    error!(error = %e, "Unable to deserialize claim as json");
                    return;
                }
            };
            let sub = match json.get(SUBJECT_KEY) {
                Some(s) => s.to_owned(),
                None => {
                    debug!("Ignoring claim without sub");
                    return;
                }
            };
            let mut c = claims.write().await;
            match c.insert(sub, json) {
                Some(_) => {
                    trace!("Updated claim with new information");
                }
                None => {
                    trace!("Added new claim");
                }
            }
            trace!(num_entries = %c.len(), "Finished handling claim put entry");
        }
    }
}
