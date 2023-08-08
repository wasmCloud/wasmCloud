//! # wasmCloud sqldb-postgres capability provider
//!
//! Enables actors to access postgres back-end database through the
//! 'wasmcloud:sqldb' capability.
//!
use std::{collections::HashMap, convert::Infallible, sync::Arc};

use bb8_postgres::tokio_postgres::NoTls;
use tokio::sync::RwLock;
use tracing::{debug, error, instrument};
use wasmbus_rpc::provider::prelude::*;
use wasmcloud_interface_sqldb::{
    Column, ExecuteResult, QueryResult, SqlDb, SqlDbReceiver, Statement,
};

mod config;
mod error;
use error::DbError;

mod types;

// main (via provider_main) initializes the threaded tokio executor,
// listens to lattice rpcs, handles actor links,
// and returns only when it receives a shutdown message
//
fn main() -> Result<(), Box<dyn std::error::Error>> {
    provider_main(
        SqlDbProvider::default(),
        Some("SQLDB Postgres Provider".to_string()),
    )?;

    eprintln!("sqldb provider exiting");
    Ok(())
}

pub(crate) type PgConnection = bb8_postgres::PostgresConnectionManager<NoTls>;
pub(crate) type Pool = bb8_postgres::bb8::Pool<PgConnection>;

/// sqldb capability provider implementation
#[derive(Default, Clone, Provider)]
#[services(SqlDb)]
struct SqlDbProvider {
    actors: Arc<RwLock<HashMap<String, Pool>>>,
}

/// use default implementations of provider message handlers
impl ProviderDispatch for SqlDbProvider {}

/// Handle connection pools for each link
#[async_trait]
impl ProviderHandler for SqlDbProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    #[instrument(level = "debug", skip(self), fields(actor_id = %ld.actor_id))]
    async fn put_link(&self, ld: &LinkDefinition) -> RpcResult<bool> {
        let config = config::load_config(ld)?;
        let pool = config::create_pool(config).await?;
        let mut update_map = self.actors.write().await;
        update_map.insert(ld.actor_id.to_string(), pool);
        Ok(true)
    }

    /// Handle notification that a link is dropped - close the connection
    #[instrument(level = "debug", skip(self))]
    async fn delete_link(&self, actor_id: &str) {
        let mut aw = self.actors.write().await;
        if let Some(conn) = aw.remove(actor_id) {
            // close all connections for this actor-link's pool
            drop(conn);
        }
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> Result<(), Infallible> {
        let mut aw = self.actors.write().await;
        // close all connections
        for (_, conn) in aw.drain() {
            drop(conn);
        }
        Ok(())
    }
}

fn actor_id(ctx: &Context) -> Result<&String, RpcError> {
    ctx.actor
        .as_ref()
        .ok_or_else(|| RpcError::InvalidParameter("no actor in request".into()))
}

/// SqlDb - SQL Database connections
/// To use this capability, the actor must be linked
/// with "wasmcloud:sqldb"
/// wasmbus.contractId: wasmcloud:sqldb
/// wasmbus.providerReceive
#[async_trait]
impl SqlDb for SqlDbProvider {
    #[instrument(level = "debug", skip(self, ctx, stmt), fields(actor_id = ?ctx.actor))]
    async fn execute(&self, ctx: &Context, stmt: &Statement) -> RpcResult<ExecuteResult> {
        debug!("executing statement");
        let actor_id = actor_id(ctx)?;
        let rd = self.actors.read().await;
        let pool = rd
            .get(actor_id)
            .ok_or_else(|| RpcError::InvalidParameter(format!("actor not linked:{}", actor_id)))?;
        let conn = pool.get().await.map_err(|e| {
            let err_msg = "failed to get connection from pool";
            error!(error = %e, err_msg);
            RpcError::Other(err_msg.to_string())
        })?;
        match conn.execute(&stmt.sql, &[]).await {
            Ok(res) => Ok(ExecuteResult {
                rows_affected: res,
                ..Default::default()
            }),
            Err(db_err) => {
                error!(
                    statement = ?stmt,
                    error = %db_err,
                    "Error executing statement"
                );
                Ok(ExecuteResult {
                    error: Some(DbError::from(db_err).into()),
                    ..Default::default()
                })
            }
        }
    }

    /// perform select query on database, returning all result rows
    #[instrument(level = "debug", skip(self, ctx, stmt), fields(actor_id = ?ctx.actor))]
    async fn query(&self, ctx: &Context, stmt: &Statement) -> RpcResult<QueryResult> {
        debug!("executing read query");
        let actor_id = actor_id(ctx)?;
        let rd = self.actors.read().await;
        let pool = rd
            .get(actor_id)
            .ok_or_else(|| RpcError::InvalidParameter(format!("actor not linked:{}", actor_id)))?;
        let conn = pool.get().await.map_err(|e| {
            let err_msg = "failed to get connection from pool";
            error!(error = %e, err_msg);
            RpcError::Other(err_msg.to_string())
        })?;

        match conn.query(&stmt.sql, &[]).await {
            Ok(rows) => {
                if rows.is_empty() {
                    Ok(QueryResult::default())
                } else {
                    let cols = rows
                        .get(0)
                        .unwrap()
                        .columns()
                        .iter()
                        .enumerate()
                        .map(|(i, c)| Column {
                            name: c.name().to_string(),
                            ordinal: i as u32,
                            db_type: c.type_().name().to_string(),
                        })
                        .collect::<Vec<Column>>();
                    match encode_result_set(&rows) {
                        Ok(buf) => Ok(QueryResult {
                            columns: cols,
                            num_rows: rows.len() as u64,
                            error: None,
                            rows: buf,
                        }),
                        Err(e) => Ok(QueryResult {
                            error: Some(e.into()),
                            ..Default::default()
                        }),
                    }
                }
            }
            Err(db_err) => {
                error!(
                    statement = ?stmt,
                    error = %db_err,
                    "Error executing query"
                );
                Ok(QueryResult {
                    error: Some(DbError::from(db_err).into()),
                    ..Default::default()
                })
            }
        }
    }
}

fn encode_result_set(rows: &[tokio_postgres::Row]) -> Result<Vec<u8>, DbError> {
    let mut buf = Vec::with_capacity(rows.len() * 2);
    let mut enc = minicbor::Encoder::new(&mut buf);
    types::encode_rows(&mut enc, rows).map_err(|e| DbError::Encoding(e.to_string()))?;
    Ok(buf)
}
