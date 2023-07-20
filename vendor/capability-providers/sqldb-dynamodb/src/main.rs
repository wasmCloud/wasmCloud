//! sqldb-dynamodb capability provider
//!
//!
use sqldb_dynamodb_lib::{SqlDbClient, StorageConfig};
use std::{collections::HashMap, convert::Infallible, sync::Arc};
use tokio::sync::RwLock;
use wasmbus_rpc::provider::prelude::*;
use wasmcloud_interface_sqldb::{ExecuteResult, QueryResult, SqlDb, SqlDbReceiver, Statement};

// main (via provider_main) initializes the threaded tokio executor,
// listens to lattice rpcs, handles actor links,
// and returns only when it receives a shutdown message
//
fn main() -> Result<(), Box<dyn std::error::Error>> {
    provider_main(
        SqldbDynamodbProvider::default(),
        Some("SqldbDynamodb".to_string()),
    )?;

    eprintln!("sqldb-dynamodb provider exiting");
    Ok(())
}

/// sqldb-dynamodb capability provider implementation
#[derive(Default, Clone, Provider)]
#[services(SqlDb)]
struct SqldbDynamodbProvider {
    actors: Arc<RwLock<HashMap<String, SqlDbClient>>>,
}

impl SqldbDynamodbProvider {
    async fn client(&self, ctx: &Context) -> RpcResult<SqlDbClient> {
        let actor_id = ctx
            .actor
            .as_ref()
            .ok_or_else(|| RpcError::InvalidParameter("no actor in request".to_string()))?;
        // get read lock on actor-client hashmap
        let rd = self.actors.read().await;
        let client = rd
            .get(actor_id)
            .ok_or_else(|| RpcError::InvalidParameter(format!("actor not linked:{}", actor_id)))?;
        Ok(client.clone())
    }
}

/// use default implementations of provider message handlers
impl ProviderDispatch for SqldbDynamodbProvider {}

#[async_trait]
impl ProviderHandler for SqldbDynamodbProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    async fn put_link(&self, ld: &LinkDefinition) -> RpcResult<bool> {
        let config = StorageConfig::from_values(&ld.values)?;
        let link = SqlDbClient::new(config, Some(ld.clone())).await;

        let mut update_map = self.actors.write().await;
        update_map.insert(ld.actor_id.to_string(), link);

        Ok(true)
    }

    /// Handle notification that a link is dropped: close the connection
    async fn delete_link(&self, actor_id: &str) {
        let mut aw = self.actors.write().await;
        if let Some(link) = aw.remove(actor_id) {
            // close and drop the connection
            let _ = link.close().await;
        }
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> Result<(), Infallible> {
        let mut aw = self.actors.write().await;
        // empty the actor link data and stop all servers
        for (_, link) in aw.drain() {
            // close and drop each connection
            let _ = link.close().await;
        }
        Ok(())
    }
}

/// Handle SqlDb methods that interact with DynamoDB
/// To simplify testing, the methods are also implemented for StorageClient,
#[async_trait]
impl SqlDb for SqldbDynamodbProvider {
    async fn execute(&self, ctx: &Context, stmt: &Statement) -> RpcResult<ExecuteResult> {
        let client = self.client(ctx).await?;
        client.execute(ctx, stmt).await
    }

    async fn query(&self, ctx: &Context, stmt: &Statement) -> RpcResult<QueryResult> {
        let client = self.client(ctx).await?;
        client.query(ctx, stmt).await
    }
}
