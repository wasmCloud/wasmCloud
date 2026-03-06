mod bindings {
    wit_bindgen::generate!({
        path: "../wit",
        world: "http-api",
        generate_all,
    });
}

use anyhow::Context as _;
use bindings::wasmcloud::messaging::consumer;

use serde::Deserialize;
use wstd::{
    http::{Body, Request, Response, StatusCode},
    time::Duration,
};

static UI_HTML: &str = include_str!("../ui.html");

#[wstd::http_server]
async fn main(req: Request<Body>) -> anyhow::Result<Response<Body>> {
    match req.uri().path() {
        "/" => home(req).await,
        "/task" => create_task(req).await,
        _ => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body("Not found\n".into())
            .map_err(Into::into),
    }
}

async fn home(_req: Request<Body>) -> anyhow::Result<Response<Body>> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/html")
        .body(UI_HTML.into())
        .map_err(Into::into)
}

#[derive(Deserialize)]
struct TaskRequest {
    worker: Option<String>,
    payload: String,
}

async fn create_task(mut req: Request<Body>) -> anyhow::Result<Response<Body>> {
    let task_request: TaskRequest = req
        .body_mut()
        .json()
        .await
        .context("failed to parse body")?;

    let subject = format!(
        "tasks.{}",
        task_request.worker.unwrap_or_else(|| "default".to_string())
    );

    let body = task_request.payload.into_bytes();
    let request_timeout = Duration::from_secs(5).as_millis() as u32;

    match consumer::request(&subject, &body, request_timeout) {
        Ok(resp) => Response::builder()
            .status(StatusCode::OK)
            .body(resp.body.into())
            .map_err(Into::into),
        Err(err) => Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .body(err.into())
            .map_err(Into::into),
    }
}
