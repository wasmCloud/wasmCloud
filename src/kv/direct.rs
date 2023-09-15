use std::collections::HashMap;
use std::ops::Deref;

use async_nats::{jetstream::kv::Store, Client};
use futures::{TryFutureExt, TryStreamExt};
use serde::de::DeserializeOwned;
use tracing::{debug, error};

use super::{
    delete_link, ld_hash_raw, put_link, KvStore, CLAIMS_PREFIX, LINKDEF_PREFIX, SUBJECT_KEY,
};
use crate::{types::LinkDefinition, Result};

#[derive(Clone, Debug)]
pub struct DirectKvStore {
    store: Store,
}

impl AsRef<Store> for DirectKvStore {
    fn as_ref(&self) -> &Store {
        &self.store
    }
}

impl Deref for DirectKvStore {
    type Target = Store;

    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

impl DirectKvStore {
    pub async fn new(nc: Client, lattice_prefix: &str, js_domain: Option<String>) -> Result<Self> {
        super::get_kv_store(nc, lattice_prefix, js_domain)
            .await
            .map(|store| Self { store })
    }

    async fn fetch_data<T: DeserializeOwned>(&self, filter_prefix: &str) -> Result<Vec<T>> {
        // NOTE(thomastaylor312): This is a fairly gnarly mapping thing, but we're trying to avoid
        // iterating over all the keys multiple times. I have heavily annotated the code below to
        // try and make it more clear what's going on.

        // First thing here is a big block that lists all keys in the store. That list is a `Stream`
        // that returns `Result`s, so we have to use the `TryStreamExt` methods here to iterate over
        // each key.
        let futs = self
            .store
            .keys()
            // Why is there a `?` here you might ask? Because `keys` might _also_ return an error
            // when constructing the stream. The `Ok` result here contains the actual stream we're
            // going to use
            .await?
            // Only process keys that have the right prefix. We use `futures::future::ready` here
            // because it gets around data ownership issues, but we still need to return a future
            .try_filter(|key| futures::future::ready(key.starts_with(filter_prefix)))
            // For the remaining keys, we want to map _all_ `Ok` responses to a future that will
            // fetch the data from the key
            .map_ok(|k| {
                self.store
                    .get(k)
                    // If we get an ok response from the `get` call, we want to map that to the
                    // serialized data
                    .map_ok(|maybe_bytes| {
                        maybe_bytes.map(|bytes| serde_json::from_slice::<T>(&bytes))
                    })
                    // Explicitly annotate the error conversion here because it is hard to do it
                    // later on and this is complex enough the compiler can't infer
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
            })
            // Read through the whole stream of keys and collect all the constructed futures into a vec
            .try_collect::<Vec<_>>()
            // We collect to an error here so if there was an error when streaming the keys, we can return early
            .await?;

        // Now that we have a vec of futures, we can use `join_all` to run them all concurrently.
        let data: Vec<T> = futures::future::join_all(futs)
            .await
            .into_iter()
            // Because `store.get` returns an `Option` if the key doesn't exist (i.e. the key was
            // deleted in between reading and fetching), we transpose the `Result<Option<_>>` to
            // `Option<Result<_>>` and let the filter_map handle the `None` case
            .filter_map(|res| res.transpose())
            // We have to collect once here to get the outer Result, which would be if any of the
            // `store.get` calls failed, which would be an error. Unfortunately that means we need
            // to collect to a Vec and then re-iterate over it, but it was the only way to handle
            // the error
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            // We don't actually care if the data is malformed, but we do need to log the error so
            // that it can be fixed. This is probably the best trade off here as a failure that we
            // return here would _definitely_ be noticeable, but we don't want things screeching to
            // a halt because of a single (or even a few) malformed entries
            .filter_map(|res| match res {
                Ok(v) => Some(v),
                Err(e) => {
                    error!(error = %e, "failed to deserialize data, skipping entry");
                    None
                }
            })
            // Tha...tha...tha...that's all folks!
            .collect();
        Ok(data)
    }

    async fn fetch_single<T: DeserializeOwned>(&self, key: String) -> Result<Option<T>> {
        self.store
            .get(key)
            .await?
            .map(|bytes| serde_json::from_slice::<T>(&bytes))
            .transpose()
            .map_err(|e| e.into())
    }

    async fn filter_claims(&self, key_prefix: char) -> Result<Vec<HashMap<String, String>>> {
        Ok(self
            .get_all_claims()
            .await?
            .into_iter()
            .filter_map(|claims| match claims.get(SUBJECT_KEY) {
                Some(subject) if subject.starts_with(key_prefix) => Some(claims),
                None => {
                    debug!(?claims, "claims missing subject key");
                    None
                }
                _ => None,
            })
            .collect())
    }
}

#[async_trait::async_trait]
impl KvStore for DirectKvStore {
    async fn get_links(&self) -> Result<Vec<LinkDefinition>> {
        self.fetch_data(LINKDEF_PREFIX).await
    }
    async fn get_all_claims(&self) -> Result<Vec<HashMap<String, String>>> {
        self.fetch_data(CLAIMS_PREFIX).await
    }
    async fn get_provider_claims(&self) -> Result<Vec<HashMap<String, String>>> {
        self.filter_claims('V').await
    }
    async fn get_actor_claims(&self) -> Result<Vec<HashMap<String, String>>> {
        self.filter_claims('M').await
    }
    async fn get_filtered_links<F>(&self, filter_fn: F) -> Result<Vec<LinkDefinition>>
    where
        F: FnMut(&LinkDefinition) -> bool + Send,
    {
        self.get_links().await.map(|links| {
            links
                .into_iter()
                .filter(filter_fn)
                .collect::<Vec<LinkDefinition>>()
        })
    }

    async fn get_link(
        &self,
        actor_id: &str,
        link_name: &str,
        contract_id: &str,
    ) -> Result<Option<LinkDefinition>> {
        self.fetch_single(format!(
            "{LINKDEF_PREFIX}{}",
            ld_hash_raw(actor_id, contract_id, link_name)
        ))
        .await
    }

    async fn get_claims(&self, id: &str) -> Result<Option<HashMap<String, String>>> {
        self.fetch_single(format!("{CLAIMS_PREFIX}{id}")).await
    }
    async fn put_link(&self, ld: LinkDefinition) -> Result<()> {
        put_link(&self.store, &ld).await
    }
    async fn delete_link(&self, actor_id: &str, contract_id: &str, link_name: &str) -> Result<()> {
        delete_link(&self.store, actor_id, contract_id, link_name).await
    }
}
