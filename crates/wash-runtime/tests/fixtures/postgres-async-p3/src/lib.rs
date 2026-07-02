//! Real-guest fixture for the async (wasip3) `wasmcloud:postgres@0.2.0` surface.
//!
//! On each HTTP request it runs a `SELECT`, drains the returned `stream<row>`
//! incrementally, then awaits the completion `future`. The response body reports
//! the column list and the streamed text values (or the error), so a test can
//! assert the whole async query path — host binding, row streaming, and the
//! completion signal — end to end.

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

const SELECT: &str = "SELECT val FROM items ORDER BY id";

impl Handler for Component {
    async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        let body = run().await.into_bytes();

        let headers = Fields::new();
        let (mut tx, rx) = bindings::wit_stream::new::<u8>();
        let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());
        wit_bindgen::spawn_local(async move {
            tx.write_all(body).await;
            drop(tx);
            let _ = trailers_tx.write(Ok(None)).await;
        });

        let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
        Ok(response)
    }
}

/// Run the query, drain the row stream, await completion, and render a summary.
async fn run() -> String {
    let (columns, mut rows, completion) = match query::query(SELECT.to_string(), Vec::new()).await {
        Ok(triple) => triple,
        Err(e) => return format!("query-error: {e:?}"),
    };

    // Drain the stream one row at a time; take the first column's text value.
    let mut values = Vec::new();
    while let Some(row) = rows.next().await {
        match row.into_iter().next() {
            Some(PgValue::Text(s)) => values.push(s),
            other => values.push(format!("{other:?}")),
        }
    }

    // The completion future resolves once the stream is exhausted.
    match completion.await {
        Ok(()) => format!("cols={} rows={}", columns.join(","), values.join(",")),
        Err(e) => format!("stream-error: {e:?}"),
    }
}

bindings::export!(Component with_types_in bindings);
