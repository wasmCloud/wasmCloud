use serde_json::json;
use wasmbus_rpc::actor::prelude::*;
use wasmbus_rpc::core::{Actor, ActorReceiver, HealthCheckRequest, HealthCheckResponse};
use wasmcloud_example_hello::{Hello, HelloSender};
use wasmcloud_example_httpserver::{HttpRequest, HttpResponse, HttpServer, HttpServerReceiver};

const HELLO_PROVIDER_ID: &str = "wasmcloud:hello";

#[derive(Debug, Default, Actor)]
#[services(Actor, HttpServer)]
struct HelloSendActor {}

/// Implementation of HttpServer trait methods
#[async_trait]
impl HttpServer for HelloSendActor {
    async fn handle_request(
        &self,
        _ctx: &context::Context<'_>,
        req: &HttpRequest,
    ) -> std::result::Result<HttpResponse, RpcError> {
        // upon receiving http request, send hello
        let client = HelloSender::new(
            client::SendConfig::target(HELLO_PROVIDER_ID),
            WasmHost::default(),
        );
        // If a query string exists and contains "name=NAME", send the value NAME, otherwise send "World"
        let name = form_urlencoded::parse(req.query_string.as_bytes())
            .find(|(n, _)| n == "name")
            .map(|(_, v)| v.to_string())
            .unwrap_or("World".to_string());

        // send hello and wait for response
        let hello_response = client
            .say_hello(&context::Context::default(), &name)
            .await?;

        let body = json!({
            "response": &hello_response,
        });
        let resp = HttpResponse {
            body: serde_json::to_vec(&body)
                .map_err(|e| RpcError::ActorHandler(format!("serializing response: {}", e)))?,
            ..Default::default()
        };
        Ok(resp)
    }
}

/// Implementation of Actor trait methods
#[async_trait]
impl Actor for HelloSendActor {
    async fn health_request(
        &self,
        _ctx: &context::Context<'_>,
        _value: &HealthCheckRequest,
    ) -> std::result::Result<HealthCheckResponse, RpcError> {
        Ok(HealthCheckResponse {
            healthy: false,
            message: Some(String::from("OK")),
        })
    }
}
