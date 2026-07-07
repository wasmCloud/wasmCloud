//! Streaming/volume fixture for the async `wasmcloud:postgres@0.2.0`.
//!
//! The query is chosen by request path so one component can drive several
//! scenarios:
//!   - `/huge`  — a large `generate_series` result; the guest *reduces* it to a
//!     running count/sum without ever holding the rows, so the whole set moves
//!     through the host's bounded channel a few rows at a time. Proves the
//!     streaming path handles a huge result set without buffering it.
//!   - `/paced` — eight rows emitted ~120ms apart (a per-row `pg_sleep`),
//!     forwarded to the response body as each arrives. A test times the chunks
//!     to prove rows stream through incrementally rather than being buffered.
//!   - anything else — reads `items` and forwards it (a small sanity default).

mod bindings {
    wit_bindgen::generate!({
        generate_all,
        async: [
            "import:wasmcloud:postgres/query@0.2.0#query",
            "import:wasmcloud:postgres/query@0.2.0#query-batch",
            "export:wasi:http/handler@0.3.0#handle",
        ],
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasmcloud::postgres::query;
use bindings::wasmcloud::postgres::types::PgValue;

struct Component;

/// The SQL and whether to forward rows (`true`) or reduce them (`false`),
/// selected by request path.
fn route(path: &str) -> (&'static str, bool) {
    if path.starts_with("/huge") {
        // A large result set the guest reduces to a count/sum. The single column
        // is cast to text so it converts cleanly and parses back to an integer,
        // and explicitly aliased `n` so the count-mode response can echo the
        // returned column name back for the test to check.
        (
            "SELECT (g.n)::text AS n FROM generate_series(1, 50000) AS g(n)",
            false,
        )
    } else if path.starts_with("/paced") {
        // Eight rows, one every ~120ms. The LATERAL subquery references `g.i`, so
        // it is correlated and re-evaluated per outer row — the volatile
        // `pg_sleep` then runs once per row and the server produces the rows
        // spread over time. (A non-correlated `LATERAL (SELECT pg_sleep)` is
        // hoisted and evaluated once, which would not pace.)
        //
        // Each row is 9000 bytes on purpose: postgres output-buffers rows and
        // only flushes to the socket when its send buffer (~8KB) fills or the
        // query ends, so *small* paced rows would all land together at the end.
        // A row that overflows the buffer is flushed on its own, so the client
        // (through the host's streaming query) sees them arrive incrementally.
        (
            "SELECT repeat('x', 9000) FROM generate_series(1, 8) AS g(i) \
             CROSS JOIN LATERAL (SELECT pg_sleep(0.12) WHERE g.i IS NOT NULL) AS s",
            true,
        )
    } else {
        ("SELECT val FROM items ORDER BY id", true)
    }
}

/// The first column of a row, when it is text (all query paths cast col 0 to
/// text). Rows whose first column is not text are skipped.
fn first_text(row: Vec<PgValue>) -> Option<String> {
    match row.into_iter().next() {
        Some(PgValue::Text(s)) => Some(s),
        _ => None,
    }
}

/// A response whose body is `body`, written once and closed.
fn respond(body: Vec<u8>) -> Response {
    let (mut tx, rx) = bindings::wit_stream::new::<u8>();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());
    wit_bindgen::spawn_local(async move {
        tx.write_all(body).await;
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });
    let (response, _result) = Response::new(Fields::new(), Some(rx), trailers_rx);
    response
}

impl Handler for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let path = request.get_path_with_query().unwrap_or_default();
        let (sql, forward) = route(&path);

        let (columns, mut rows, completion) =
            match query::query(sql.to_string(), Vec::new()).await {
                Ok(triple) => triple,
                Err(e) => return Ok(respond(format!("query-error: {e:?}").into_bytes())),
            };

        if forward {
            // Forward each row's value to the response body as it arrives, so a
            // paced query streams through incrementally end to end.
            let (mut tx, rx) = bindings::wit_stream::new::<u8>();
            let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());
            wit_bindgen::spawn_local(async move {
                while let Some(row) = rows.next().await {
                    if let Some(v) = first_text(row) {
                        tx.write_all(format!("{v}\n").into_bytes()).await;
                    }
                }
                let _ = completion.await;
                drop(tx);
                let _ = trailers_tx.write(Ok(None)).await;
            });
            let (response, _result) = Response::new(Fields::new(), Some(rx), trailers_rx);
            Ok(response)
        } else {
            // Reduce a huge result set without accumulating rows: the host
            // streams them through its bounded channel; we keep only a running
            // count and sum, bounding guest memory too.
            let mut count: u64 = 0;
            let mut sum: i128 = 0;
            while let Some(row) = rows.next().await {
                count += 1;
                if let Some(v) = first_text(row) {
                    sum += v.parse::<i128>().unwrap_or(0);
                }
            }
            // Echo the returned column names too, so the test verifies the
            // query's `list<column-name>` threads back through the host.
            let body = match completion.await {
                Ok(()) => format!("count={count} sum={sum} cols={}", columns.join(",")),
                Err(e) => format!("stream-error: {e:?}"),
            };
            Ok(respond(body.into_bytes()))
        }
    }
}

bindings::export!(Component with_types_in bindings);
