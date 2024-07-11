//! SQL-powered database access provider implementing `wasmcloud:postgres` for connecting
//! to Postgres clusters.
//!
//! This implementation is multi-threaded and operations between different actors
//! use different connections and can run in parallel.
//!

use core::pin::pin;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use deadpool_postgres::Pool;
use futures::{stream, StreamExt as _, TryStreamExt as _};
use tokio::select;
use tokio::sync::RwLock;
use tokio_postgres::Statement;
use tracing::{debug, error, instrument, warn};
use ulid::Ulid;

use wasmcloud_provider_sdk::initialize_observability;
use wasmcloud_provider_sdk::{
    get_connection, propagate_trace_for_ctx, run_provider, LinkConfig, Provider,
};

mod bindings;
use bindings::{
    into_result_row, PgValue, PreparedStatementExecError, PreparedStatementToken, QueryError,
    ResultRow, StatementPrepareError,
};

mod config;
use config::{parse_prefixed_config_from_map, ConnectionCreateOptions};

use wasmcloud_provider_sdk::Context;

#[derive(Clone, Default)]
pub struct PostgresProvider {
    /// Database connections indexed by source ID name
    connections: Arc<RwLock<HashMap<String, Pool>>>,
    /// Lookup of prepared statements to the statement and the source ID that prepared them
    prepared_statements: Arc<RwLock<HashMap<PreparedStatementToken, (Statement, String)>>>,
}

impl PostgresProvider {
    fn name() -> &'static str {
        "sqldb-postgres-provider"
    }

    /// Run [`PostgresProvider`] as a wasmCloud provider
    pub async fn run() -> anyhow::Result<()> {
        initialize_observability!(
            PostgresProvider::name(),
            std::env::var_os("PROVIDER_SQLDB_POSTGRES_FLAMEGRAPH_PATH")
        );
        let provider = PostgresProvider::default();
        let shutdown = run_provider(provider.clone(), PostgresProvider::name())
            .await
            .context("failed to run provider")?;
        let connection = get_connection();
        let invocations = bindings::serve(
            &connection.get_wrpc_client(connection.provider_key()),
            provider,
        )
        .await
        .context("failed to serve exports")?;
        let mut invocations = stream::select_all(invocations.into_iter().map(
            |(instance, name, invocations)| {
                invocations
                    .try_buffer_unordered(256)
                    .map(move |res| (instance, name, res))
            },
        ));
        let mut shutdown = pin!(shutdown);
        loop {
            select! {
                Some((instance, name, res)) = invocations.next() => {
                    if let Err(err) = res {
                        warn!(?err, instance, name, "failed to serve invocation");
                    } else {
                        debug!(instance, name, "successfully served invocation");
                    }
                },
                () = &mut shutdown => {
                    return Ok(())
                }
            }
        }
    }

    /// Create and store a connection pool, if not already present
    async fn ensure_pool(
        &self,
        source_id: &str,
        create_opts: ConnectionCreateOptions,
    ) -> Result<()> {
        // Exit early if a pool with the given source ID is already present
        {
            let connections = self.connections.read().await;
            if connections.get(source_id).is_some() {
                return Ok(());
            }
        }

        // Build the new connection pool
        let runtime = Some(deadpool_postgres::Runtime::Tokio1);
        let tls_required = create_opts.tls_required;
        let cfg = deadpool_postgres::Config::from(create_opts);
        let pool = if tls_required {
            create_tls_pool(cfg, runtime)
        } else {
            cfg.create_pool(runtime, tokio_postgres::NoTls)
                .context("failed to create non-TLS postgres pool")
        }?;

        // Save the newly created connection to the pool
        let mut connections = self.connections.write().await;
        connections.insert(source_id.into(), pool);
        Ok(())
    }

    /// Perform a query
    async fn do_query(
        &self,
        source_id: &str,
        query: &str,
        params: Vec<PgValue>,
    ) -> Result<Vec<ResultRow>, QueryError> {
        let connections = self.connections.read().await;
        let pool = connections.get(source_id).ok_or_else(|| {
            QueryError::Unexpected(format!(
                "missing connection pool for source [{source_id}] while querying"
            ))
        })?;

        let client = pool.get().await.map_err(|e| {
            QueryError::Unexpected(format!("failed to build client from pool: {e}"))
        })?;

        let rows = client
            .query_raw(query, params)
            .await
            .map_err(|e| QueryError::Unexpected(format!("failed to perform query: {e}")))?;

        // todo(fix): once async stream support is available & in contract
        // replace this with a mapped stream
        rows.map_ok(into_result_row)
            .try_collect::<Vec<_>>()
            .await
            .map_err(|e| QueryError::Unexpected(format!("failed to evaluate full row: {e}")))
    }

    /// Prepare a statement
    async fn do_statement_prepare(
        &self,
        connection_token: &str,
        query: &str,
    ) -> Result<PreparedStatementToken, StatementPrepareError> {
        let connections = self.connections.read().await;
        let pool = connections.get(connection_token).ok_or_else(|| {
            StatementPrepareError::Unexpected(format!(
                "failed to find connection pool for token [{connection_token}]"
            ))
        })?;

        let client = pool.get().await.map_err(|e| {
            StatementPrepareError::Unexpected(format!("failed to build client from pool: {e}"))
        })?;

        let statement = client.prepare(query).await.map_err(|e| {
            StatementPrepareError::Unexpected(format!("failed to prepare query: {e}"))
        })?;

        let statement_token = format!("prepared-statement-{}", Ulid::new().to_string());

        let mut prepared_statements = self.prepared_statements.write().await;
        prepared_statements.insert(
            statement_token.clone(),
            (statement, connection_token.into()),
        );

        Ok(statement_token)
    }

    /// Execute a prepared statement, returning the number of rows affected
    async fn do_statement_execute(
        &self,
        statement_token: &str,
        params: Vec<PgValue>,
    ) -> Result<u64, PreparedStatementExecError> {
        let statements = self.prepared_statements.read().await;
        let (statement, connection_token) = statements.get(statement_token).ok_or_else(|| {
            PreparedStatementExecError::Unexpected(format!(
                "missing prepared statement with statement ID [{statement_token}]"
            ))
        })?;

        let connections = self.connections.read().await;
        let pool = connections.get(connection_token).ok_or_else(|| {
            PreparedStatementExecError::Unexpected(format!(
                "missing connection pool for token [{connection_token}], statement ID [{statement_token}]"
            ))
        })?;
        let client = pool.get().await.map_err(|e| {
            PreparedStatementExecError::Unexpected(format!("failed to build client from pool: {e}"))
        })?;

        let rows_affected = client.execute_raw(statement, params).await.map_err(|e| {
            PreparedStatementExecError::Unexpected(format!(
                "failed to execute prepared statement with token [{statement_token}]: {e}"
            ))
        })?;

        Ok(rows_affected)
    }
}

impl Provider for PostgresProvider {
    /// Handle being linked to a source (likely a component) as a target
    ///
    /// Components are expected to provide references to named configuration via link definitions
    /// which contain keys named `POSTGRES_*` detailing configuration for connecting to Postgres.
    #[instrument(level = "debug", skip_all, fields(source_id))]
    async fn receive_link_config_as_target(
        &self,
        LinkConfig {
            source_id, config, ..
        }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        // Attempt to parse a configuration from the map with the prefix POSTGRES_
        let Some(db_cfg) = parse_prefixed_config_from_map("POSTGRES_", config) else {
            // If we failed to find a config on the link, then we
            warn!(source_id, "no link-level DB configuration");
            return Ok(());
        };

        // Create a pool if one isn't already present for this particular source
        if let Err(error) = self.ensure_pool(source_id, db_cfg).await {
            error!(?error, source_id, "failed to create connection");
        };

        Ok(())
    }

    /// Handle notification that a link is dropped
    ///
    /// Generally we can release the resources (connections) associated with the source
    #[instrument(level = "debug", skip_all, fields(source_id))]
    async fn delete_link(&self, source_id: &str) -> anyhow::Result<()> {
        let mut prepared_statements = self.prepared_statements.write().await;
        prepared_statements.retain(|_stmt_token, (_conn, src_id)| source_id != *src_id);
        drop(prepared_statements);
        let mut connections = self.connections.write().await;
        connections.remove(source_id);
        drop(connections);
        Ok(())
    }

    /// Handle shutdown request by closing all connections
    #[instrument(level = "debug", skip_all)]
    async fn shutdown(&self) -> anyhow::Result<()> {
        let mut prepared_statements = self.prepared_statements.write().await;
        prepared_statements.drain();
        let mut connections = self.connections.write().await;
        connections.drain();
        Ok(())
    }
}

/// Implement the `wasmcloud:postgres/query` interface for [`PostgresProvider`]
impl bindings::query::Handler<Option<Context>> for PostgresProvider {
    #[instrument(level = "debug", skip_all, fields(query))]
    async fn query(
        &self,
        ctx: Option<Context>,
        query: String,
        params: Vec<PgValue>,
    ) -> Result<Result<Vec<ResultRow>, QueryError>> {
        propagate_trace_for_ctx!(ctx);
        let Some(Context {
            component: Some(source_id),
            ..
        }) = ctx
        else {
            return Ok(Err(QueryError::Unexpected(
                "unexpectedly missing source ID".into(),
            )));
        };

        Ok(self.do_query(&source_id, &query, params).await)
    }
}

/// Implement the `wasmcloud:postgres/prepared` interface for [`PostgresProvider`]
impl bindings::prepared::Handler<Option<Context>> for PostgresProvider {
    #[instrument(level = "debug", skip_all, fields(query))]
    async fn prepare(
        &self,
        ctx: Option<Context>,
        query: String,
    ) -> Result<Result<PreparedStatementToken, StatementPrepareError>> {
        propagate_trace_for_ctx!(ctx);
        let Some(Context {
            component: Some(source_id),
            ..
        }) = ctx
        else {
            return Ok(Err(StatementPrepareError::Unexpected(
                "unexpectedly missing source ID".into(),
            )));
        };
        Ok(self.do_statement_prepare(&source_id, &query).await)
    }

    #[instrument(level = "debug", skip_all, fields(statement_token))]
    async fn exec(
        &self,
        ctx: Option<Context>,
        statement_token: PreparedStatementToken,
        params: Vec<PgValue>,
    ) -> Result<Result<u64, PreparedStatementExecError>> {
        propagate_trace_for_ctx!(ctx);
        Ok(self.do_statement_execute(&statement_token, params).await)
    }
}

#[cfg(feature = "rustls")]
fn create_tls_pool(
    cfg: deadpool_postgres::Config,
    runtime: Option<deadpool_postgres::Runtime>,
) -> Result<Pool> {
    cfg.create_pool(
        runtime,
        tokio_postgres_rustls::MakeRustlsConnect::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(rustls::RootCertStore::empty())
                .with_no_client_auth(),
        ),
    )
    .context("failed to create TLS-enabled connection pool")
}

#[cfg(not(feature = "rustls"))]
fn create_tls_pool(
    _cfg: deadpool_postgres::Config,
    _runtime: Option<deadpool_postgres::Runtime>,
) -> Result<Pool> {
    anyhow::bail!("cannot build TLS connections without rustls feature")
}
