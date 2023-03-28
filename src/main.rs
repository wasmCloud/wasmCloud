#![cfg(feature = "bin")]
#![warn(clippy::pedantic)]
#![forbid(clippy::unwrap_used)]

use std::env::args;

use anyhow::{self, bail, Context};
use tokio::fs;
use tokio::io::{stdin, AsyncReadExt};
use tracing_subscriber::prelude::*;
use wascap::jwt;
use wasmcloud::capability::{HandlerFunc, HostInvocation};
use wasmcloud::{Actor, Runtime};

#[allow(clippy::unused_async)]
async fn host_call(
    claims: jwt::Claims<jwt::Actor>,
    binding: String,
    invocation: HostInvocation,
    _call_context: Option<Vec<u8>>,
) -> anyhow::Result<Option<[u8; 0]>> {
    bail!(
        "cannot execute `{invocation:?}` within binding `{binding}` for actor `{}`",
        claims.subject
    )
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().pretty().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,cranelift_codegen=warn")
            }),
        )
        .init();

    let mut args = args();
    let name = args.next().context("argv[0] not set")?;
    let usage = || format!("Usage: {name} [--version | [actor-wasm op]]");

    let rt = Runtime::from_host_handler(HandlerFunc::from(host_call))
        .context("failed to construct runtime")?;

    let first = args.next().with_context(usage)?;
    let second = args.next();
    let (actor, op) = match (first.as_str(), second, args.next()) {
        ("--version", None, None) => {
            println!("wasmCloud Runtime Version: {}", rt.version());
            return Ok(());
        }
        (_, Some(op), None) => (first, op),
        _ => bail!(usage()),
    };

    let mut pld = vec![];
    _ = stdin()
        .read_to_end(&mut pld)
        .await
        .context("failed to read payload from STDIN")?;

    let actor = fs::read(&actor)
        .await
        .with_context(|| format!("failed to read `{actor}`"))?;

    match Actor::new(&rt, actor)
        .context("failed to create actor")?
        .call(op, Some(pld))
        .await
        .context("failed to call actor")?
    {
        Ok(Some(response)) => {
            println!("{response:?}");
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(err) => bail!("operation failed with: {err}"),
    }
}
