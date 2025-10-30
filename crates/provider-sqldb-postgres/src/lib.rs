#![cfg(not(doctest))]

//! SQL-powered database access provider implementing `wasmcloud:postgres` for connecting
//! to Postgres clusters.
//!
//! This implementation is multi-threaded and operations between different actors
//! use different connections and can run in parallel.
//!

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context as _, Result};
use bytes::Bytes;
use deadpool_postgres::Pool;
use futures::TryStreamExt as _;
use sha2::{Digest as _, Sha256};
use tokio::sync::RwLock;
use tokio_postgres::types::Type as PgType;
use tracing::{error, instrument, warn, Instrument};
use ulid::Ulid;

use wasmcloud_provider_sdk::{
    get_connection, propagate_trace_for_ctx, run_provider, LinkConfig, LinkDeleteInfo, Provider,
};
use wasmcloud_provider_sdk::{initialize_observability, serve_provider_exports};

mod bindings;
use bindings::{
    into_result_row, transaction::Transaction, PgValue, PreparedStatementExecError,
    PreparedStatementToken, QueryError, ResultRow, StatementPrepareError,
};

mod config;
use config::{extract_prefixed_conn_config, ConnectionCreateOptions};

use wasmcloud_provider_sdk::Context;
use wit_bindgen_wrpc::wrpc_transport::{ResourceBorrow, ResourceOwn};

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

#[derive(Clone, Default)]
pub struct PostgresProvider {
    /// Database connections indexed by source ID name
    connections: Arc<RwLock<HashMap<SourceId, PostgresConnection>>>,
    /// Shared connection pools indexed by configuration hash
    shared_connections: Arc<RwLock<HashMap<SharedConnectionKey, Pool>>>,
    /// Lookup of prepared statements to the statement and the source ID that prepared them
    prepared_statements: Arc<RwLock<HashMap<PreparedStatementToken, PreparedStatementInfo>>>,
    /// Lookup of transaction ID (as Bytes) to the [TransactionManager] which stores statements for execution on commit
    transactions: Arc<RwLock<HashMap<Bytes, TransactionManager>>>,
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
        initialize_observability!(
            PostgresProvider::name(),
            std::env::var_os("PROVIDER_SQLDB_POSTGRES_FLAMEGRAPH_PATH")
        );
        let provider = PostgresProvider::default();
        let shutdown = run_provider(provider.clone(), PostgresProvider::name())
            .await
            .context("failed to run provider")?;
        let connection = get_connection();
        let wrpc = connection
            .get_wrpc_client(connection.provider_key())
            .await?;
        serve_provider_exports(&wrpc, provider, shutdown, bindings::serve)
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
    #[instrument(level = "debug", skip_all, fields(source_id))]
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
            .in_current_span()
            .await
            .map_err(|e| QueryError::Unexpected(format!("failed to perform query: {e}")))?;

        // todo(fix): once async stream support is available & in contract
        // replace this with a mapped stream
        rows.map_ok(into_result_row)
            .try_collect::<Vec<_>>()
            .in_current_span()
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

impl Provider for PostgresProvider {
    /// Handle being linked to a source (likely a component) as a target
    ///
    /// Components are expected to provide references to named configuration via link definitions
    /// which contain keys named `POSTGRES_*` detailing configuration for connecting to Postgres.
    #[instrument(level = "debug", skip_all, fields(source_id))]
    async fn receive_link_config_as_target(
        &self,
        link_config @ LinkConfig { source_id, .. }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        // Attempt to parse a configuration from the map with the prefix POSTGRES_
        let Some(db_cfg) = extract_prefixed_conn_config("POSTGRES_", &link_config) else {
            // If we failed to find a config on the link, then we
            warn!(source_id, "no link-level DB configuration");
            return Ok(());
        };

        // Check if connection sharing is enabled
        let share_connections = if let Some(value) =
            link_config.config.get(CONFIG_SHARE_CONNECTIONS_BY_URL_KEY)
        {
            matches!(value.to_lowercase().as_str(), "true" | "yes")
        } else if let Some(secret) = link_config.secrets.get(CONFIG_SHARE_CONNECTIONS_BY_URL_KEY) {
            if let Some(value) = secret.as_string() {
                matches!(value.to_lowercase().as_str(), "true" | "yes")
            } else {
                false
            }
        } else {
            false
        };

        // Create a pool if one isn't already present for this particular source
        if let Err(error) = self.ensure_pool(source_id, db_cfg, share_connections).await {
            error!(?error, source_id, "failed to create connection");
        };

        Ok(())
    }

    /// Handle notification that a link is dropped
    ///
    /// Generally we can release the resources (connections) associated with the source
    #[instrument(level = "info", skip_all, fields(source_id = info.get_source_id()))]
    async fn delete_link_as_target(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let source_id = info.get_source_id();
        let mut prepared_statements = self.prepared_statements.write().await;
        prepared_statements.retain(|_stmt_token, (_query, _statement, src_id)| src_id != source_id);
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

pub struct TransactionManager {
    /// The resource that this transaction manager is associated with. This must
    /// be stored for the lifetime of the transaction and then dropped when the transaction is committed or rolled back.
    #[allow(unused)]
    resource: ResourceOwn<Transaction>,
    queries: Vec<(String, Vec<PgValue>)>,
}

impl TransactionManager {
    pub fn new(resource: ResourceOwn<Transaction>) -> Self {
        Self {
            resource,
            queries: Vec::new(),
        }
    }

    /// Add a query to be executed when the transaction is committed
    pub fn add_query(&mut self, query: String, params: Vec<PgValue>) {
        self.queries.push((query, params));
    }

    #[instrument(level = "debug", skip_all)]
    async fn commit(self, tx: deadpool_postgres::Transaction<'_>) -> anyhow::Result<()> {
        for (query, params) in self.queries {
            // NOTE: Transactions that fail are automatically rolled back
            let statement = tx
                .prepare_cached(&query)
                .await
                .context("failed to prepare statement in transaction, rolling back")?;
            tx.execute_raw(&statement, params)
                .await
                .context("failed to execute query in transaction, rolling back")?;
        }

        tx.commit().await.context("failed to commit transaction")?;

        Ok(())
    }
}

impl bindings::transaction::Handler<Option<Context>> for PostgresProvider {}
/// Implement the `wasmcloud:postgres/prepared` interface for [`PostgresProvider`]
impl bindings::transaction::HandlerTransaction<Option<Context>> for PostgresProvider {
    #[instrument(level = "debug", skip_all)]
    async fn new(&self, ctx: Option<Context>) -> anyhow::Result<ResourceOwn<Transaction>> {
        propagate_trace_for_ctx!(ctx);

        let Some(ctx) = ctx else {
            bail!("unexpectedly missing invocation context");
        };

        let Some(source_id) = ctx.component else {
            bail!("unexpectedly missing source ID");
        };

        // Ensure that we have a connection pool for the source ID
        let connections = self.connections.read().await;
        let _pool = connections.get(&source_id).ok_or_else(|| {
            anyhow::anyhow!(
                "missing connection pool for source [{source_id}] while creating transaction"
            )
        })?;
        drop(connections);

        let transaction_id = Ulid::new().to_string();
        let resource = ResourceOwn::new(transaction_id.clone());
        let key: &Bytes = resource.as_ref();

        self.transactions
            .write()
            .await
            .insert(key.to_owned(), TransactionManager::new(resource.clone()));

        Ok(resource)
    }

    #[instrument(level = "debug", skip_all)]
    async fn query(
        &self,
        ctx: Option<Context>,
        transaction: ResourceBorrow<Transaction>,
        query: String,
        params: Vec<PgValue>,
    ) -> anyhow::Result<Result<(), QueryError>> {
        propagate_trace_for_ctx!(ctx);
        let key: &Bytes = transaction.as_ref();

        match self.transactions.write().await.get_mut(key) {
            Some(manager) => {
                manager.add_query(query, params);
                Ok(Ok(()))
            }
            None => Ok(Err(QueryError::Unexpected("missing transaction".into()))),
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn query_batch(
        &self,
        ctx: Option<Context>,
        transaction: ::wit_bindgen_wrpc::wrpc_transport::ResourceBorrow<Transaction>,
        query: String,
    ) -> anyhow::Result<Result<(), QueryError>> {
        propagate_trace_for_ctx!(ctx);
        let key: &Bytes = transaction.as_ref();

        match self.transactions.write().await.get_mut(key) {
            Some(manager) => {
                manager.add_query(query, Vec::with_capacity(0));
                Ok(Ok(()))
            }
            None => Ok(Err(QueryError::Unexpected("missing transaction".into()))),
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn commit(
        &self,
        ctx: Option<wasmcloud_provider_sdk::Context>,
        transaction: ResourceOwn<Transaction>,
    ) -> anyhow::Result<Result<(), QueryError>> {
        propagate_trace_for_ctx!(ctx);

        let Some(ctx) = ctx else {
            return Ok(Err(QueryError::Unexpected(
                "unexpectedly missing invocation context".into(),
            )));
        };

        let Some(source_id) = ctx.component else {
            return Ok(Err(QueryError::Unexpected(
                "unexpectedly missing source ID".into(),
            )));
        };

        // Ensure that we have a connection pool for the source ID
        let connections = self.connections.read().await;
        let Some(pool) = connections.get(&source_id) else {
            return Ok(Err(QueryError::Unexpected(format!(
                "missing connection pool for source [{source_id}] while querying"
            ))));
        };

        let mut client = match pool.get().await {
            Ok(client) => client,
            Err(e) => {
                return Ok(Err(QueryError::Unexpected(format!(
                    "failed to build client from pool: {e}"
                ))));
            }
        };

        let tx = match client.build_transaction().start().await {
            Ok(tx) => tx,
            Err(e) => {
                return Ok(Err(QueryError::Unexpected(format!(
                    "failed to start transaction: {e}"
                ))));
            }
        };

        let key: &Bytes = transaction.as_ref();

        let manager = match self.transactions.write().await.remove(key) {
            Some(manager) => manager,
            None => {
                return Ok(Err(QueryError::Unexpected(
                    "failed to find transaction".into(),
                )));
            }
        };

        match manager.commit(tx).await {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(QueryError::Unexpected(format!(
                "failed to commit transaction: {e}"
            )))),
        }
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
