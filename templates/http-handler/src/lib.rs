use axum::{
    Json, Router,
    extract::Query,
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

#[wstd_axum::http_server]
fn main() -> Router {
    Router::new()
        .route("/", get(hello))
        .route("/api/echo", post(echo))
        .route("/api/greet", get(greet))
        .fallback(not_found)
}

async fn hello() -> &'static str {
    "Hello from wasmCloud!\n"
}

#[derive(Deserialize)]
struct GreetParams {
    name: Option<String>,
}

async fn greet(Query(params): Query<GreetParams>) -> String {
    let name = params.name.as_deref().unwrap_or("world");
    format!("Hello, {name}!\n")
}

#[derive(Deserialize, Serialize)]
struct EchoBody {
    message: String,
}

async fn echo(Json(body): Json<EchoBody>) -> Json<EchoBody> {
    Json(body)
}

async fn not_found() -> (StatusCode, &'static str) {
    (StatusCode::NOT_FOUND, "Not found\n")
}
