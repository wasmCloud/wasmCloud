#![cfg(feature = "bin")]
#![warn(clippy::pedantic)]
#![forbid(clippy::unwrap_used)]

use std::env::args;

use anyhow::{self, bail, ensure, Context};
use tokio::fs;
use tokio::io::{stdin, AsyncReadExt};
use tracing_subscriber::prelude::*;
use wascap::jwt;
use wasmcloud::capability::HostHandlerBuilder;
use wasmcloud::{ActorModule, ActorResponse, Runtime};

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

    let rt: Runtime<_> = Runtime::builder(
        HostHandlerBuilder::new(
            |claims: &jwt::Claims<jwt::Actor>,
             bd,
             ns,
             op,
             pld|
             -> anyhow::Result<anyhow::Result<[u8; 0]>> {
                bail!(
                    "cannot execute `{bd}.{ns}.{op}` with payload {pld:?} for actor `{}`",
                    claims.subject
                )
            },
        )
        .build(),
    )
    .into();
    let first = args.next().with_context(usage)?;
    let (actor, op) = match (first.as_str(), args.next(), args.next()) {
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

    let ActorResponse {
        code,
        console_log,
        response,
    } = ActorModule::new(&rt, actor)
        .context("failed to create actor")?
        .instantiate()
        .context("failed to instantiate actor")?
        .call(&op, &pld)
        .with_context(|| format!("failed to call `{op}` with payload {pld:?}"))?;
    for log in console_log {
        eprintln!("{log}");
    }
    if let Some(response) = response {
        println!("{response:?}");
    }
    ensure!(code == 1, "actor returned code `{code}`");
    Ok(())
}
