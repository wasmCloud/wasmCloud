use http_body_util::BodyExt;
use tower_service::Service;

use hello_world::greeter_server::{Greeter, GreeterServer};
use hello_world::{HelloReply, HelloRequest};

use wstd::http::{Body, Request, Response};

pub mod hello_world {
    tonic::include_proto!("helloworld");
}

#[derive(Debug, Default)]
pub struct MyGreeter;

#[tonic::async_trait]
impl Greeter for MyGreeter {
    async fn say_hello(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, tonic::Status> {
        let name = request.into_inner().name;
        eprintln!("Got gRPC request for: {}", name);
        Ok(tonic::Response::new(HelloReply {
            message: format!("Hello from gRPC component: {}!", name),
        }))
    }
}

#[wstd::http_server]
async fn main(req: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    // wstd body → Full<Bytes> for tonic
    let (parts, wstd_body) = req.into_parts();
    let collected = wstd_body.into_boxed_body().collect().await?;
    let http_req = http::Request::from_parts(parts, http_body_util::Full::new(collected.to_bytes()));

    // Dispatch through tonic server
    let mut svc = GreeterServer::new(MyGreeter);
    let http_resp = svc.call(http_req).await.expect("Infallible");

    // tonic BoxBody → wstd Body (map tonic::Status error to anyhow)
    let (resp_parts, tonic_body) = http_resp.into_parts();
    let mapped = tonic_body.map_err(|s| anyhow::anyhow!("tonic: {s}"));
    Ok(http::Response::from_parts(resp_parts, Body::from_http_body(mapped)))
}
