mod async_p3;

/// Sync (`0.1.1-draft`) `PgValue` <-> tokio-postgres conversions.
///
/// The conversion body lives in `conversions.rs` and is shared verbatim with the
/// async (`0.2.0`) binding (see that file's header). `include!` pastes it here
/// with this binding's generated types in scope; only `into_result_row` (the
/// sync `result-row` of named entries) is specific to this binding.
mod conversions {
    use super::bindings::wasmcloud::postgres0_1_1_draft::query::{PgValue, ResultRow};
    use super::bindings::wasmcloud::postgres0_1_1_draft::types::{
        Date, HashableF64, MacAddressEui48, MacAddressEui64, Numeric, Offset, ResultRowEntry, Time,
        Timestamp, TimestampTz,
    };

    include!("conversions.rs");

    /// Build a `result-row` (a list of named `result-row-entry`) from a [`Row`].
    pub(super) fn into_result_row(r: Row) -> ResultRow {
        let mut rr = Vec::new();
        for (idx, col) in r.columns().iter().enumerate() {
            rr.push(ResultRowEntry {
                column_name: col.name().into(),
                value: r.get(idx),
            });
        }
        rr
    }
}

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context as _, bail};
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod};
use futures::TryStreamExt;
use tokio::sync::RwLock;
use tokio_postgres::types::Type as PgType;
use tracing::instrument;
use url::Url;

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
#[cfg(feature = "wasm_component_model_implements")]
use crate::plugin::multiplex::Multiplexer;
use crate::plugin::{HostPlugin, WitInterfaces};
use crate::wit::{WitInterface, WitWorld};

use conversions::into_result_row;

/// `(implements ..)` named per-credential routing: [`PgId`], its provider, the
/// named-import host impls, and the multiplex constructors. The shared query
/// helpers (`execute_query`, `pg_error_string`) and the legacy per-component
/// path stay here in the parent module.
#[cfg(feature = "wasm_component_model_implements")]
mod multiplexed;
#[cfg(feature = "wasm_component_model_implements")]
pub use multiplexed::PgId;

const PLUGIN_POSTGRES_ID: &str = "wasmcloud-postgres";
const DEFAULT_POOL_SIZE: usize = 10;

// Two variants of the same `postgres` world. They generate identical
// `bindings::wasmcloud::postgres0_1_1_draft::{types,query,prepared}` modules (so the legacy
// per-component path and `conversions` are unaffected); the implements variant
// additionally generates `bindings::named_imports::*` for `(implements ..)`
// routing. `types` carries no functions, so it is linked once and left unnamed.
#[cfg(not(feature = "wasm_component_model_implements"))]
mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "postgres",
        imports: { default: async | trappable | tracing },
    });
}

#[cfg(feature = "wasm_component_model_implements")]
mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "postgres",
        imports: { default: async | trappable | tracing },
        // Allow a component to import `wasmcloud:postgres` multiple times via
        // `(implements ..)`, routing each named import to its own credentialed
        // connection pool (the embedder-chosen `id`).
        named_imports: {
            "wasmcloud:postgres/query": super::PgId,
            "wasmcloud:postgres/prepared": super::PgId,
        },
    });
}

use bindings::wasmcloud::postgres0_1_1_draft::prepared;
use bindings::wasmcloud::postgres0_1_1_draft::query;
use bindings::wasmcloud::postgres0_1_1_draft::types;

// `pub` because these appear in `PgId`'s public query/exec API.
pub use query::{PgValue, QueryError, ResultRow};

use prepared::{PreparedStatementExecError, StatementPrepareError};

/// A prepared statement entry stored by the plugin.
/// Contains the original SQL, the inferred parameter types, and the database name.
struct PreparedEntry {
    sql: String,
    param_types: Vec<PgType>,
    database: String,
    component_id: String,
}

/// wasmcloud:postgres host plugin.
///
/// Manages postgres connection pools at the host level and routes workload
/// queries to the appropriate database via a connection bouncer pattern.
#[derive(Clone)]
pub struct WasmcloudPostgres {
    /// Base config parsed from URL (no dbname set)
    base_config: tokio_postgres::Config,
    /// Max pool size per database
    pool_size: usize,
    /// Whether TLS should be used for connections
    tls: bool,
    /// database_name -> Pool
    pools: Arc<RwLock<HashMap<String, Pool>>>,
    /// prepared_statement_token -> PreparedEntry
    prepared_statements: Arc<RwLock<HashMap<String, PreparedEntry>>>,
    /// component_id -> database_name
    component_databases: Arc<RwLock<HashMap<String, String>>>,
    /// Notifies the pool reaper that an unbind happened
    pool_reaper_notify: Arc<tokio::sync::Notify>,
    /// Multiplexing core for `(implements ..)` named imports: builds and shares
    /// a per-credential [`PgId`] connection pool per named host interface, keyed
    /// by URL so identical interfaces reuse one pool across workload binds.
    #[cfg(feature = "wasm_component_model_implements")]
    mux: Arc<Multiplexer<PgId>>,
}

impl WasmcloudPostgres {
    /// Create a new WasmcloudPostgres plugin from a postgres URL.
    ///
    /// The URL should contain credentials, host/port, sslmode, and optionally pool_size.
    /// The database name should NOT be included - workloads provide it via config.
    ///
    /// Example: `postgres://user:pass@bouncer:6432?sslmode=require&pool_size=10`
    pub fn new(url: &str) -> anyhow::Result<Self> {
        let parsed = Url::parse(url).context("failed to parse postgres URL")?;
        let mut config: tokio_postgres::Config =
            url.parse().context("failed to parse postgres config")?;

        // Extract pool_size from the URL (not a standard postgres param, we parse it ourselves)
        let pool_size = extract_pool_size(&parsed);

        // Determine TLS from sslmode
        let tls = extract_tls_requirement(&parsed);

        // Strip dbname from the base config - workloads set this via their config
        config.dbname("");

        Ok(Self::with_base(config, pool_size, tls))
    }

    /// Assemble a plugin around an already-parsed base config, fresh shared state
    /// and (under the implements feature) the named-import multiplexer.
    fn with_base(base_config: tokio_postgres::Config, pool_size: usize, tls: bool) -> Self {
        Self {
            base_config,
            pool_size,
            tls,
            pools: Arc::new(RwLock::new(HashMap::new())),
            prepared_statements: Arc::new(RwLock::new(HashMap::new())),
            component_databases: Arc::new(RwLock::new(HashMap::new())),
            pool_reaper_notify: Arc::new(tokio::sync::Notify::new()),
            #[cfg(feature = "wasm_component_model_implements")]
            mux: Arc::new(Self::multiplexer()),
        }
    }

    /// Get or lazily create a connection pool for the given database name.
    async fn get_or_create_pool(&self, database: &str) -> anyhow::Result<Pool> {
        // Fast path: pool already exists
        {
            let pools = self.pools.read().await;
            if let Some(pool) = pools.get(database) {
                return Ok(pool.clone());
            }
        }

        // Slow path: create pool
        let mut pools = self.pools.write().await;
        // Double-check after acquiring write lock
        if let Some(pool) = pools.get(database) {
            return Ok(pool.clone());
        }

        let mut pg_config = self.base_config.clone();
        pg_config.dbname(database);

        let mgr_config = ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        };

        let pool = if self.tls {
            let tls_config = rustls::ClientConfig::builder()
                .with_root_certificates(rustls::RootCertStore {
                    roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
                })
                .with_no_client_auth();
            let tls = tokio_postgres_rustls::MakeRustlsConnect::new(tls_config);
            let mgr = Manager::from_config(pg_config, tls, mgr_config);
            Pool::builder(mgr)
                .max_size(self.pool_size)
                .build()
                .context("failed to build TLS connection pool")?
        } else {
            let mgr = Manager::from_config(pg_config, tokio_postgres::NoTls, mgr_config);
            Pool::builder(mgr)
                .max_size(self.pool_size)
                .build()
                .context("failed to build connection pool")?
        };

        pools.insert(database.to_string(), pool.clone());
        Ok(pool)
    }

    /// Look up the database name for a component.
    async fn database_for_component(&self, component_id: &str) -> Option<String> {
        self.component_databases
            .read()
            .await
            .get(component_id)
            .cloned()
    }
}

/// Extract pool_size from URL query params. Returns default if not found.
fn extract_pool_size(url: &Url) -> usize {
    url.query_pairs()
        .find(|(k, _)| k == "pool_size")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(DEFAULT_POOL_SIZE)
}

/// Determine whether TLS is required based on the sslmode URL parameter.
fn extract_tls_requirement(url: &Url) -> bool {
    url.query_pairs()
        .find(|(k, _)| k == "sslmode")
        .map(|(_, v)| matches!(v.as_ref(), "require" | "verify-ca" | "verify-full"))
        .unwrap_or(false)
}

// ── Host trait implementations ──────────────────────────────────────────────

impl<'a> types::Host for ActiveCtx<'a> {}

impl<'a> query::Host for ActiveCtx<'a> {
    #[instrument(name = "wasmcloud.postgres.query", skip_all, fields(query = %q))]
    async fn query(
        &mut self,
        q: String,
        params: Vec<PgValue>,
    ) -> wasmtime::Result<Result<Vec<query::ResultRow>, QueryError>> {
        let plugin = self.try_get_plugin::<WasmcloudPostgres>(PLUGIN_POSTGRES_ID)?;

        let component_id = self.component_id.to_string();
        let database = match plugin.database_for_component(&component_id).await {
            Some(db) => db,
            None => {
                return Ok(Err(QueryError::Unexpected(
                    "no database configured for this component".to_string(),
                )));
            }
        };

        let pool = match plugin.get_or_create_pool(&database).await {
            Ok(p) => p,
            Err(e) => {
                return Ok(Err(QueryError::Unexpected(format!(
                    "failed to get connection pool: {e}"
                ))));
            }
        };

        let client = match pool.get().await {
            Ok(c) => c,
            Err(e) => {
                return Ok(Err(QueryError::Unexpected(format!(
                    "failed to get connection: {e}"
                ))));
            }
        };

        Ok(execute_query(&client, &q, &params).await)
    }

    #[instrument(name = "wasmcloud.postgres.query_batch", skip_all, fields(query = %q))]
    async fn query_batch(&mut self, q: String) -> wasmtime::Result<Result<(), QueryError>> {
        let plugin = self.try_get_plugin::<WasmcloudPostgres>(PLUGIN_POSTGRES_ID)?;

        let component_id = self.component_id.to_string();
        let database = match plugin.database_for_component(&component_id).await {
            Some(db) => db,
            None => {
                return Ok(Err(QueryError::Unexpected(
                    "no database configured for this component".to_string(),
                )));
            }
        };

        let pool = match plugin.get_or_create_pool(&database).await {
            Ok(p) => p,
            Err(e) => {
                return Ok(Err(QueryError::Unexpected(format!(
                    "failed to get connection pool: {e}"
                ))));
            }
        };

        let client = match pool.get().await {
            Ok(c) => c,
            Err(e) => {
                return Ok(Err(QueryError::Unexpected(format!(
                    "failed to get connection: {e}"
                ))));
            }
        };

        match client.batch_execute(&q).await {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(QueryError::InvalidQuery(format!(
                "batch execution failed: {}",
                pg_error_string(&e)
            )))),
        }
    }
}

impl<'a> prepared::Host for ActiveCtx<'a> {
    #[instrument(name = "wasmcloud.postgres.prepare", skip_all)]
    async fn prepare(
        &mut self,
        statement: String,
    ) -> wasmtime::Result<Result<String, StatementPrepareError>> {
        let plugin = self.try_get_plugin::<WasmcloudPostgres>(PLUGIN_POSTGRES_ID)?;

        let component_id = self.component_id.to_string();
        let database = match plugin.database_for_component(&component_id).await {
            Some(db) => db,
            None => {
                return Ok(Err(StatementPrepareError::Unexpected(
                    "no database configured for this component".to_string(),
                )));
            }
        };

        let pool = match plugin.get_or_create_pool(&database).await {
            Ok(p) => p,
            Err(e) => {
                return Ok(Err(StatementPrepareError::Unexpected(format!(
                    "failed to get connection pool: {e}"
                ))));
            }
        };

        let client = match pool.get().await {
            Ok(c) => c,
            Err(e) => {
                return Ok(Err(StatementPrepareError::Unexpected(format!(
                    "failed to get connection: {e}"
                ))));
            }
        };

        let stmt = match client.prepare(&statement).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(Err(StatementPrepareError::Unexpected(format!(
                    "prepare failed: {}",
                    pg_error_string(&e)
                ))));
            }
        };

        let token = ulid::Ulid::new().to_string();
        let param_types = stmt.params().to_vec();

        plugin.prepared_statements.write().await.insert(
            token.clone(),
            PreparedEntry {
                sql: statement,
                param_types,
                database,
                component_id,
            },
        );

        Ok(Ok(token))
    }

    #[instrument(name = "wasmcloud.postgres.exec", skip_all, fields(stmt_token = %stmt_token))]
    async fn exec(
        &mut self,
        stmt_token: String,
        params: Vec<PgValue>,
    ) -> wasmtime::Result<Result<u64, PreparedStatementExecError>> {
        let plugin = self.try_get_plugin::<WasmcloudPostgres>(PLUGIN_POSTGRES_ID)?;

        let entry = {
            let stmts = plugin.prepared_statements.read().await;
            match stmts.get(&stmt_token) {
                Some(entry) => PreparedEntry {
                    sql: entry.sql.clone(),
                    param_types: entry.param_types.clone(),
                    database: entry.database.clone(),
                    component_id: entry.component_id.clone(),
                },
                None => return Ok(Err(PreparedStatementExecError::UnknownPreparedQuery)),
            }
        };

        let pool = match plugin.get_or_create_pool(&entry.database).await {
            Ok(p) => p,
            Err(e) => {
                return Ok(Err(PreparedStatementExecError::Unexpected(format!(
                    "failed to get connection pool: {e}"
                ))));
            }
        };

        let client = match pool.get().await {
            Ok(c) => c,
            Err(e) => {
                return Ok(Err(PreparedStatementExecError::Unexpected(format!(
                    "failed to get connection: {e}"
                ))));
            }
        };

        // Re-prepare via statement cache (deadpool-postgres caches these per connection)
        let stmt = match client.prepare_typed(&entry.sql, &entry.param_types).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(Err(PreparedStatementExecError::Unexpected(format!(
                    "re-prepare failed: {}",
                    pg_error_string(&e)
                ))));
            }
        };

        let param_refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = params
            .iter()
            .map(|p| p as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();

        match client.execute_raw(&stmt, param_refs).await {
            Ok(n) => Ok(Ok(n)),
            Err(e) => Ok(Err(PreparedStatementExecError::QueryError(
                QueryError::Unexpected(format!("execute failed: {}", pg_error_string(&e))),
            ))),
        }
    }
}

// ── shared query helpers (used by the legacy and multiplexed paths) ─────────

/// Format a postgres error, surfacing the server-side message (e.g. "permission
/// denied for table ...") since `tokio_postgres::Error`'s `Display` is only the
/// terse "db error". Per-credential routing leans on these messages to show RBAC
/// denials, so make them legible.
fn pg_error_string(e: &tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => format!("{}: {}", db.code().code(), db.message()),
        None => e.to_string(),
    }
}

/// Run a parameterized query on a connection and map rows to WIT result rows.
/// Shared by the legacy per-component path and the named per-credential path.
async fn execute_query(
    client: &deadpool_postgres::Client,
    q: &str,
    params: &[PgValue],
) -> Result<Vec<ResultRow>, QueryError> {
    let param_refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = params
        .iter()
        .map(|p| p as &(dyn tokio_postgres::types::ToSql + Sync))
        .collect();
    let stream = client
        .query_raw(q, param_refs)
        .await
        .map_err(|e| QueryError::InvalidQuery(format!("query failed: {}", pg_error_string(&e))))?;
    let rows = stream.try_collect::<Vec<_>>().await.map_err(|e| {
        QueryError::Unexpected(format!("failed to collect rows: {}", pg_error_string(&e)))
    })?;
    Ok(rows.into_iter().map(into_result_row).collect())
}

// ── HostPlugin implementation ───────────────────────────────────────────────

#[async_trait::async_trait]
impl HostPlugin for WasmcloudPostgres {
    fn id(&self) -> &'static str {
        PLUGIN_POSTGRES_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([
                // Sync (wasip2) and async (wasip3) surfaces are both served by
                // this plugin; a component may import either.
                WitInterface::from("wasmcloud:postgres/types,query,prepared@0.1.1-draft"),
                WitInterface::from("wasmcloud:postgres/types,query,prepared@0.2.0"),
            ]),
            ..Default::default()
        }
    }

    #[cfg(feature = "wasm_component_model_implements")]
    fn supports_named_instances(&self) -> bool {
        true
    }

    async fn start(&self) -> anyhow::Result<()> {
        let pools = Arc::clone(&self.pools);
        let component_databases = Arc::clone(&self.component_databases);
        let notify = Arc::clone(&self.pool_reaper_notify);

        tokio::spawn(async move {
            loop {
                notify.notified().await;
                // Grace period: let rapid restarts re-bind before we clean up
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;

                // Determine which databases are still in use
                let in_use: HashSet<String> =
                    component_databases.read().await.values().cloned().collect();

                // Remove pools for databases with no remaining components
                let mut pools_lock = pools.write().await;
                pools_lock.retain(|db, _| {
                    let keep = in_use.contains(db);
                    if !keep {
                        tracing::debug!(database = %db, "Removing idle connection pool");
                    }
                    keep
                });
            }
        });

        Ok(())
    }

    async fn on_workload_item_bind<'a>(
        &self,
        component_handle: &mut WorkloadItem<'a>,
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        let pg: Vec<&WitInterface> = interfaces
            .iter()
            .filter(|i| i.namespace == "wasmcloud" && i.package == "postgres")
            .collect();
        if pg.is_empty() {
            return Ok(());
        }

        // Split by package version: `0.1.1-draft` is the sync (wasip2) surface,
        // `0.2.0`+ the async (wasip3) one. Both are served by this plugin off the
        // same pools/prepared-statement registry.
        let async_min = semver::Version::new(0, 2, 0);
        let is_async = |i: &WitInterface| i.version.as_ref().is_some_and(|v| *v >= async_min);
        let pg_async: Vec<&WitInterface> = pg.iter().copied().filter(|i| is_async(i)).collect();
        let pg_sync: Vec<&WitInterface> = pg.iter().copied().filter(|i| !is_async(i)).collect();

        let component_id = component_handle.id().to_string();
        // Clone the component (cheap, Arc-backed) before taking the mutable
        // linker borrow — named-import linking needs both.
        #[cfg(feature = "wasm_component_model_implements")]
        let component = component_handle.component().clone();
        let linker = component_handle.linker();

        // ── sync `0.1.1-draft` (wasip2) ──────────────────────────────────────
        if !pg_sync.is_empty() {
            // A `(implements ..)` import is a *named* postgres interface routed
            // to its own credentialed connection; an unnamed one keeps the
            // per-component-database behavior (one shared bouncer URL, database
            // chosen by config).
            let unnamed = pg_sync.iter().find(|i| i.name.is_none()).copied();

            // `types` carries no functions and is shared by query/prepared; link
            // the instance once regardless of named/unnamed routing.
            bindings::wasmcloud::postgres0_1_1_draft::types::add_to_linker::<_, SharedCtx>(
                linker,
                extract_active_ctx,
            )?;

            if let Some(i) = unnamed {
                let Some(database) = i.config.get("database").cloned() else {
                    bail!("wasmcloud:postgres requires a 'database' config parameter")
                };
                tracing::debug!(
                    component_id = %component_id,
                    database = %database,
                    "Binding postgres plugin to component (per-component database)"
                );
                self.component_databases
                    .write()
                    .await
                    .insert(component_id.clone(), database);
                bindings::wasmcloud::postgres0_1_1_draft::query::add_to_linker::<_, SharedCtx>(
                    linker,
                    extract_active_ctx,
                )?;
                bindings::wasmcloud::postgres0_1_1_draft::prepared::add_to_linker::<_, SharedCtx>(
                    linker,
                    extract_active_ctx,
                )?;
            }

            #[cfg(feature = "wasm_component_model_implements")]
            if pg_sync.iter().any(|i| i.name.is_some()) {
                let registry = self.build_named_pools(pg_sync.iter().copied()).await?;
                tracing::debug!(
                    imports = ?registry.keys().collect::<Vec<_>>(),
                    "Binding postgres plugin to component (per-credential named imports)"
                );
                bindings::named_imports::wasmcloud::postgres0_1_1_draft::query::add_to_linker::<
                    _,
                    SharedCtx,
                >(
                    linker,
                    &component,
                    |name| self.mux.resolve(&registry, name),
                    extract_active_ctx,
                )?;
                bindings::named_imports::wasmcloud::postgres0_1_1_draft::prepared::add_to_linker::<
                    _,
                    SharedCtx,
                >(
                    linker,
                    &component,
                    |name| self.mux.resolve(&registry, name),
                    extract_active_ctx,
                )?;
            }
        }

        // ── async `0.2.0` (wasip3) ───────────────────────────────────────────
        if !pg_async.is_empty() {
            // `types` carries no functions and is shared by query/prepared; link
            // it once regardless of default/named routing.
            async_p3::add_types_to_linker(linker)?;

            // An unnamed import is the default, per-component-database path; a
            // `(implements ..)` import is a named one routed to its own pool.
            if let Some(i) = pg_async.iter().find(|i| i.name.is_none()).copied() {
                let Some(database) = i.config.get("database").cloned() else {
                    bail!("wasmcloud:postgres requires a 'database' config parameter")
                };
                tracing::debug!(
                    component_id = %component_id,
                    database = %database,
                    "Binding async postgres plugin to component (per-component database)"
                );
                self.component_databases
                    .write()
                    .await
                    .insert(component_id.clone(), database);
                async_p3::add_default_to_linker(linker)?;
            }

            #[cfg(feature = "wasm_component_model_implements")]
            if pg_async.iter().any(|i| i.name.is_some()) {
                let registry = self.build_named_pools(pg_async.iter().copied()).await?;
                tracing::debug!(
                    imports = ?registry.keys().collect::<Vec<_>>(),
                    "Binding async postgres plugin to component (per-credential named imports)"
                );
                async_p3::add_named_to_linker(linker, &component, &registry, &self.mux)?;
            }
        }

        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload_id: &str,
        _interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        tracing::debug!(workload_id = %workload_id, "Unbinding postgres plugin from workload");

        // Remove component → database mappings for this workload (legacy path).
        {
            let mut component_databases = self.component_databases.write().await;
            component_databases.retain(|component_id, _| !component_id.starts_with(workload_id));
        }

        // Clean up prepared statements created by this workload's components.
        // Both the legacy per-database path and implements-routed named imports
        // tag each entry with the creating `component_id`, so match on it
        // directly — a pure-multiplex workload never populates
        // `component_databases`, so deriving the set from there would miss its
        // prepared statements.
        {
            let mut prepared = self.prepared_statements.write().await;
            prepared.retain(|_, entry| !entry.component_id.starts_with(workload_id));
        }

        // Signal the pool reaper to check for idle pools
        self.pool_reaper_notify.notify_one();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(component_id: &str) -> PreparedEntry {
        PreparedEntry {
            sql: "SELECT 1".to_string(),
            param_types: Vec::new(),
            database: String::new(),
            component_id: component_id.to_string(),
        }
    }

    /// Unbinding a workload reaps prepared statements tagged with any of its
    /// component ids — covering both the legacy path and implements-routed named
    /// imports (which set an empty `database` but the same `component_id`), and
    /// leaving other workloads' statements untouched.
    #[tokio::test]
    async fn unbind_reaps_only_this_workloads_prepared_statements() {
        let pg = WasmcloudPostgres::new("postgres://u:p@localhost/").unwrap();
        {
            let mut prepared = pg.prepared_statements.write().await;
            prepared.insert("legacy".to_string(), entry("workload-a-component-0"));
            prepared.insert("named".to_string(), entry("workload-a-component-1"));
            prepared.insert("other".to_string(), entry("workload-b-component-0"));
        }

        let empty = HashSet::new();
        pg.on_workload_unbind("workload-a", WitInterfaces::new(&empty))
            .await
            .unwrap();

        let prepared = pg.prepared_statements.read().await;
        assert!(!prepared.contains_key("legacy"), "legacy entry reaped");
        assert!(!prepared.contains_key("named"), "named entry reaped");
        assert!(
            prepared.contains_key("other"),
            "another workload's entry must survive"
        );
    }
}
