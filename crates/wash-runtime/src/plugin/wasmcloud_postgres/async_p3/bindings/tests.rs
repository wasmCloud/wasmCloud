//! Exercise generated linking, canonical ABI conversions, and async dispatch.

use std::time::Duration;

use anyhow::{Context as _, Result, bail, ensure};
use futures::StreamExt as _;
use tokio::sync::{mpsc, oneshot};
use wasmtime::component::{
    Accessor, Component, FutureReader, HasData, Instance, Linker, StreamReader, Val,
};
use wasmtime::{Config, Engine, Store};

use super::wasmcloud::postgres0_2_0::{prepared, query, types};
use crate::engine::store::stream_pump;
use types::{DbError, Error, PgValue, Row};

type QueryResult = Result<
    (
        Vec<String>,
        StreamReader<Row>,
        FutureReader<Result<(), Error>>,
    ),
    Error,
>;
type Padding = (
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
    u32,
);
type QueryExport = (QueryResult, Padding);

const AFFECTED: u64 = u32::MAX as u64 + 17;
const SQL: &str = "SELECT $1 /* café 🦀 */";
const TOKEN: &str = "opaque\0token-🦀";

#[derive(Debug)]
struct Call {
    route: usize,
    operation: &'static str,
    text: String,
    params: Vec<PgValue>,
}

#[derive(Default)]
struct Host {
    calls: Vec<Call>,
    error: Option<Error>,
    trap: bool,
    columns: Option<Vec<String>>,
    rows: Option<mpsc::Receiver<Row>>,
    completion: Option<oneshot::Receiver<Result<(), Error>>>,
}

impl Host {
    async fn record<U>(
        store: &Accessor<U, Self>,
        route: usize,
        operation: &'static str,
        text: String,
        params: Vec<PgValue>,
    ) -> wasmtime::Result<Result<(), Error>> {
        // Every generated wrapper must release its store borrow across suspension.
        tokio::task::yield_now().await;
        store.with(|mut access| {
            let host = access.get();
            host.calls.push(Call {
                route,
                operation,
                text,
                params,
            });
            if host.trap {
                return Err(wasmtime::format_err!("postgres host trap"));
            }
            Ok(host.error.clone().map_or(Ok(()), Err))
        })
    }

    async fn query<U>(
        store: &Accessor<U, Self>,
        route: usize,
        q: String,
        params: Vec<PgValue>,
    ) -> wasmtime::Result<QueryResult> {
        if let Err(error) = Self::record(store, route, "query", q, params).await? {
            return Ok(Err(error));
        }
        store.with(|mut access| {
            let host = access.get();
            let columns = host
                .columns
                .take()
                .unwrap_or_else(|| vec!["first".into(), "第二".into()]);
            let rows = host
                .rows
                .take()
                .ok_or_else(|| wasmtime::format_err!("row producer not configured"))?;
            let completion = host
                .completion
                .take()
                .ok_or_else(|| wasmtime::format_err!("completion producer not configured"))?;
            Ok(Ok((
                columns,
                StreamReader::new(&mut access, super::super::RowStreamProducer { rows })?,
                FutureReader::new(&mut access, completion)?,
            )))
        })
    }
}

impl HasData for Host {
    type Data<'a> = &'a mut Host;
}

impl types::Host for Host {}
impl query::Host for Host {}
impl prepared::Host for Host {}

impl<U> query::HostWithStore<U> for Host {
    async fn query(
        store: &Accessor<U, Self>,
        q: String,
        params: Vec<PgValue>,
    ) -> wasmtime::Result<QueryResult> {
        Self::query(store, 0, q, params).await
    }

    async fn query_batch(
        store: &Accessor<U, Self>,
        q: String,
    ) -> wasmtime::Result<Result<(), Error>> {
        Self::record(store, 0, "query-batch", q, vec![]).await
    }
}

impl<U> prepared::HostWithStore<U> for Host {
    async fn prepare(
        store: &Accessor<U, Self>,
        statement: String,
    ) -> wasmtime::Result<Result<String, Error>> {
        Ok(Self::record(store, 0, "prepare", statement, vec![])
            .await?
            .map(|()| TOKEN.into()))
    }

    async fn exec(
        store: &Accessor<U, Self>,
        token: String,
        params: Vec<PgValue>,
    ) -> wasmtime::Result<Result<u64, Error>> {
        Ok(Self::record(store, 0, "exec", token, params)
            .await?
            .map(|()| AFFECTED))
    }
}

struct Harness {
    store: Store<Host>,
    instance: Instance,
}

fn engine() -> Result<Engine> {
    let mut config = Config::new();
    config.wasm_component_model_async(true);
    config.wasm_component_model_async_stackful(true);
    config.concurrency_support(true);
    #[cfg(feature = "wasm_component_model_implements")]
    config.wasm_component_model_implements(true);
    Ok(Engine::new(&config)?)
}

fn default_linker(engine: &Engine) -> Result<Linker<Host>> {
    let mut linker = Linker::new(engine);
    types::add_to_linker::<_, Host>(&mut linker, |host| host)?;
    query::add_to_linker::<_, Host>(&mut linker, |host| host)?;
    prepared::add_to_linker::<_, Host>(&mut linker, |host| host)?;
    Ok(linker)
}

impl Harness {
    async fn new() -> Result<Self> {
        let engine = engine()?;
        let component = Component::new(&engine, wat::parse_str(include_str!("contract.wat"))?)?;
        Self::instantiate(&engine, &component, default_linker(&engine)?).await
    }

    async fn instantiate(
        engine: &Engine,
        component: &Component,
        linker: Linker<Host>,
    ) -> Result<Self> {
        let mut store = Store::new(engine, Host::default());
        let instance = linker.instantiate_async(&mut store, component).await?;
        Ok(Self { store, instance })
    }

    async fn call(&mut self, operation: &str, params: Vec<Val>) -> Result<Val> {
        let func = self
            .instance
            .get_func(&mut self.store, operation)
            .context("export exists")?;
        Ok(tokio::time::timeout(
            Duration::from_secs(5),
            self.store.run_concurrent(async |accessor| {
                let mut results = [Val::Bool(false)];
                func.call_concurrent(accessor, &params, &mut results)
                    .await?;
                let [Val::Tuple(result)] = results else {
                    return Err(wasmtime::format_err!(
                        "adapter must return its result and padding"
                    ));
                };
                let [value, _padding]: [Val; 2] = result
                    .try_into()
                    .map_err(|_| wasmtime::format_err!("adapter must return two fields"))?;
                Ok::<_, wasmtime::Error>(value)
            }),
        )
        .await
        .with_context(|| format!("{operation} binding call timed out"))???)
    }
}

fn variant(name: &str, payload: Option<Val>) -> Val {
    Val::Variant(name.into(), payload.map(Box::new))
}

fn parameters() -> Vec<Val> {
    vec![
        variant("null", None),
        variant("big-int", Some(Val::S64(i64::MIN))),
        variant("pg-lsn", Some(Val::U64(u64::MAX))),
        variant("text", Some(Val::String("nul\0café 🦀".into()))),
        variant("bytea", Some(Val::List(vec![Val::U8(0), Val::U8(255)]))),
        variant(
            "int2-vector-array",
            Some(Val::List(vec![
                Val::List(vec![Val::S16(i16::MIN), Val::S16(i16::MAX)]),
                Val::List(vec![]),
            ])),
        ),
        variant(
            "hstore",
            Some(Val::List(vec![
                Val::Tuple(vec![Val::String("absent".into()), Val::Option(None)]),
                Val::Tuple(vec![
                    Val::String("present".into()),
                    Val::Option(Some(Box::new(Val::String("".into())))),
                ]),
            ])),
        ),
        variant(
            "ts-vector",
            Some(Val::List(vec![
                Val::Record(vec![
                    (
                        "position".into(),
                        Val::Option(Some(Box::new(Val::U16(16383)))),
                    ),
                    (
                        "weight".into(),
                        Val::Option(Some(Box::new(Val::Enum("C".into())))),
                    ),
                    ("data".into(), Val::String("検索".into())),
                ]),
                Val::Record(vec![
                    ("position".into(), Val::Option(None)),
                    ("weight".into(), Val::Option(None)),
                    ("data".into(), Val::String("".into())),
                ]),
            ])),
        ),
        variant(
            "double",
            Some(Val::Tuple(vec![
                Val::U64(u64::MAX),
                Val::S16(i16::MIN),
                Val::S8(i8::MIN),
            ])),
        ),
        variant(
            "date",
            Some(variant(
                "ymd",
                Some(Val::Tuple(vec![Val::S32(-4000), Val::U32(2), Val::U32(29)])),
            )),
        ),
    ]
}

fn assert_parameters(params: &[PgValue]) -> Result<()> {
    assert_eq!(params.len(), 10);
    assert!(matches!(params[0], PgValue::Null));
    assert!(matches!(params[1], PgValue::BigInt(i64::MIN)));
    assert!(matches!(params[2], PgValue::PgLsn(u64::MAX)));
    assert!(matches!(&params[3], PgValue::Text(s) if s == "nul\0café 🦀"));
    assert!(matches!(&params[4], PgValue::Bytea(b) if b == &[0, 255]));
    assert!(
        matches!(&params[5], PgValue::Int2VectorArray(v) if v == &[vec![i16::MIN, i16::MAX], vec![]])
    );
    assert!(
        matches!(&params[6], PgValue::Hstore(v) if v == &[("absent".into(), None), ("present".into(), Some("".into()))])
    );
    let PgValue::TsVector(lexemes) = &params[7] else {
        bail!("expected ts-vector")
    };
    assert_eq!(lexemes.len(), 2);
    assert_eq!(lexemes[0].position, Some(16383));
    assert!(matches!(lexemes[0].weight, Some(types::LexemeWeight::C)));
    assert_eq!(lexemes[0].data, "検索");
    assert_eq!(lexemes[1].position, None);
    assert!(lexemes[1].weight.is_none());
    assert_eq!(lexemes[1].data, "");
    assert!(matches!(
        params[8],
        PgValue::Double((u64::MAX, i16::MIN, i8::MIN))
    ));
    assert!(matches!(
        params[9],
        PgValue::Date(types::Date::Ymd((-4000, 2, 29)))
    ));
    Ok(())
}

#[tokio::test]
async fn default_operations_preserve_arguments_tokens_and_u64_results() -> Result<()> {
    let mut h = Harness::new().await?;
    assert_eq!(
        h.call("query-batch", vec![Val::String(SQL.into())]).await?,
        Val::Result(Ok(None))
    );
    assert_eq!(
        h.call("prepare", vec![Val::String(SQL.into())]).await?,
        Val::Result(Ok(Some(Box::new(Val::String(TOKEN.into())))))
    );
    assert_eq!(
        h.call(
            "exec",
            vec![Val::String(TOKEN.into()), Val::List(parameters())]
        )
        .await?,
        Val::Result(Ok(Some(Box::new(Val::U64(AFFECTED)))))
    );
    let calls = &h.store.data().calls;
    assert_eq!(calls.len(), 3);
    for (call, operation, text) in [
        (&calls[0], "query-batch", SQL),
        (&calls[1], "prepare", SQL),
        (&calls[2], "exec", TOKEN),
    ] {
        assert_eq!(call.route, 0);
        assert_eq!(call.operation, operation);
        assert_eq!(call.text, text);
    }
    assert_parameters(&calls[2].params)?;
    Ok(())
}

fn errors() -> Vec<(Error, Val)> {
    vec![
        (
            Error::ConnectionFailed("offline".into()),
            variant("connection-failed", Some(Val::String("offline".into()))),
        ),
        (
            Error::InvalidParams("$1".into()),
            variant("invalid-params", Some(Val::String("$1".into()))),
        ),
        (
            Error::InvalidQuery("syntax".into()),
            variant("invalid-query", Some(Val::String("syntax".into()))),
        ),
        (
            Error::QueryFailed(DbError {
                code: "23505".into(),
                severity: "ERROR".into(),
                message: "duplicate 🦀".into(),
                detail: Some("key=(1)".into()),
                extras: vec![("constraint".into(), "unique_id".into())],
            }),
            variant(
                "query-failed",
                Some(Val::Record(vec![
                    ("code".into(), Val::String("23505".into())),
                    ("severity".into(), Val::String("ERROR".into())),
                    ("message".into(), Val::String("duplicate 🦀".into())),
                    (
                        "detail".into(),
                        Val::Option(Some(Box::new(Val::String("key=(1)".into())))),
                    ),
                    (
                        "extras".into(),
                        Val::List(vec![Val::Tuple(vec![
                            Val::String("constraint".into()),
                            Val::String("unique_id".into()),
                        ])]),
                    ),
                ])),
            ),
        ),
        (
            Error::ValueConversionFailed("column 2".into()),
            variant(
                "value-conversion-failed",
                Some(Val::String("column 2".into())),
            ),
        ),
        (
            Error::UnknownPreparedStatement,
            variant("unknown-prepared-statement", None),
        ),
        (Error::AccessDenied, variant("access-denied", None)),
        (Error::Timeout, variant("timeout", None)),
        (
            Error::Other("other\0error".into()),
            variant("other", Some(Val::String("other\0error".into()))),
        ),
    ]
}

#[tokio::test]
async fn every_operation_preserves_each_wit_error_without_trapping() -> Result<()> {
    let mut h = Harness::new().await?;
    for (error, value) in errors() {
        h.store.data_mut().error = Some(error);
        for operation in ["query", "query-batch", "prepare", "exec"] {
            let mut params = vec![Val::String(SQL.into())];
            if matches!(operation, "query" | "exec") {
                params.push(Val::List(vec![]));
            }
            assert_eq!(
                h.call(operation, params).await?,
                Val::Result(Err(Some(Box::new(value.clone())))),
                "{operation}"
            );
        }
    }
    // A WIT error must leave the instance callable.
    h.store.data_mut().error = None;
    assert_eq!(
        h.call("query-batch", vec![Val::String(SQL.into())]).await?,
        Val::Result(Ok(None))
    );
    Ok(())
}

#[tokio::test]
async fn host_failures_trap_instead_of_becoming_wit_errors() -> Result<()> {
    for operation in ["query", "query-batch", "prepare", "exec"] {
        let mut h = Harness::new().await?;
        h.store.data_mut().trap = true;
        let mut params = vec![Val::String(SQL.into())];
        if matches!(operation, "query" | "exec") {
            params.push(Val::List(vec![]));
        }
        let error = h
            .call(operation, params)
            .await
            .err()
            .context("host failure must trap")?;
        assert!(
            format!("{error:#}").contains("postgres host trap"),
            "{operation}: {error:#}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn rows_and_delayed_completion_cross_the_binding_independently() -> Result<()> {
    for fail in [false, true] {
        let mut h = Harness::new().await?;
        let (row_tx, row_rx) = mpsc::channel(1);
        let (done_tx, done_rx) = oneshot::channel();
        h.store.data_mut().rows = Some(row_rx);
        h.store.data_mut().completion = Some(done_rx);
        let query = h
            .instance
            .get_typed_func::<(String, Vec<PgValue>), (QueryExport,)>(&mut h.store, "query")?;
        tokio::time::timeout(Duration::from_secs(5), h.store.run_concurrent(async |accessor| {
            let result = query.call_concurrent(accessor, (SQL.into(), vec![PgValue::Text("parameter".into())])).await?.0.0;
            let (columns, rows, completion) = result.map_err(|e| wasmtime::format_err!("{e:?}"))?;
            assert_eq!(columns, ["first", "第二"]);
            let (sink, completion_rx, _done) = stream_pump::future_channel();
            accessor.with(|access| completion.pipe(access, sink))?;
            let mut completion_rx = Box::pin(completion_rx);
            let (rows_tx, mut rows_rx) = futures::channel::mpsc::unbounded();
            accessor.with(|access| rows.pipe(access, RowConsumer(rows_tx)))?;
            // The header is available before the producer emits any rows.
            for n in 0..40 {
                row_tx.send(vec![PgValue::Integer(n), PgValue::Text(format!("row-{n}"))]).await?;
                let row = rows_rx.next().await.context("row stream ended early")?;
                ensure!(matches!(row.as_slice(), [PgValue::Integer(i), PgValue::Text(s)] if *i == n && s == &format!("row-{n}")), "row order or payload changed");
                assert!(futures::poll!(&mut completion_rx).is_pending());
            }
            drop(row_tx);
            assert!(rows_rx.next().await.is_none());
            assert!(futures::poll!(&mut completion_rx).is_pending());
            done_tx.send(if fail { Err(Error::Timeout) } else { Ok(()) }).map_err(|_| anyhow::anyhow!("completion reader dropped"))?;
            let result = completion_rx.await?;
            assert!(matches!((fail, result), (false, Ok(())) | (true, Err(Error::Timeout))));
            anyhow::Ok(())
        })).await???;
        let call = &h.store.data().calls[0];
        assert_eq!(call.operation, "query");
        assert_eq!(call.text, SQL);
        assert!(matches!(call.params.as_slice(), [PgValue::Text(s)] if s == "parameter"));
    }
    Ok(())
}

struct RowConsumer(futures::channel::mpsc::UnboundedSender<Row>);

impl<D: 'static> wasmtime::component::StreamConsumer<D> for RowConsumer {
    type Item = Row;

    fn poll_consume(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        mut store: wasmtime::StoreContextMut<D>,
        mut source: wasmtime::component::Source<'_, Row>,
        _: bool,
    ) -> std::task::Poll<wasmtime::Result<wasmtime::component::StreamResult>> {
        let mut rows = Vec::with_capacity(source.remaining(&mut store));
        source.read(&mut store, &mut rows)?;
        for row in rows {
            self.0
                .unbounded_send(row)
                .map_err(|_| wasmtime::format_err!("row reader dropped"))?;
        }
        std::task::Poll::Ready(Ok(wasmtime::component::StreamResult::Completed))
    }
}

#[cfg(feature = "wasm_component_model_implements")]
mod named {
    use std::collections::HashMap;

    use super::super::named_imports::wasmcloud::postgres0_2_0::{prepared, query};
    use super::*;
    use crate::plugin::wasmcloud_postgres::{PgId, WasmcloudPostgres};
    use crate::wit::WitInterface;

    impl query::Host for Host {}
    impl prepared::Host for Host {}

    impl<U> query::HostWithStore<U> for Host {
        async fn query(
            store: &Accessor<U, Self>,
            id: PgId,
            q: String,
            params: Vec<PgValue>,
        ) -> wasmtime::Result<QueryResult> {
            Self::query(store, id.pool().status().max_size, q, params).await
        }
        async fn query_batch(
            store: &Accessor<U, Self>,
            id: PgId,
            q: String,
        ) -> wasmtime::Result<Result<(), Error>> {
            Self::record(store, id.pool().status().max_size, "query-batch", q, vec![]).await
        }
    }

    impl<U> prepared::HostWithStore<U> for Host {
        async fn prepare(
            store: &Accessor<U, Self>,
            id: PgId,
            statement: String,
        ) -> wasmtime::Result<Result<String, Error>> {
            Ok(Self::record(
                store,
                id.pool().status().max_size,
                "prepare",
                statement,
                vec![],
            )
            .await?
            .map(|()| TOKEN.into()))
        }
        async fn exec(
            store: &Accessor<U, Self>,
            id: PgId,
            token: String,
            params: Vec<PgValue>,
        ) -> wasmtime::Result<Result<u64, Error>> {
            Ok(
                Self::record(store, id.pool().status().max_size, "exec", token, params)
                    .await?
                    .map(|()| AFFECTED),
            )
        }
    }

    // Default and named imports share value types and keep separate routes.
    fn contract() -> Result<String> {
        let source = include_str!("contract.wat");
        let (types, body) = source
            .split_once("  (import \"wasmcloud:postgres/query@0.2.0\"")
            .context("query import in contract")?;
        let body = format!("  (import \"wasmcloud:postgres/query@0.2.0\"{body}");
        let body = body
            .trim_end()
            .strip_suffix(')')
            .context("contract closing parenthesis")?;
        let mut contract = types.to_string();
        contract.push_str(body);
        for team in ["alpha", "beta"] {
            let mut copy = body
                .replace(
                    "\"wasmcloud:postgres/query@0.2.0\"",
                    &format!("\"{team}-query\" (implements \"wasmcloud:postgres/query@0.2.0\")"),
                )
                .replace(
                    "\"wasmcloud:postgres/prepared@0.2.0\"",
                    &format!(
                        "\"{team}-prepared\" (implements \"wasmcloud:postgres/prepared@0.2.0\")"
                    ),
                );
            for name in [
                "query", "prepared", "memory", "forward", "batch", "prepare", "exec",
            ] {
                copy = copy.replace(&format!("${name}"), &format!("${team}-{name}"));
            }
            copy = copy.replace(&format!("${team}-query-result"), "$query-result");
            for name in ["query", "query-batch", "prepare", "exec"] {
                copy = copy.replace(
                    &format!("\n  (func (export \"{name}\")"),
                    &format!("\n  (func (export \"{team}-{name}\")"),
                );
            }
            contract.push_str(&copy);
        }
        contract.push(')');
        Ok(contract)
    }

    async fn registry() -> Result<HashMap<String, PgId>> {
        // Different capacities make the captured PgId observable without opening connections.
        let interfaces = [
            ("alpha-query", 11),
            ("alpha-prepared", 12),
            ("beta-query", 21),
            ("beta-prepared", 22),
        ]
        .map(|(name, size)| WitInterface {
            namespace: "wasmcloud".into(),
            package: "postgres".into(),
            interfaces: [if name.ends_with("query") {
                "query"
            } else {
                "prepared"
            }
            .into()]
            .into_iter()
            .collect(),
            version: Some(semver::Version::new(0, 2, 0)),
            name: Some(name.into()),
            config: HashMap::from([(
                "url".into(),
                format!("postgres://test:test@127.0.0.1/test-{size}?sslmode=disable"),
            )]),
        });
        let registry = WasmcloudPostgres::multiplexer()
            .build_registry(&interfaces)
            .await?;
        for (name, size) in [
            ("alpha-query", 11),
            ("alpha-prepared", 12),
            ("beta-query", 21),
            ("beta-prepared", 22),
        ] {
            registry[name].pool().resize(size);
        }
        Ok(registry)
    }

    fn link(
        engine: &Engine,
        component: &Component,
        registry: &HashMap<String, PgId>,
        resolved: &mut Vec<String>,
    ) -> Result<Linker<Host>> {
        let mut linker = default_linker(engine)?;
        let mux = WasmcloudPostgres::multiplexer();
        query::add_to_linker::<_, Host>(
            &mut linker,
            component,
            |name| {
                resolved.push(name.into());
                mux.resolve(registry, name)
            },
            |host| host,
        )?;
        prepared::add_to_linker::<_, Host>(
            &mut linker,
            component,
            |name| {
                resolved.push(name.into());
                mux.resolve(registry, name)
            },
            |host| host,
        )?;
        Ok(linker)
    }

    #[tokio::test]
    async fn both_named_interfaces_capture_the_correct_pool_for_each_label() -> Result<()> {
        let engine = engine()?;
        let component = Component::new(&engine, wat::parse_str(contract()?)?)?;
        let registry = registry().await?;
        let mut resolved = vec![];
        let linker = link(&engine, &component, &registry, &mut resolved)?;
        resolved.sort();
        assert_eq!(
            resolved,
            [
                "alpha-prepared",
                "alpha-query",
                "beta-prepared",
                "beta-query"
            ]
        );
        // The linker owns its resolved IDs after the registry goes away.
        drop(registry);
        let mut h = Harness::instantiate(&engine, &component, linker).await?;
        for (team, query_route, prepared_route) in
            [("beta", 21, 22), ("alpha", 11, 12), ("beta", 21, 22)]
        {
            assert_eq!(
                h.call(&format!("{team}-prepare"), vec![Val::String(SQL.into())])
                    .await?,
                Val::Result(Ok(Some(Box::new(Val::String(TOKEN.into())))))
            );
            assert_eq!(
                h.call(
                    &format!("{team}-exec"),
                    vec![Val::String(TOKEN.into()), Val::List(parameters())]
                )
                .await?,
                Val::Result(Ok(Some(Box::new(Val::U64(AFFECTED)))))
            );
            h.store.data_mut().error = Some(Error::AccessDenied);
            let denied = Val::Result(Err(Some(Box::new(variant("access-denied", None)))));
            assert_eq!(
                h.call(
                    &format!("{team}-query"),
                    vec![Val::String(SQL.into()), Val::List(parameters())]
                )
                .await?,
                denied
            );
            assert_eq!(
                h.call(
                    &format!("{team}-query-batch"),
                    vec![Val::String(SQL.into())]
                )
                .await?,
                denied
            );
            h.store.data_mut().error = None;
            let calls = &h.store.data().calls;
            let calls = &calls[calls.len() - 4..];
            for (call, route, operation, text) in [
                (&calls[0], prepared_route, "prepare", SQL),
                (&calls[1], prepared_route, "exec", TOKEN),
                (&calls[2], query_route, "query", SQL),
                (&calls[3], query_route, "query-batch", SQL),
            ] {
                assert_eq!(call.route, route);
                assert_eq!(call.operation, operation);
                assert_eq!(call.text, text);
            }
            assert_parameters(&calls[1].params)?;
            assert_parameters(&calls[2].params)?;
        }
        round_trip_parameters(&mut h, "", 0).await?;
        round_trip_parameters(&mut h, "alpha-", 11).await?;
        round_trip_parameters(&mut h, "beta-", 21).await?;

        Ok(())
    }

    #[tokio::test]
    async fn unresolved_named_import_fails_linking_without_using_a_default_pool() -> Result<()> {
        let engine = engine()?;
        let component = Component::new(&engine, wat::parse_str(contract()?)?)?;
        for missing in ["beta-query", "beta-prepared"] {
            let mut registry = registry().await?;
            let default = registry.remove(missing).context("named pool exists")?;
            registry.insert(String::new(), default);
            let error = link(&engine, &component, &registry, &mut vec![])
                .err()
                .with_context(|| format!("unbound import {missing} linked"))?;
            assert!(format!("{error:#}").contains(missing), "{error:#}");
        }
        Ok(())
    }
}

#[tokio::test]
async fn linker_rejects_incompatible_import_versions_and_signatures() -> Result<()> {
    let engine = engine()?;
    for (from, to) in [
        (
            "wasmcloud:postgres/query@0.2.0",
            "wasmcloud:postgres/query@0.3.0",
        ),
        (
            "wasmcloud:postgres/prepared@0.2.0",
            "wasmcloud:postgres/prepared@0.3.0",
        ),
        ("(func async", "(func"),
        (
            "(result (result u64 (error $error)))",
            "(result (result u32 (error $error)))",
        ),
    ] {
        let source = include_str!("contract.wat").replace(from, to);
        let component = Component::new(&engine, wat::parse_str(source)?)?;
        assert!(
            default_linker(&engine)?
                .instantiate_pre(&component)
                .is_err(),
            "accepted incompatible contract: {from} -> {to}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn empty_streams_and_reader_cancellation_preserve_completion() -> Result<()> {
    for cancel in [false, true] {
        let mut h = Harness::new().await?;
        let (row_tx, row_rx) = mpsc::channel(1);
        let (done_tx, done_rx) = oneshot::channel();
        h.store.data_mut().rows = Some(row_rx);
        h.store.data_mut().completion = Some(done_rx);
        let query = h
            .instance
            .get_typed_func::<(String, Vec<PgValue>), (QueryExport,)>(&mut h.store, "query")?;
        tokio::time::timeout(
            Duration::from_secs(5),
            h.store.run_concurrent(async |accessor| {
                let result = query
                    .call_concurrent(accessor, (SQL.into(), vec![]))
                    .await?
                    .0
                    .0;
                let (columns, mut rows, completion) =
                    result.map_err(|e| wasmtime::format_err!("{e:?}"))?;
                assert_eq!(columns, ["first", "第二"]);
                if cancel {
                    rows.close_with(accessor)?;
                    row_tx.closed().await;
                    assert!(row_tx.send(vec![PgValue::Null]).await.is_err());
                } else {
                    let (rows_tx, mut rows_rx) = futures::channel::mpsc::unbounded();
                    accessor.with(|access| rows.pipe(access, RowConsumer(rows_tx)))?;
                    drop(row_tx);
                    assert!(rows_rx.next().await.is_none());
                }
                let (sink, completion_rx, _done) = stream_pump::future_channel();
                accessor.with(|access| completion.pipe(access, sink))?;
                done_tx
                    .send(Ok(()))
                    .map_err(|_| anyhow::anyhow!("completion reader dropped"))?;
                assert!(completion_rx.await?.is_ok());
                anyhow::Ok(())
            }),
        )
        .await???;
    }
    Ok(())
}

async fn round_trip_parameters(h: &mut Harness, prefix: &str, route: usize) -> Result<()> {
    h.call(
        &format!("{prefix}exec"),
        vec![Val::String(TOKEN.into()), Val::List(parameters())],
    )
    .await?;
    let row = std::mem::take(
        &mut h
            .store
            .data_mut()
            .calls
            .last_mut()
            .context("exec call recorded")?
            .params,
    );
    assert_parameters(&row)?;
    let columns = (0..row.len())
        .map(|i| format!("column-{i}"))
        .collect::<Vec<_>>();
    h.store.data_mut().columns = Some(columns.clone());
    let (row_tx, row_rx) = mpsc::channel(1);
    row_tx.send(row).await?;
    drop(row_tx);
    let (done_tx, done_rx) = oneshot::channel();
    done_tx
        .send(Ok(()))
        .map_err(|_| anyhow::anyhow!("completion reader dropped"))?;
    h.store.data_mut().rows = Some(row_rx);
    h.store.data_mut().completion = Some(done_rx);
    let value = h
        .call(
            &format!("{prefix}query"),
            vec![Val::String(SQL.into()), Val::List(parameters())],
        )
        .await?;
    let Val::Result(Ok(Some(value))) = value else {
        bail!("expected a successful query")
    };
    let Val::Tuple(values) = *value else {
        bail!("expected query result tuple")
    };
    let [columns, Val::Stream(rows), Val::Future(completion)]: [Val; 3] = values
        .try_into()
        .map_err(|_| anyhow::anyhow!("expected three query result fields"))?
    else {
        bail!("expected columns, stream, and future")
    };
    assert_eq!(
        columns,
        Val::List(
            (0..10)
                .map(|i| Val::String(format!("column-{i}")))
                .collect()
        )
    );
    let rows = rows.try_into_stream_reader::<Row>()?;
    let completion = completion.try_into_future_reader::<Result<(), Error>>()?;
    tokio::time::timeout(
        Duration::from_secs(5),
        h.store.run_concurrent(async |accessor| {
            let (rows_tx, mut rows_rx) = futures::channel::mpsc::unbounded();
            accessor.with(|access| rows.pipe(access, RowConsumer(rows_tx)))?;
            let row = rows_rx.next().await.context("missing row")?;
            assert_parameters(&row)?;
            assert!(rows_rx.next().await.is_none());
            let (sink, completion_rx, _done) = stream_pump::future_channel();
            accessor.with(|access| completion.pipe(access, sink))?;
            assert!(completion_rx.await?.is_ok());
            anyhow::Ok(())
        }),
    )
    .await???;
    let call = h.store.data().calls.last().context("query call recorded")?;
    assert_eq!(call.route, route);
    assert_eq!(call.operation, "query");
    assert_parameters(&call.params)?;
    Ok(())
}

#[tokio::test]
async fn parameter_values_round_trip_through_streamed_rows() -> Result<()> {
    round_trip_parameters(&mut Harness::new().await?, "", 0).await
}
