#![warn(clippy::pedantic)]
#![forbid(clippy::unwrap_used)]

use std::env::args;
use std::ops::Deref;
use std::sync::Arc;

use anyhow::{self, bail, Context};
use async_trait::async_trait;
use tokio::fs;
use tokio::io::{stdin, AsyncReadExt};
use tracing::{debug, error, info, trace, warn};
use tracing_subscriber::prelude::*;
use wascap::jwt;
use wasmcloud_host::capability::{host, logging};
use wasmcloud_host::{Actor, Runtime};

#[derive(Clone)]
struct Handler {
    claims: jwt::Claims<jwt::Actor>,
}

#[derive(Clone)]
struct HandlerArc(Arc<Handler>);

impl Deref for HandlerArc {
    type Target = Handler;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Handler> for HandlerArc {
    fn from(handler: Handler) -> Self {
        Self(handler.into())
    }
}

#[async_trait]
impl host::Host for HandlerArc {
    async fn call(
        &mut self,
        binding: String,
        namespace: String,
        operation: String,
        _payload: Option<Vec<u8>>,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, String>> {
        Ok(Err(format!(
            "host cannot handle `{namespace}.{operation}` within `{binding}` requested by {}",
            self.claims.subject
        )))
    }
}

#[async_trait]
impl logging::Host for HandlerArc {
    async fn log(
        &mut self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()> {
        let subject = self.claims.subject.as_str();
        match level {
            logging::Level::Trace => trace!(subject, context, message),
            logging::Level::Debug => debug!(subject, context, message),
            logging::Level::Info => info!(subject, context, message),
            logging::Level::Warn => warn!(subject, context, message),
            logging::Level::Error => error!(subject, context, message),
        }
        Ok(())
    }
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

    let rt = Runtime::new().context("failed to construct runtime")?;

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

    let (actor, claims) = Actor::new(&rt, actor)
        .context("failed to create actor")?
        .into_configure_claims();

    let handler = HandlerArc::from(Handler { claims });
    match actor
        .logging(handler.clone())
        .host(handler.clone())
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
