//! [Couchbase](https://www.couchbase.com/) implementation for `wrpc:keyvalue`.
//!
//! This implementation is multi-threaded and operations between different actors
//! use different connections and can run in parallel.
//!
//! A single connection is shared by all instances of the same component ID,
//! so there may be some brief lock contention if several instances of the same component
//! are simultaneously attempting to communicate with Couchbase.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context as _};
use couchbase::Cluster;
use tokio::sync::RwLock;
use tracing::{debug, error, instrument, warn};

use wasmcloud_provider_sdk::{
    get_connection, propagate_trace_for_ctx, run_provider, Context, LinkConfig, Provider,
};

/// Generated bindings for the WIT world specified in `wit/provider.wit`
mod bindings {
    wit_bindgen_wrpc::generate!();
}
use bindings::exports::wrpc::keyvalue;

/// Key name for Couchbase connection URLs supplied at link time
const CONFIG_COUCHBASE_URL_KEY: &str = "URL";

/// Key name for Couchbase username supplied at link time
const CONFIG_COUCHBASE_USERNAME_KEY: &str = "USERNAME";

/// Key name for Couchbase password supplied at link time
const CONFIG_COUCHBASE_PASSWORD_KEY: &str = "PASSWORD";

/// Alias for [`Result`]s that return errors defined in the keyvalue WIT contract
type KeyvalueResult<T, E = keyvalue::store::Error> = core::result::Result<T, E>;

/// Couchbase `wrpc:keyvalue` Provider implementation
///
/// All functionality required to access Couchbase and traits necessary to implement
/// a capability provider that runs in a wasmCloud lattice are hung from this struct
#[derive(Clone, Default)]
pub struct KvCouchbaseProvider {
    /// Map of source ID (ex. component IDs) to Couchbase connections
    connections: Arc<RwLock<HashMap<String, Arc<Cluster>>>>,
}

pub async fn run() -> anyhow::Result<()> {
    KvCouchbaseProvider::run().await
}

impl KvCouchbaseProvider {
    pub async fn run() -> anyhow::Result<()> {
        let provider = KvCouchbaseProvider::new();
        let shutdown = run_provider(provider.clone(), "keyvalue-couchbase-provider")
            .await
            .context("failed to run provider")?;
        let connection = get_connection();
        bindings::serve(
            &connection.get_wrpc_client(connection.provider_key()),
            provider,
            shutdown,
        )
        .await
    }

    #[must_use]
    pub fn new() -> Self {
        KvCouchbaseProvider {
            connections: Arc::default(),
        }
    }

    /// Retrieve the a [`Cluster`] for a given `Context` (normally tied to an invocation on a link)
    #[instrument(level = "debug", skip(self))]
    async fn cluster_from_ctx(&self, context: Option<Context>) -> anyhow::Result<Arc<Cluster>> {
        let Some(ref source_id) = context.and_then(|Context { component, .. }| component) else {
            bail!("failed to find source_id on context");
        };

        let connections = self.connections.read().await;
        let Some(conn) = connections.get(source_id) else {
            error!(source_id, "no Couchbase connection found for component");
            bail!("No Couchbase connection found for component [{source_id}]. Please ensure the URL supplied in the link definition is a valid Couchbase URL")
        };
        Ok(conn.clone())
    }
}

impl keyvalue::store::Handler<Option<Context>> for KvCouchbaseProvider {
    #[instrument(level = "debug", skip(self))]
    async fn delete(
        &self,
        ctx: Option<Context>,
        bucket: String,
        key: String,
    ) -> anyhow::Result<KeyvalueResult<()>> {
        propagate_trace_for_ctx!(ctx);
        let cluster = self.cluster_from_ctx(ctx).await?;
        let bucket = cluster.bucket(bucket);
        let collection = bucket.default_collection();
        let _ = collection
            .remove(&key, couchbase::RemoveOptions::default())
            .await
            .with_context(|| format!("failed to delete key [{key}]"))?;
        Ok(Ok(()))
    }

    #[instrument(level = "debug", skip(self))]
    async fn exists(
        &self,
        ctx: Option<Context>,
        bucket: String,
        key: String,
    ) -> anyhow::Result<KeyvalueResult<bool>> {
        propagate_trace_for_ctx!(ctx);
        let cluster = self.cluster_from_ctx(ctx).await?;
        let bucket = cluster.bucket(bucket);
        let collection = bucket.default_collection();
        let exists = collection
            .exists(&key, couchbase::ExistsOptions::default())
            .await
            .map(|result| result.exists())
            .with_context(|| format!("failed to check if key [{key}] exists"))?;
        Ok(Ok(exists))
    }

    #[instrument(level = "debug", skip(self))]
    async fn get(
        &self,
        ctx: Option<Context>,
        bucket: String,
        key: String,
    ) -> anyhow::Result<KeyvalueResult<Option<Vec<u8>>>> {
        propagate_trace_for_ctx!(ctx);
        let cluster = self.cluster_from_ctx(ctx).await?;
        let bucket = cluster.bucket(bucket);
        let collection = bucket.default_collection();
        let bytes = collection
            .get(&key, couchbase::GetOptions::default())
            .await
            .with_context(|| format!("failed to retrieve value for key [{key}]"))?
            .content::<Vec<u8>>()
            .context("failed to parse returned content as bytes")?;
        Ok(Ok(Some(bytes)))
    }

    #[instrument(level = "debug", skip(self))]
    async fn set(
        &self,
        ctx: Option<Context>,
        bucket: String,
        key: String,
        value: Vec<u8>,
    ) -> anyhow::Result<KeyvalueResult<()>> {
        propagate_trace_for_ctx!(ctx);
        let cluster = self.cluster_from_ctx(ctx).await?;
        let bucket = cluster.bucket(bucket);
        let collection = bucket.default_collection();
        collection
            .insert(&key, value, couchbase::InsertOptions::default())
            .await
            .with_context(|| format!("failed to insert value for key [{key}]"))?;
        Ok(Ok(()))
    }

    #[instrument(level = "debug", skip(self))]
    async fn list_keys(
        &self,
        ctx: Option<Context>,
        bucket: String,
        cursor: Option<u64>,
    ) -> anyhow::Result<KeyvalueResult<keyvalue::store::KeyResponse>> {
        propagate_trace_for_ctx!(ctx);
        // This does not seem to be supported without SQL?
        // https://www.couchbase.com/forums/t/fetching-keys-across-all-docs-in-a-bucket-from-the-cli/36221/5
        todo!()
    }
}

impl keyvalue::atomics::Handler<Option<Context>> for KvCouchbaseProvider {
    /// Increments a numeric value, returning the new value
    #[instrument(level = "debug", skip(self))]
    async fn increment(
        &self,
        ctx: Option<Context>,
        bucket: String,
        key: String,
        delta: u64,
    ) -> anyhow::Result<KeyvalueResult<u64, keyvalue::store::Error>> {
        propagate_trace_for_ctx!(ctx);
        let cluster = self.cluster_from_ctx(ctx).await?;
        let bucket = cluster.bucket(bucket);
        let collection = bucket.default_collection().binary();
        let opts = couchbase::IncrementOptions::default().delta(delta);
        let updated = collection
            .increment(&key, opts)
            .await
            .map(|result| result.content())
            .with_context(|| format!("failed to increment value for key [{key}] by [{delta}]"))?;
        Ok(Ok(updated))
    }
}

impl keyvalue::batch::Handler<Option<Context>> for KvCouchbaseProvider {
    async fn get_many(
        &self,
        ctx: Option<Context>,
        bucket: String,
        keys: Vec<String>,
    ) -> anyhow::Result<KeyvalueResult<Vec<Option<(String, Vec<u8>)>>>> {
        propagate_trace_for_ctx!(ctx);
        let cluster = self.cluster_from_ctx(ctx).await?;
        let bucket = cluster.bucket(bucket);

        let mut set = tokio::task::JoinSet::new();
        for key in keys {
            let collection = bucket.default_collection();
            set.spawn(async move {
                collection
                    .get(&key, couchbase::GetOptions::default())
                    .await
                    .map(|result| (String::from(&key), result))
                    .with_context(|| format!("failed to retrieve value for key [{key}]"))
            });
        }

        let mut results = Vec::new();
        while let Some(res) = set.join_next().await {
            if let Ok(Ok((key, result))) = res {
                results.push(Some((
                    key.to_string(),
                    result
                        .content::<Vec<u8>>()
                        .context("failed to retrieve byte content for key")?,
                )));
            }
        }

        Ok(Ok(results))
    }

    async fn set_many(
        &self,
        ctx: Option<Context>,
        bucket: String,
        items: Vec<(String, Vec<u8>)>,
    ) -> anyhow::Result<KeyvalueResult<()>> {
        propagate_trace_for_ctx!(ctx);
        let cluster = self.cluster_from_ctx(ctx).await?;
        let bucket = cluster.bucket(bucket);

        let mut set = tokio::task::JoinSet::new();
        for (key, value) in items {
            let collection = bucket.default_collection();
            set.spawn(async move {
                collection
                    .insert(&key, value, couchbase::InsertOptions::default())
                    .await
                    .map(|result| (String::from(&key), result))
                    .with_context(|| format!("failed to retrieve value for key [{key}]"))
            });
        }

        while let Some(res) = set.join_next().await {
            let _ = res.context("failed to perform set_many")?;
        }

        Ok(Ok(()))
    }

    async fn delete_many(
        &self,
        ctx: Option<Context>,
        bucket: String,
        keys: Vec<String>,
    ) -> anyhow::Result<KeyvalueResult<()>> {
        propagate_trace_for_ctx!(ctx);
        let cluster = self.cluster_from_ctx(ctx).await?;
        let bucket = cluster.bucket(bucket);

        let mut set = tokio::task::JoinSet::new();
        for key in keys {
            let collection = bucket.default_collection();
            set.spawn(async move {
                collection
                    .remove(&key, couchbase::RemoveOptions::default())
                    .await
                    .map(|result| (String::from(&key), result))
                    .with_context(|| format!("failed to retrieve value for key [{key}]"))
            });
        }

        while let Some(res) = set.join_next().await {
            let _ = res.context("failed to perform delete_many")?;
        }

        Ok(Ok(()))
    }
}

/// Handle provider control commands
impl Provider for KvCouchbaseProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-component resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    #[instrument(level = "debug", skip(self, config))]
    async fn receive_link_config_as_target(
        &self,
        LinkConfig {
            source_id, config, ..
        }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let (Some(url), Some(username), Some(password)) =
            config.keys().fold((None, None, None), |mut acc, k| {
                if k.eq_ignore_ascii_case(CONFIG_COUCHBASE_URL_KEY) {
                    acc.0 = config.get(k);
                }
                if k.eq_ignore_ascii_case(CONFIG_COUCHBASE_USERNAME_KEY) {
                    acc.1 = config.get(k);
                }
                if k.eq_ignore_ascii_case(CONFIG_COUCHBASE_PASSWORD_KEY) {
                    acc.2 = config.get(k);
                }
                acc
            })
        else {
            warn!("failed to find one or more Couchbase configuration options");
            return Ok(());
        };

        // Save cluster connection for source
        let mut connections = self.connections.write().await;
        connections.insert(
            source_id.to_string(),
            Arc::new(Cluster::connect(url, username, password)),
        );

        Ok(())
    }

    /// Handle notification that a link is dropped - close the connection
    #[instrument(level = "info", skip(self))]
    async fn delete_link(&self, source_id: &str) -> anyhow::Result<()> {
        let mut aw = self.connections.write().await;
        if let Some(conn) = aw.remove(source_id) {
            debug!(component_id = source_id, "closing connection for component");
            drop(conn);
        }
        Ok(())
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> anyhow::Result<()> {
        let mut aw = self.connections.write().await;
        // empty the component link data and stop all servers
        for (_, conn) in aw.drain() {
            drop(conn);
        }
        Ok(())
    }
}
