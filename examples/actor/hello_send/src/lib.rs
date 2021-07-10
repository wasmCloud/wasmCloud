use serde_json::json;
use wasmbus_rpc::actor::prelude::*;
use wasmbus_rpc::core::{Actor, ActorReceiver, HealthCheckRequest, HealthCheckResponse};
use wasmcloud_example_hello::{Hello, HelloSender};
use wasmcloud_example_httpserver::{HttpRequest, HttpResponse, HttpServer, HttpServerReceiver};

const HELLO_PROVIDER: &str = "wasmcloud:example:hello";
//const LINK_DEFAULT: &str = "default";

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
        console_log("HelloSend: received request");
        // upon receiving http request, send hello
        let client = HelloSender::new(
            client::SendConfig::contract(HELLO_PROVIDER),
            WasmHost::default(),
        );
        // If a query string exists and contains "name=NAME", send the value NAME, otherwise send "World"
        let name = form_urlencoded::parse(req.query_string.as_bytes())
            .find(|(n, _)| n == "name")
            .map(|(_, v)| v.to_string())
            .unwrap_or("World11".to_string());

        console_log(&format!("HelloSend: sending '{}'", name));
        // send hello and wait for response
        let hello_response: String = client
            .say_hello(&context::Context::default(), &name)
            .await?;

        console_log(&format!("HelloSend: received '{}'", hello_response));
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
