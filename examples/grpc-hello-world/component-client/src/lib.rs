use std::pin::Pin;
use std::task::{Context, Poll};

use http_body_util::BodyExt;
use serde::Deserialize;
use tonic::Request as GrpcRequest;

use hello_world::HelloRequest;
use hello_world::greeter_client::GreeterClient;

use wstd::http::{Body, Request, Response, StatusCode};

pub mod hello_world {
    tonic::include_proto!("helloworld");
}

static UI_HTML: &str = include_str!("../ui.html");

// -- WasiGrpcService: bridges tonic/tower to wstd HTTP client --

struct WasiGrpcService {
    base_uri: http::Uri,
}

impl WasiGrpcService {
    fn new(base_uri: http::Uri) -> Self {
        Self { base_uri }
    }
}

impl tower_service::Service<http::Request<tonic::body::BoxBody>> for WasiGrpcService {
    type Response =
        http::Response<http_body_util::combinators::UnsyncBoxBody<bytes::Bytes, tonic::Status>>;
    type Error = Box<dyn std::error::Error + Send + Sync>;
    type Future =
        Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<tonic::body::BoxBody>) -> Self::Future {
        let base_uri = self.base_uri.clone();

        Box::pin(async move {
            // Rebuild URI: base scheme+authority + request path+query
            let mut parts = base_uri.into_parts();
            if let Some(pq) = req.uri().path_and_query().cloned() {
                parts.path_and_query = Some(pq);
            }
            let uri = http::Uri::from_parts(parts)?;

            // Split request, convert body
            let (mut head, body) = req.into_parts();
            head.uri = uri;

            // Collect tonic BoxBody into bytes, then wrap as wstd Body
            let collected = body
                .collect()
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?
                .to_bytes();
            let wasi_body = Body::from(collected);
            let req = http::Request::from_parts(head, wasi_body);

            // Send via wstd HTTP client
            let client = wstd::http::Client::new();
            let resp = client
                .send(req)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;

            // Convert response body back
            let (resp_head, resp_body) = resp.into_parts();
            let boxed = resp_body
                .into_boxed_body()
                .map_err(|e| tonic::Status::internal(format!("{e:#}")));
            let boxed = http_body_util::combinators::UnsyncBoxBody::new(boxed);
            Ok(http::Response::from_parts(resp_head, boxed))
        })
    }
}

#[wstd::http_server]
async fn main(req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    match router(req).await {
        Ok(resp) => Ok(resp),
        Err(e) => {
            eprintln!("Error handling request: {:?}", e);
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body("sadness. go check logs.\n".into())
                .map_err(Into::into)
        }
    }
}

async fn router(req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    match req.uri().path() {
        "/" => home(req).await,
        "/greet" => greet(req).await,
        _ => not_found(req).await,
    }
}

async fn home(_req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/html")
        .body(UI_HTML.into())
        .map_err(Into::into)
}

#[derive(Deserialize)]
struct GreetRequest {
    name: String,
}

async fn greet(mut req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    let js_req: GreetRequest = req.body_mut().json().await?;

    // Create the gRPC client
    let svc = WasiGrpcService::new("http://localhost:50051".parse().unwrap());
    let mut client = GreeterClient::new(svc);

    // Make the gRPC call
    let request = GrpcRequest::new(HelloRequest { name: js_req.name });

    eprintln!("Sending gRPC request...");
    let response = client.say_hello(request).await?;

    let message = response.into_inner().message;
    eprintln!("gRPC Response: {}", message);

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/plain")
        .body(message.into())
        .map_err(Into::into)
}

async fn not_found(_req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body("Not found\n".into())
        .map_err(Into::into)
}
