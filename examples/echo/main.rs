use anyhow::{bail, Context};
use tokio::fs;
use wascap::prelude::{ClaimsBuilder, KeyPair};
use wascap::wasm::embed_claims;
use wascap::{caps, jwt};
use wasmbus_rpc::common::{deserialize, serialize};
use wasmcloud::capability::{HandlerFunc, HostInvocation};
use wasmcloud::{Actor, Runtime};
use wasmcloud_interface_httpserver::{HttpRequest, HttpResponse};

#[allow(clippy::unused_async)]
async fn host_call(
    claims: jwt::Claims<jwt::Actor>,
    binding: String,
    invocation: HostInvocation,
) -> anyhow::Result<Option<[u8; 0]>> {
    bail!(
        "cannot execute `{invocation:?}` within binding `{binding}` for actor `{}`",
        claims.subject
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    const WASM: &str = env!("CARGO_CDYLIB_FILE_ACTOR_ECHO_MODULE");
    let wasm = fs::read(WASM)
        .await
        .with_context(|| format!("failed to read `{WASM}`"))?;

    let issuer = KeyPair::new_account();
    let module = KeyPair::new_module();
    let claims = ClaimsBuilder::new()
        .issuer(&issuer.public_key())
        .subject(&module.public_key())
        .with_metadata(jwt::Actor {
            name: Some("echo".into()),
            caps: Some(vec![caps::HTTP_SERVER.into()]),
            ..Default::default()
        })
        .build();
    let wasm = embed_claims(&wasm, &claims, &issuer).context("failed to embed actor claims")?;

    let rt = Runtime::from_host_handler(HandlerFunc::from(host_call))
        .context("failed to construct runtime")?;

    let actor = Actor::read(&rt, wasm.as_slice())
        .await
        .context("failed to construct actor")?;
    let buf = serialize(&HttpRequest::default()).context("failed to encode request")?;
    match actor
        .call("HttpServer.HandleRequest", Some(buf))
        .await
        .context("failed to call `HttpServer.HandleRequest`")?
    {
        Ok(Some(response)) => {
            let HttpResponse {
                status_code,
                header,
                body,
            } = deserialize(&response).context("failed to deserialize response")?;
            println!("Status code: {status_code}");
            for (k, v) in header {
                println!("Header `{k}`: `{v:?}`");
            }
            let body = String::from_utf8(body).context("failed to convert body to UTF-8")?;
            println!("Body: {body}");
            Ok(())
        }
        Ok(None) => bail!("actor did not return a response"),
        Err(err) => bail!("actor failed with: {err}"),
    }
}
