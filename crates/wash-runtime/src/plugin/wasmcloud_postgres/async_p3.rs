//! wasip3 async host binding for `wasmcloud:postgres@0.2.0`.
//!
//! This lives alongside the sync `0.1.1-draft` binding in the parent module; a
//! component may import either — as an unnamed *default* import, or (under the
//! `wasm_component_model_implements` feature) several times via `(implements ..)`
//! with each import routed to its own credentialed pool. Both surfaces share the
//! parent's connection pools, prepared-statement registry, and per-component
//! database mapping; only the WIT differs.
//!
//! The functions are bound with the `store` bindgen option so `query` can mint a
//! `stream<row>` and a completion `future` (via [`StreamReader`]/[`FutureReader`]).
//! Rows are streamed incrementally: a background task owns the connection and the
//! `tokio_postgres` row stream and feeds rows over a bounded channel (so a slow
//! guest exerts backpressure), while the completion future carries any error that
//! surfaces partway through. The value model (`pg-value`) is byte-identical to
//! the sync binding, so the whole `conversions` body is `include!`d and reused.

use core::pin::Pin;
use core::task::{Context, Poll};

use futures::TryStreamExt as _;
use tokio::sync::{mpsc, oneshot};
use tokio_postgres::types::{ToSql, Type as PgType};
use wasmtime::StoreContextMut;
use wasmtime::component::{
    Accessor, Destination, FutureReader, Linker, StreamProducer, StreamReader, StreamResult,
};

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};

use super::{PLUGIN_POSTGRES_ID, PreparedEntry, WasmcloudPostgres};

#[cfg(feature = "wasm_component_model_implements")]
use std::collections::HashMap;
#[cfg(feature = "wasm_component_model_implements")]
use wasmtime::component::Component;

#[cfg(feature = "wasm_component_model_implements")]
use super::PgId;
#[cfg(feature = "wasm_component_model_implements")]
use crate::plugin::multiplex::Multiplexer;

/// How many fetched rows may sit buffered between the fetch task and the guest
/// before the task blocks — bounds host memory and applies backpressure.
const ROW_CHANNEL_CAPACITY: usize = 16;

#[cfg(not(feature = "wasm_component_model_implements"))]
mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "async-postgres",
        imports: { default: store | async | trappable | tracing },
    });
}

#[cfg(feature = "wasm_component_model_implements")]
mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "async-postgres",
        imports: { default: store | async | trappable | tracing },
        // Allow a component to import `wasmcloud:postgres@0.2.0` multiple times
        // via `(implements ..)`, routing each named import to its own
        // credentialed connection pool (the embedder-chosen `id`).
        named_imports: {
            "wasmcloud:postgres/query@0.2.0": crate::plugin::wasmcloud_postgres::PgId,
            "wasmcloud:postgres/prepared@0.2.0": crate::plugin::wasmcloud_postgres::PgId,
        },
    });
}

use bindings::wasmcloud::postgres0_2_0::types::{DbError, Error, PgValue, Row};
use bindings::wasmcloud::postgres0_2_0::{prepared, query, types};

/// The completion `future<result<_, error>>` end returned alongside a query's
/// row stream.
type ResultFuture = FutureReader<Result<(), Error>>;

/// `PgValue` <-> tokio-postgres conversions for the async binding.
///
/// The conversion body is shared verbatim with the sync binding via
/// [`conversions.rs`](./conversions.rs) (see that file's header); only the
/// row-shaping is specific to `0.2.0` (a bare `row = list<pg-value>` rather than
/// the sync `result-row` of named entries).
mod conversions {
    use super::bindings::wasmcloud::postgres0_2_0::types::{
        Date, HashableF64, MacAddressEui48, MacAddressEui64, Numeric, Offset, PgValue, Time,
        Timestamp, TimestampTz,
    };

    include!("conversions.rs");

    /// Build a `row` (bare list of values, positionally aligned with the query's
    /// column list) from a tokio-postgres [`Row`].
    ///
    /// Uses `try_get` rather than `get` so a column whose type the `FromSql` impl
    /// cannot decode surfaces as `error::value-conversion-failed` (routed to the
    /// query's completion future) instead of panicking the fetch task.
    pub(super) fn row_to_values(r: &Row) -> Result<Vec<PgValue>, super::Error> {
        (0..r.len())
            .map(|idx| {
                r.try_get(idx)
                    .map_err(|e| super::Error::ValueConversionFailed(format!("column {idx}: {e}")))
            })
            .collect()
    }
}

/// Map a `tokio_postgres::Error` to the WIT `error`.
///
/// Failures the database reported (with a SQLSTATE) become `query-failed` and
/// carry the structured [`DbError`] so a guest can branch on `code`; everything
/// else (connection loss, protocol errors) is reported as `connection-failed`.
fn to_wit_error(e: &tokio_postgres::Error) -> Error {
    match e.as_db_error() {
        Some(db) => Error::QueryFailed(DbError {
            code: db.code().code().to_string(),
            severity: db.severity().to_string(),
            message: db.message().to_string(),
            detail: db.detail().map(ToString::to_string),
            extras: Vec::new(),
        }),
        None => Error::ConnectionFailed(e.to_string()),
    }
}

/// Acquire a pooled connection for `database`, mapping pool/connection failures
/// to the WIT `error::connection-failed`.
async fn client_for(
    plugin: &WasmcloudPostgres,
    database: &str,
) -> Result<deadpool_postgres::Client, Error> {
    let pool = plugin
        .get_or_create_pool(database)
        .await
        .map_err(|e| Error::ConnectionFailed(format!("failed to get connection pool: {e}")))?;
    pool.get()
        .await
        .map_err(|e| Error::ConnectionFailed(format!("failed to get connection: {e}")))
}

/// Run `q` on `client` and start streaming its rows.
///
/// The statement is prepared first so the column list is known up front (an
/// empty result set still reports its columns, and `tokio_postgres`'s row stream
/// exposes no column metadata of its own). A background task then owns the
/// connection and the row stream, forwarding converted rows over a bounded
/// channel and resolving the completion future when the stream ends — or with
/// the error, if one surfaces mid-stream.
async fn stream_query<U>(
    store: &Accessor<U, SharedCtx>,
    client: deadpool_postgres::Client,
    q: String,
    params: Vec<PgValue>,
) -> wasmtime::Result<Result<(Vec<String>, StreamReader<Row>, ResultFuture), Error>> {
    let stmt = match client.prepare(&q).await {
        Ok(s) => s,
        Err(e) => return Ok(Err(to_wit_error(&e))),
    };
    let columns = stmt
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect::<Vec<String>>();

    let (row_tx, row_rx) = mpsc::channel::<Row>(ROW_CHANNEL_CAPACITY);
    let (done_tx, done_rx) = oneshot::channel::<Result<(), Error>>();
    tokio::spawn(async move {
        let result = drain_query(client, stmt, params, row_tx).await;
        let _ = done_tx.send(result);
    });

    store.with(|mut access| {
        let stream = StreamReader::new(&mut access, RowStreamProducer { rows: row_rx })?;
        let future = ResultFuture::new(&mut access, done_rx)?;
        wasmtime::Result::Ok(Ok((columns, stream, future)))
    })
}

/// Body of the row-fetch task: execute the prepared statement and forward each
/// converted row to `row_tx`, stopping early if the guest drops the stream.
async fn drain_query(
    client: deadpool_postgres::Client,
    stmt: tokio_postgres::Statement,
    params: Vec<PgValue>,
    row_tx: mpsc::Sender<Row>,
) -> Result<(), Error> {
    let param_refs = params
        .iter()
        .map(|p| p as &(dyn ToSql + Sync))
        .collect::<Vec<_>>();
    let stream = client
        .query_raw(&stmt, param_refs)
        .await
        .map_err(|e| to_wit_error(&e))?;
    let mut stream = std::pin::pin!(stream);
    loop {
        match stream.try_next().await {
            Ok(Some(row)) => {
                let values = conversions::row_to_values(&row)?;
                if row_tx.send(values).await.is_err() {
                    // Guest dropped the stream; stop fetching.
                    return Ok(());
                }
            }
            Ok(None) => return Ok(()),
            Err(e) => return Err(to_wit_error(&e)),
        }
    }
}

/// Run a multi-statement batch (no parameters, no row results) on `client`.
async fn batch_with_client(client: deadpool_postgres::Client, q: String) -> Result<(), Error> {
    client.batch_execute(&q).await.map_err(|e| to_wit_error(&e))
}

/// Prepare `statement` on `client` and register it in the plugin's shared table,
/// tagged with `component_id` so workload unbind reaps it. `database` is empty
/// for implements-routed entries (exec routes by connection, not database name).
async fn prepare_with_client(
    client: &deadpool_postgres::Client,
    plugin: &WasmcloudPostgres,
    component_id: String,
    database: String,
    statement: String,
) -> Result<String, Error> {
    let stmt = client
        .prepare(&statement)
        .await
        .map_err(|e| to_wit_error(&e))?;
    let token = ulid::Ulid::new().to_string();
    plugin.prepared_statements.write().await.insert(
        token.clone(),
        PreparedEntry {
            sql: statement,
            param_types: stmt.params().to_vec(),
            database,
            component_id,
        },
    );
    Ok(token)
}

/// Execute a previously prepared statement on `client`, returning rows affected.
async fn exec_with_client(
    client: &deadpool_postgres::Client,
    sql: &str,
    param_types: &[PgType],
    params: &[PgValue],
) -> Result<u64, Error> {
    // Re-prepare via the connection's statement cache (deadpool caches these).
    let stmt = client
        .prepare_typed(sql, param_types)
        .await
        .map_err(|e| to_wit_error(&e))?;
    let param_refs = params
        .iter()
        .map(|p| p as &(dyn ToSql + Sync))
        .collect::<Vec<_>>();
    client
        .execute_raw(&stmt, param_refs)
        .await
        .map_err(|e| to_wit_error(&e))
}

/// Look up a prepared-statement entry by token, cloning the fields exec needs.
async fn lookup_prepared(
    plugin: &WasmcloudPostgres,
    token: &str,
) -> Option<(String, Vec<PgType>, String)> {
    let stmts = plugin.prepared_statements.read().await;
    stmts
        .get(token)
        .map(|e| (e.sql.clone(), e.param_types.clone(), e.database.clone()))
}

/// [`StreamProducer`] that forwards rows from the fetch task's channel to the
/// guest, one per poll. When the channel closes (task finished) the stream ends;
/// the paired completion future — resolved by the task — carries the outcome.
struct RowStreamProducer {
    rows: mpsc::Receiver<Row>,
}

impl<D: 'static> StreamProducer<D> for RowStreamProducer {
    type Item = Row;
    type Buffer = Option<Row>;

    fn poll_produce<'a>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let this = self.get_mut();
        if finish {
            // Guest cancelled; dropping the receiver signals the fetch task.
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }
        if dst.remaining(&mut store) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        match this.rows.poll_recv(cx) {
            Poll::Ready(Some(row)) => {
                dst.set_buffer(Some(row));
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Poll::Ready(None) => Poll::Ready(Ok(StreamResult::Dropped)),
            Poll::Pending => Poll::Pending,
        }
    }
}

// The `store` bindgen option routes the functions onto the `...WithStore` traits
// (implemented on the store-data type `SharedCtx`, with an `&Accessor` handle);
// the companion `Host` traits, implemented on the `ActiveCtx` view, stay empty.

impl types::Host for ActiveCtx<'_> {}
impl query::Host for ActiveCtx<'_> {}
impl prepared::Host for ActiveCtx<'_> {}

/// Resolve `(plugin, component-id)` for the active call. The plugin is
/// `Arc`-cloned so it outlives the synchronous store borrow.
fn plugin_and_component<U>(
    store: &Accessor<U, SharedCtx>,
) -> wasmtime::Result<(std::sync::Arc<WasmcloudPostgres>, String)> {
    store.with(|mut access| {
        let view = access.get();
        let plugin = view.try_get_plugin::<WasmcloudPostgres>(PLUGIN_POSTGRES_ID)?;
        let component_id = view.component_id.to_string();
        wasmtime::Result::Ok((plugin, component_id))
    })
}

impl<U> query::HostWithStore<U> for SharedCtx {
    async fn query(
        store: &Accessor<U, Self>,
        q: String,
        params: Vec<PgValue>,
    ) -> wasmtime::Result<Result<(Vec<String>, StreamReader<Row>, ResultFuture), Error>> {
        let (plugin, component_id) = plugin_and_component(store)?;
        let Some(database) = plugin.database_for_component(&component_id).await else {
            return Ok(Err(Error::Other(
                "no database configured for this component".to_string(),
            )));
        };
        let client = match client_for(&plugin, &database).await {
            Ok(c) => c,
            Err(e) => return Ok(Err(e)),
        };
        stream_query(store, client, q, params).await
    }

    async fn query_batch(
        store: &Accessor<U, Self>,
        q: String,
    ) -> wasmtime::Result<Result<(), Error>> {
        let (plugin, component_id) = plugin_and_component(store)?;
        let Some(database) = plugin.database_for_component(&component_id).await else {
            return Ok(Err(Error::Other(
                "no database configured for this component".to_string(),
            )));
        };
        let client = match client_for(&plugin, &database).await {
            Ok(c) => c,
            Err(e) => return Ok(Err(e)),
        };
        Ok(batch_with_client(client, q).await)
    }
}

impl<U> prepared::HostWithStore<U> for SharedCtx {
    async fn prepare(
        store: &Accessor<U, Self>,
        statement: String,
    ) -> wasmtime::Result<Result<String, Error>> {
        let (plugin, component_id) = plugin_and_component(store)?;
        let Some(database) = plugin.database_for_component(&component_id).await else {
            return Ok(Err(Error::Other(
                "no database configured for this component".to_string(),
            )));
        };
        let client = match client_for(&plugin, &database).await {
            Ok(c) => c,
            Err(e) => return Ok(Err(e)),
        };
        Ok(prepare_with_client(&client, &plugin, component_id, database, statement).await)
    }

    async fn exec(
        store: &Accessor<U, Self>,
        stmt_token: String,
        params: Vec<PgValue>,
    ) -> wasmtime::Result<Result<u64, Error>> {
        let (plugin, _component_id) = plugin_and_component(store)?;
        let Some((sql, param_types, database)) = lookup_prepared(&plugin, &stmt_token).await else {
            return Ok(Err(Error::UnknownPreparedStatement));
        };
        let client = match client_for(&plugin, &database).await {
            Ok(c) => c,
            Err(e) => return Ok(Err(e)),
        };
        Ok(exec_with_client(&client, &sql, &param_types, &params).await)
    }
}

// Same shape as the default impls, but the connection comes from the resolved
// per-credential `PgId` pool rather than the component's configured database.

#[cfg(feature = "wasm_component_model_implements")]
impl bindings::named_imports::wasmcloud::postgres0_2_0::query::Host for ActiveCtx<'_> {}
#[cfg(feature = "wasm_component_model_implements")]
impl bindings::named_imports::wasmcloud::postgres0_2_0::prepared::Host for ActiveCtx<'_> {}

#[cfg(feature = "wasm_component_model_implements")]
impl<U> bindings::named_imports::wasmcloud::postgres0_2_0::query::HostWithStore<U> for SharedCtx {
    async fn query(
        store: &Accessor<U, Self>,
        id: PgId,
        q: String,
        params: Vec<PgValue>,
    ) -> wasmtime::Result<Result<(Vec<String>, StreamReader<Row>, ResultFuture), Error>> {
        let client = match id.client().await {
            Ok(c) => c,
            Err(e) => return Ok(Err(Error::ConnectionFailed(e))),
        };
        stream_query(store, client, q, params).await
    }

    async fn query_batch(
        _store: &Accessor<U, Self>,
        id: PgId,
        q: String,
    ) -> wasmtime::Result<Result<(), Error>> {
        let client = match id.client().await {
            Ok(c) => c,
            Err(e) => return Ok(Err(Error::ConnectionFailed(e))),
        };
        Ok(batch_with_client(client, q).await)
    }
}

#[cfg(feature = "wasm_component_model_implements")]
impl<U> bindings::named_imports::wasmcloud::postgres0_2_0::prepared::HostWithStore<U>
    for SharedCtx
{
    async fn prepare(
        store: &Accessor<U, Self>,
        id: PgId,
        statement: String,
    ) -> wasmtime::Result<Result<String, Error>> {
        let (plugin, component_id) = plugin_and_component(store)?;
        let client = match id.client().await {
            Ok(c) => c,
            Err(e) => return Ok(Err(Error::ConnectionFailed(e))),
        };
        // Implements-routed entries carry an empty database: exec re-acquires the
        // connection from the same `PgId`, not by database name.
        Ok(prepare_with_client(&client, &plugin, component_id, String::new(), statement).await)
    }

    async fn exec(
        store: &Accessor<U, Self>,
        id: PgId,
        stmt_token: String,
        params: Vec<PgValue>,
    ) -> wasmtime::Result<Result<u64, Error>> {
        let (plugin, _component_id) = plugin_and_component(store)?;
        let Some((sql, param_types, _database)) = lookup_prepared(&plugin, &stmt_token).await
        else {
            return Ok(Err(Error::UnknownPreparedStatement));
        };
        let client = match id.client().await {
            Ok(c) => c,
            Err(e) => return Ok(Err(Error::ConnectionFailed(e))),
        };
        Ok(exec_with_client(&client, &sql, &param_types, &params).await)
    }
}

/// Link the `types` instance. It carries no functions but its import must be
/// satisfied; link it exactly once regardless of default/named routing.
pub(super) fn add_types_to_linker(linker: &mut Linker<SharedCtx>) -> wasmtime::Result<()> {
    types::add_to_linker::<_, SharedCtx>(linker, extract_active_ctx)
}

/// Link the default (unnamed) `query`/`prepared` interfaces.
pub(super) fn add_default_to_linker(linker: &mut Linker<SharedCtx>) -> wasmtime::Result<()> {
    query::add_to_linker::<_, SharedCtx>(linker, extract_active_ctx)?;
    prepared::add_to_linker::<_, SharedCtx>(linker, extract_active_ctx)?;
    Ok(())
}

/// Link the named (`implements`) `query`/`prepared` interfaces, routing each
/// import name to its per-credential [`PgId`] via the multiplexer.
#[cfg(feature = "wasm_component_model_implements")]
pub(super) fn add_named_to_linker(
    linker: &mut Linker<SharedCtx>,
    component: &Component,
    registry: &HashMap<String, PgId>,
    mux: &Multiplexer<PgId>,
) -> wasmtime::Result<()> {
    bindings::named_imports::wasmcloud::postgres0_2_0::query::add_to_linker::<_, SharedCtx>(
        linker,
        component,
        |name| mux.resolve(registry, name),
        extract_active_ctx,
    )?;
    bindings::named_imports::wasmcloud::postgres0_2_0::prepared::add_to_linker::<_, SharedCtx>(
        linker,
        component,
        |name| mux.resolve(registry, name),
        extract_active_ctx,
    )?;
    Ok(())
}
