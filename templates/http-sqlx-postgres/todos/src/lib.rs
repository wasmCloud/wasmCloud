//! The stateless `/todos` backend.
//!
//! It exports `wasmcloud:app/todos` and is invoked by the Service's router
//! over a host-linked WIT call — it does no HTTP or socket work of its own.
//! For data it runs ordinary sqlx against the Service's **loopback Postgres
//! endpoint** (`127.0.0.1:6432`): a client-side `PgPool` keeps the loopback
//! connection warm while this instance lives, and the Service maps it onto a
//! shared, pre-authenticated session to the real database.
//!
//! Note what is absent: database credentials. The loopback endpoint is only
//! reachable from inside the workload, so it accepts the connection without
//! them; the real credentials live in the Service alone. And because the
//! Service pools the expensive upstream sessions, this component stays
//! serverless — it can be torn down at any time and the next instance still
//! hits a warm database connection.

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "todos" });
}

use std::sync::OnceLock;

use bindings::exports::wasmcloud::app::todos::{Guest, Request, Response};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

/// The Service's loopback endpoint. Fixed because sidecar components do not
/// receive workload config — the Service holds the real upstream address and
/// credentials.
const POOLER_URL: &str = "postgres://app@127.0.0.1:6432/app?sslmode=disable";

/// sqlx drives its I/O on a Tokio current-thread runtime; it must be the same
/// runtime across calls so pooled connections created in one call are usable
/// in the next.
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static POOL: OnceLock<PgPool> = OnceLock::new();

struct Component;

impl Guest for Component {
    async fn handle(req: Request) -> Response {
        if req.method != "GET" {
            return json(405, r#"{"error":"method not allowed"}"#.to_string());
        }
        match query() {
            Ok(body) => json(200, body),
            Err(e) => json(500, format!("{{\"error\":{}}}", json_str(&e.to_string()))),
        }
    }
}

fn query() -> anyhow::Result<String> {
    let rt = RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime")
    });
    rt.block_on(async {
        // One connection is enough: requests to this instance are serialized
        // through `block_on`. The win is reuse — while this instance is warm
        // its loopback connection (and the upstream session behind it) is
        // kept open instead of being re-established per request.
        let pool = POOL.get_or_init(|| {
            PgPoolOptions::new()
                .max_connections(1)
                .connect_lazy(POOLER_URL)
                .expect("invalid pooler URL")
        });
        let rows = sqlx::query("SELECT id, title, done FROM todos ORDER BY id")
            .fetch_all(pool)
            .await?;
        let items: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.get::<i32, _>("id"),
                    "title": r.get::<String, _>("title"),
                    "done": r.get::<bool, _>("done"),
                })
            })
            .collect();
        Ok::<_, anyhow::Error>(serde_json::json!({ "todos": items }).to_string())
    })
}

fn json_str(s: &str) -> String {
    serde_json::Value::String(s.to_string()).to_string()
}

fn json(status: u16, body: String) -> Response {
    Response {
        status,
        content_type: "application/json".to_string(),
        body: body.into_bytes(),
    }
}

mod export {
    #![allow(unsafe_code)]
    use super::{bindings, Component};
    bindings::export!(Component with_types_in bindings);
}
