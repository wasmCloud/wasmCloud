#![cfg(not(doctest))]

//! SQL-powered database access provider implementing `wasmcloud:postgres` for connecting
//! to Postgres clusters.
//!
//! This implementation is multi-threaded and operations between different actors
//! use different connections and can run in parallel.
//!

use std::collections::HashMap;
use std::sync::Arc;

use crate::bindings::ext::exports::wrpc::extension::{
    configurable::{self, InterfaceConfig},
    manageable,
};
use anyhow::{Context as _, Result};
use deadpool_postgres::Pool;
use futures::TryStreamExt as _;
use sha2::{Digest as _, Sha256};
use tokio::sync::RwLock;
use tokio_postgres::types::Type as PgType;
use tracing::{error, instrument, warn};
use ulid::Ulid;
use wasmcloud_provider_sdk::{
    core::secrets::SecretValue,
    get_connection, initialize_observability, propagate_trace_for_ctx, run_provider,
    serve_provider_exports,
    types::{BindRequest, BindResponse, HealthCheckResponse},
    Context,
};
mod bindings;
use bindings::{
    into_result_row, PgValue, PreparedStatementExecError, PreparedStatementToken, QueryError,
    ResultRow, StatementPrepareError,
};

mod config;
use config::{extract_prefixed_conn_config, ConnectionCreateOptions};

/// Whether to share connections by URL
///
/// This option indicates that URLs with identical connection configurations will be shared/reused by
/// components that are linked with the same configurations
const CONFIG_SHARE_CONNECTIONS_BY_URL_KEY: &str = "POSTGRES_SHARE_CONNECTIONS_BY_URL";

/// A unique identifier for a created connection
type SourceId = String;

/// A query used in the process of creating a prepared statement
type PreparedStatementQuery = String;

/// Parameters determined to be used in a statement
///
/// This value is usually constructed after running a prepare against a given
/// client from a given pool, and saving the relevant type information.
type StatementParams = Vec<PgType>;

/// Information about a given prepared statement
type PreparedStatementInfo = (PreparedStatementQuery, StatementParams, SourceId);

/// Shared connection keys are keys that identify shared connections
///
/// This is the hash of the connection configuration, to avoid printing credentials inadvertently.
type SharedConnectionKey = String;

/// Type of postgres connection - either direct or shared
#[derive(Clone)]
enum PostgresConnection {
    /// Direct connection to a pool
    Direct(Pool),
    /// Shared connection, identified by the hash of the connection configuration
    Shared(String),
}

#[derive(Clone)]
pub struct PostgresProvider {
    /// Database connections indexed by source ID name
    connections: Arc<RwLock<HashMap<SourceId, PostgresConnection>>>,
    /// Shared connection pools indexed by configuration hash
    shared_connections: Arc<RwLock<HashMap<SharedConnectionKey, Pool>>>,
    /// Lookup of prepared statements to the statement and the source ID that prepared them
    prepared_statements: Arc<RwLock<HashMap<PreparedStatementToken, PreparedStatementInfo>>>,
    /// Channel to signal provider shutdown
    quit_tx: tokio::sync::broadcast::Sender<()>,
}

impl PostgresProvider {
    fn new(quit_tx: tokio::sync::broadcast::Sender<()>) -> Self {
        Self {
            connections: Arc::default(),
            shared_connections: Arc::default(),
            prepared_statements: Arc::default(),
            quit_tx,
        }
    }
}

impl PostgresProvider {
    fn name() -> &'static str {
        "sqldb-postgres-provider"
    }

    /// Generate a connection string from ConnectionCreateOptions for hashing
    fn connection_string_for_hashing(opts: &ConnectionCreateOptions) -> String {
        format!(
            "postgres://{}:{}@{}:{}/{}?tls_required={}&pool_size={:?}",
            opts.username,
            opts.password,
            opts.host,
            opts.port,
            opts.database,
            opts.tls_required,
            opts.pool_size
        )
    }

    /// Get a pool for the given source_id, resolving shared connections if necessary
    async fn get_pool(&self, source_id: &str) -> Result<Pool, String> {
        let connections = self.connections.read().await;
        let connection = connections
            .get(source_id)
            .ok_or_else(|| format!("missing connection pool for source [{source_id}]"))?;

        match connection {
            PostgresConnection::Direct(pool) => Ok(pool.clone()),
            PostgresConnection::Shared(key) => {
                let shared = self.shared_connections.read().await;
                shared
                    .get(key)
                    .cloned()
                    .ok_or_else(|| format!("no shared connection found with key [{key}]"))
            }
        }
    }

    /// Run [`PostgresProvider`] as a wasmCloud provider
    pub async fn run() -> anyhow::Result<()> {
        let (shutdown, quit_tx) = run_provider(PostgresProvider::name(), None)
            .await
            .context("failed to run provider")?;
        let provider = PostgresProvider::new(quit_tx);
        let connection = get_connection();
        let (main_client, ext_client) = connection.get_wrpc_clients_for_serving().await?;
        serve_provider_exports(
            &main_client,
            &ext_client,
            provider,
            shutdown,
            bindings::serve,
            bindings::ext::serve,
        )
        .await
        .context("failed to serve provider exports")
    }

    /// Create and store a connection pool, if not already present
    async fn ensure_pool(
        &self,
        source_id: &str,
        create_opts: ConnectionCreateOptions,
        share_connections: bool,
    ) -> Result<()> {
        // If sharing is enabled, check if we already have a shared connection for this configuration
        if share_connections {
            let connection_string = Self::connection_string_for_hashing(&create_opts);
            let shared_key = format!("{:X}", Sha256::digest(&connection_string));

            // Check if we already have this shared connection
            {
                let shared_connections = self.shared_connections.read().await;
                if shared_connections.contains_key(&shared_key) {
                    let mut connections = self.connections.write().await;
                    connections.insert(source_id.into(), PostgresConnection::Shared(shared_key));
                    return Ok(());
                }
            }
        }

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
        let cfg = deadpool_postgres::Config::from(create_opts.clone());
        let pool = if tls_required {
            create_tls_pool(cfg, runtime)
        } else {
            cfg.create_pool(runtime, tokio_postgres::NoTls)
                .context("failed to create non-TLS postgres pool")
        }?;

        if share_connections {
            // Store as shared connection
            let connection_string = Self::connection_string_for_hashing(&create_opts);
            let shared_key = format!("{:X}", Sha256::digest(&connection_string));

            // Store the shared connection first, then reference it
            let mut shared_connections = self.shared_connections.write().await;
            shared_connections.insert(shared_key.clone(), pool);
            drop(shared_connections);

            let mut connections = self.connections.write().await;
            connections.insert(source_id.into(), PostgresConnection::Shared(shared_key));
        } else {
            // Store as direct connection
            let mut connections = self.connections.write().await;
            connections.insert(source_id.into(), PostgresConnection::Direct(pool));
        }

        Ok(())
    }

    /// Perform a query
    async fn do_query(
        &self,
        source_id: &str,
        query: &str,
        params: Vec<PgValue>,
    ) -> Result<Vec<ResultRow>, QueryError> {
        let pool = self.get_pool(source_id).await.map_err(|e| {
            QueryError::Unexpected(format!(
                "missing connection pool for source [{source_id}] while querying: {e}"
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

    /// Perform a raw query
    async fn do_query_batch(&self, source_id: &str, query: &str) -> Result<(), QueryError> {
        let pool = self.get_pool(source_id).await.map_err(|e| {
            QueryError::Unexpected(format!(
                "missing connection pool for source [{source_id}] while querying: {e}"
            ))
        })?;

        let client = pool.get().await.map_err(|e| {
            QueryError::Unexpected(format!("failed to build client from pool: {e}"))
        })?;

        client
            .batch_execute(query)
            .await
            .map_err(|e| QueryError::Unexpected(format!("failed to perform query: {e}")))?;

        Ok(())
    }

    /// Prepare a statement
    async fn do_statement_prepare(
        &self,
        source_id: &str,
        query: &str,
    ) -> Result<PreparedStatementToken, StatementPrepareError> {
        let pool = self.get_pool(source_id).await.map_err(|e| {
            StatementPrepareError::Unexpected(format!(
                "failed to find connection pool for token [{source_id}]: {e}"
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
            (query.into(), statement.params().into(), source_id.into()),
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
        let (query, types, source_id) = statements.get(statement_token).ok_or_else(|| {
            PreparedStatementExecError::Unexpected(format!(
                "missing prepared statement with statement ID [{statement_token}]"
            ))
        })?;

        let pool = self.get_pool(source_id).await.map_err(|e| {
            PreparedStatementExecError::Unexpected(format!(
                "missing connection pool for token [{source_id}], statement ID [{statement_token}]: {e}"
            ))
        })?;
        let client = pool.get().await.map_err(|e| {
            PreparedStatementExecError::Unexpected(format!("failed to build client from pool: {e}"))
        })?;

        // Since the pool is not aware of already created statements managed by tokio_postgres,
        // we may have pulled a client that has not already has this statement prepared,
        // so we must prepare, just in case.
        let statement = client
            .statement_cache
            .prepare_typed(&client, query, types)
            .await
            .map_err(|e| {
                PreparedStatementExecError::Unexpected(format!(
                    "failed to prepare statement for client in pool: {e}"
                ))
            })?;

        let rows_affected = client.execute_raw(&statement, params).await.map_err(|e| {
            PreparedStatementExecError::Unexpected(format!(
                "failed to execute prepared statement with token [{statement_token}]: {e}"
            ))
        })?;

        Ok(rows_affected)
    }
}

impl manageable::Handler<Option<Context>> for PostgresProvider {
    async fn bind(
        &self,
        _cx: Option<Context>,
        _req: BindRequest,
    ) -> anyhow::Result<Result<BindResponse, String>> {
        Ok(Ok(BindResponse {
            identity_token: None,
            provider_xkey: Some(get_connection().provider_xkey.public_key().into()),
        }))
    }

    async fn health_request(
        &self,
        _cx: Option<Context>,
    ) -> anyhow::Result<Result<HealthCheckResponse, String>> {
        Ok(Ok(HealthCheckResponse {
            healthy: true,
            message: Some("OK".to_string()),
        }))
    }

    /// Handle shutdown request by closing all connections and signaling shutdown
    async fn shutdown(&self, _cx: Option<Context>) -> anyhow::Result<Result<(), String>> {
        let mut prepared_statements = self.prepared_statements.write().await;
        prepared_statements.drain();
        let mut connections = self.connections.write().await;
        connections.drain();
        // Signal the provider to shut down
        let _ = self.quit_tx.send(());
        Ok(Ok(()))
    }
}

impl configurable::Handler<Option<Context>> for PostgresProvider {
    #[instrument(level = "debug", skip_all)]
    async fn update_base_config(
        &self,
        _cx: Option<Context>,
        config: wasmcloud_provider_sdk::types::BaseConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let flamegraph_path = config
            .config
            .iter()
            .find(|(k, _)| k == "FLAMEGRAPH_PATH")
            .map(|(_, v)| v.clone())
            .or_else(|| std::env::var("PROVIDER_SQLDB_POSTGRES_FLAMEGRAPH_PATH").ok());
        initialize_observability!(Self::name(), flamegraph_path, config.config);

        Ok(Ok(()))
    }

    #[instrument(level = "debug", skip_all, fields(source_id))]
    async fn update_interface_export_config(
        &self,
        _cx: Option<Context>,
        source_id: String,
        _link_name: String,
        link_config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        // Attempt to parse a configuration from the map with the prefix POSTGRES_
        let Some(db_cfg) = extract_prefixed_conn_config("POSTGRES_", &link_config) else {
            // If we failed to find a config on the link, then we
            warn!(source_id, "no link-level DB configuration");
            return Ok(Ok(()));
        };

        // Convert config Vec to HashMap for easier access
        let config: HashMap<String, String> = link_config.config.iter().cloned().collect();

        // Check if connection sharing is enabled
        let share_connections = if let Some(value) = config.get(CONFIG_SHARE_CONNECTIONS_BY_URL_KEY)
        {
            matches!(value.to_lowercase().as_str(), "true" | "yes")
        } else if let Some(secret) = link_config
            .secrets
            .as_ref()
            .and_then(|s| {
                s.iter()
                    .find(|(k, _)| k == CONFIG_SHARE_CONNECTIONS_BY_URL_KEY)
            })
            .and_then(|(_, v)| {
                let secret: SecretValue = v.into();
                secret.as_string().map(String::from)
            })
        {
            matches!(secret.to_lowercase().as_str(), "true" | "yes")
        } else {
            false
        };

        // Create a pool if one isn't already present for this particular source
        if let Err(error) = self
            .ensure_pool(&source_id, db_cfg, share_connections)
            .await
        {
            error!(?error, source_id, "failed to create connection");
        };

        Ok(Ok(()))
    }

    async fn update_interface_import_config(
        &self,
        _cx: Option<Context>,
        _target_id: String,
        _link_name: String,
        _config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(Ok(()))
    }

    async fn delete_interface_import_config(
        &self,
        _cx: Option<Context>,
        _target_id: String,
        _link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(Ok(()))
    }

    #[instrument(level = "info", skip_all, fields(source_id))]
    async fn delete_interface_export_config(
        &self,
        _cx: Option<Context>,
        source_id: String,
        _link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        let mut prepared_statements = self.prepared_statements.write().await;
        prepared_statements
            .retain(|_stmt_token, (_query, _statement, src_id)| *src_id != source_id);
        drop(prepared_statements);
        let mut connections = self.connections.write().await;
        connections.remove(&source_id);
        drop(connections);
        Ok(Ok(()))
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

    #[instrument(level = "debug", skip_all, fields(query))]
    async fn query_batch(
        &self,
        ctx: Option<Context>,
        query: String,
    ) -> Result<Result<(), QueryError>> {
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

        Ok(self.do_query_batch(&source_id, &query).await)
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

fn create_tls_pool(
    cfg: deadpool_postgres::Config,
    runtime: Option<deadpool_postgres::Runtime>,
) -> Result<Pool> {
    let mut store = rustls::RootCertStore::empty();
    store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    cfg.create_pool(
        runtime,
        tokio_postgres_rustls::MakeRustlsConnect::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(store)
                .with_no_client_auth(),
        ),
    )
    .context("failed to create TLS-enabled connection pool")
}
