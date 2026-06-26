//! # Multiplexed `wasmcloud:postgres` (implements-routed)
//!
//! Binds `wasmcloud:postgres` via the component-model `(implements ..)` /
//! `named_imports` mechanism so a single component can import the query/prepared
//! interfaces multiple times and have each import backed by a *different*
//! credentialed connection (e.g. one `query` import as `team-a`, another as
//! `team-b`, each with its own database role).
//!
//! Each named import resolves to a [`PgId`] (the wasmtime "implements id"): a
//! connection pool built from that interface's `url`. Pools are shared across
//! workload binds with the same URL by the [`Multiplexer`]. The shared query
//! helpers (`execute_query`, `pg_error_string`) and the legacy per-component
//! path live in the parent module.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod};
use tokio::sync::RwLock;
use url::Url;

use crate::engine::ctx::ActiveCtx;
use crate::plugin::multiplex::{BackendProvider, Multiplexer};
use crate::wit::WitInterface;

use super::{
    DEFAULT_POOL_SIZE, PLUGIN_POSTGRES_ID, PgValue, PreparedEntry, PreparedStatementExecError,
    QueryError, ResultRow, StatementPrepareError, WasmcloudPostgres, bindings, execute_query,
    extract_pool_size, extract_tls_requirement, pg_error_string,
};

/// The single `wasmcloud:postgres` backend type. Unlike `wasi:keyvalue` (which
/// multiplexes across redis/NATS/in-memory), every postgres backend is a
/// connection pool built from a URL, so there is one provider and it is the
/// default for any named interface that omits `config.backend`.
const POSTGRES_BACKEND: &str = "postgres";

/// The "implements id" for a named `wasmcloud:postgres` import: the connection
/// pool that import is bound to. Each named host interface's `url` carries its
/// own credentials and database, so two imports for different teams talk to
/// postgres with different roles. Cloning shares the underlying pool (it is
/// `Arc`-backed); all binds the multiplexer routes to the same URL reuse one
/// pool. Prepared statements created through a `PgId` are tracked in the
/// plugin's shared table (tagged with the creating component) so workload unbind
/// reaps them, just like the legacy path.
#[derive(Clone)]
pub struct PgId {
    pool: Pool,
}

/// Build a deadpool connection pool from a full postgres URL (credentials +
/// database). Unlike the legacy bouncer config, the URL here is complete: the
/// named interface owns its role and target database.
fn build_pool_from_url(url: &str) -> anyhow::Result<Pool> {
    let parsed = Url::parse(url).context("failed to parse postgres URL")?;
    let pg_config: tokio_postgres::Config =
        url.parse().context("failed to parse postgres config")?;
    let pool_size = extract_pool_size(&parsed);
    let mgr_config = ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    };
    let pool = if extract_tls_requirement(&parsed) {
        let tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            })
            .with_no_client_auth();
        let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);
        let mgr = Manager::from_config(pg_config, tls, mgr_config);
        Pool::builder(mgr)
            .max_size(pool_size)
            .build()
            .context("failed to build TLS connection pool")?
    } else {
        let mgr = Manager::from_config(pg_config, tokio_postgres::NoTls, mgr_config);
        Pool::builder(mgr)
            .max_size(pool_size)
            .build()
            .context("failed to build connection pool")?
    };
    Ok(pool)
}

impl PgId {
    async fn client(&self) -> Result<deadpool_postgres::Client, String> {
        self.pool
            .get()
            .await
            .map_err(|e| format!("failed to get connection: {e}"))
    }

    /// Run a parameterized query on this named import's connection.
    pub async fn query(&self, sql: &str, params: &[PgValue]) -> Result<Vec<ResultRow>, QueryError> {
        let client = self.client().await.map_err(QueryError::Unexpected)?;
        execute_query(&client, sql, params).await
    }

    /// Execute a statement on this named import's connection, returning the
    /// number of rows affected. Convenience for callers (and tests) that only
    /// need success/failure routed to the right per-credential connection.
    pub async fn execute(&self, sql: &str) -> Result<u64, QueryError> {
        let client = self.client().await.map_err(QueryError::Unexpected)?;
        client.execute(sql, &[]).await.map_err(|e| {
            QueryError::InvalidQuery(format!("statement failed: {}", pg_error_string(&e)))
        })
    }

    /// Run a multi-statement batch (no parameters, no row results).
    async fn query_batch(&self, sql: &str) -> Result<(), QueryError> {
        let client = self.client().await.map_err(QueryError::Unexpected)?;
        client.batch_execute(sql).await.map_err(|e| {
            QueryError::InvalidQuery(format!("batch execution failed: {}", pg_error_string(&e)))
        })
    }

    /// Prepare a statement on this connection and register it in the plugin's
    /// shared prepared-statement table, tagged with `component_id` so the
    /// workload's unbind reaps it. Returns the lookup token. The empty `database`
    /// marks the entry as implements-routed (exec routes by connection, not by
    /// database name).
    async fn prepare(
        &self,
        store: &RwLock<HashMap<String, PreparedEntry>>,
        component_id: &str,
        statement: String,
    ) -> Result<String, StatementPrepareError> {
        let client = self
            .client()
            .await
            .map_err(StatementPrepareError::Unexpected)?;
        let stmt = client.prepare(&statement).await.map_err(|e| {
            StatementPrepareError::Unexpected(format!("prepare failed: {}", pg_error_string(&e)))
        })?;
        let token = ulid::Ulid::new().to_string();
        store.write().await.insert(
            token.clone(),
            PreparedEntry {
                sql: statement,
                param_types: stmt.params().to_vec(),
                database: String::new(),
                component_id: component_id.to_string(),
            },
        );
        Ok(token)
    }

    /// Execute a previously prepared statement by token on this connection.
    async fn exec(
        &self,
        store: &RwLock<HashMap<String, PreparedEntry>>,
        token: &str,
        params: &[PgValue],
    ) -> Result<u64, PreparedStatementExecError> {
        let (sql, param_types) = {
            let store = store.read().await;
            match store.get(token) {
                Some(e) => (e.sql.clone(), e.param_types.clone()),
                None => return Err(PreparedStatementExecError::UnknownPreparedQuery),
            }
        };
        let client = self
            .client()
            .await
            .map_err(PreparedStatementExecError::Unexpected)?;
        let stmt = client
            .prepare_typed(&sql, &param_types)
            .await
            .map_err(|e| {
                PreparedStatementExecError::Unexpected(format!(
                    "re-prepare failed: {}",
                    pg_error_string(&e)
                ))
            })?;
        let param_refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = params
            .iter()
            .map(|p| p as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();
        client.execute_raw(&stmt, param_refs).await.map_err(|e| {
            PreparedStatementExecError::QueryError(QueryError::Unexpected(format!(
                "execute failed: {}",
                pg_error_string(&e)
            )))
        })
    }
}

/// The sole `wasmcloud:postgres` backend provider: builds a [`PgId`] connection
/// pool from a named interface's `url`. The multiplexer shares one pool across
/// binds with the same URL (its `pool_key`).
struct PostgresProvider;

#[async_trait::async_trait]
impl BackendProvider<PgId> for PostgresProvider {
    fn backend_type(&self) -> &'static str {
        POSTGRES_BACKEND
    }

    fn pool_key(&self, config: &HashMap<String, String>) -> Option<String> {
        config.get("url").cloned()
    }

    async fn instantiate(&self, config: &HashMap<String, String>) -> anyhow::Result<PgId> {
        let url = config.get("url").ok_or_else(|| {
            anyhow::anyhow!("named wasmcloud:postgres interface requires a 'url' config")
        })?;
        Ok(PgId {
            pool: build_pool_from_url(url)?,
        })
    }
}

impl WasmcloudPostgres {
    /// Create a postgres plugin with no shared bouncer URL, for deployments that
    /// route purely through `(implements ..)` named imports — each named
    /// interface carries its own full URL. The legacy per-component-database
    /// path is inert until an unnamed interface supplies a `database`, and with
    /// an empty base config such a bind would fail to connect; pure-multiplex
    /// workloads never take that path.
    pub fn multiplex_only() -> Self {
        Self::with_base(tokio_postgres::Config::new(), DEFAULT_POOL_SIZE, false)
    }

    /// Build a fresh [`Multiplexer`] for `wasmcloud:postgres` named imports,
    /// registering the single [`PostgresProvider`]. Exposed so tests can route
    /// named interfaces to per-credential pools without a full plugin instance.
    pub fn multiplexer() -> Multiplexer<PgId> {
        Multiplexer::new("wasmcloud", "postgres", POSTGRES_BACKEND)
            .with_provider(Arc::new(PostgresProvider))
    }

    /// Build the per-import connection registry (interface name -> [`PgId`]) for
    /// the named `wasmcloud:postgres` host interfaces, through the shared
    /// multiplexer so identical URLs reuse one pool.
    pub async fn build_named_pools<'i>(
        &self,
        interfaces: impl IntoIterator<Item = &'i WitInterface>,
    ) -> anyhow::Result<HashMap<String, PgId>> {
        // Only named interfaces are multiplexed; an unnamed one keeps the legacy
        // per-component-database path, so filter it out before building.
        let named: Vec<&WitInterface> = interfaces
            .into_iter()
            .filter(|i| i.name.is_some())
            .collect();
        self.mux.build_registry(named).await
    }
}

impl<'a> bindings::named_imports::wasmcloud::postgres::query::Host for ActiveCtx<'a> {
    async fn query(
        &mut self,
        id: PgId,
        q: String,
        params: Vec<PgValue>,
    ) -> wasmtime::Result<Result<Vec<ResultRow>, QueryError>> {
        Ok(id.query(&q, &params).await)
    }

    async fn query_batch(
        &mut self,
        id: PgId,
        q: String,
    ) -> wasmtime::Result<Result<(), QueryError>> {
        Ok(id.query_batch(&q).await)
    }
}

impl<'a> bindings::named_imports::wasmcloud::postgres::prepared::Host for ActiveCtx<'a> {
    async fn prepare(
        &mut self,
        id: PgId,
        statement: String,
    ) -> wasmtime::Result<Result<String, StatementPrepareError>> {
        let plugin = self.try_get_plugin::<WasmcloudPostgres>(PLUGIN_POSTGRES_ID)?;
        let component_id = self.component_id.to_string();
        Ok(id
            .prepare(&plugin.prepared_statements, &component_id, statement)
            .await)
    }

    async fn exec(
        &mut self,
        id: PgId,
        stmt_token: String,
        params: Vec<PgValue>,
    ) -> wasmtime::Result<Result<u64, PreparedStatementExecError>> {
        let plugin = self.try_get_plugin::<WasmcloudPostgres>(PLUGIN_POSTGRES_ID)?;
        Ok(id
            .exec(&plugin.prepared_statements, &stmt_token, &params)
            .await)
    }
}
