use super::{Ctx, Instance, InterfaceBindings, InterfaceInstance};

use crate::capability::logging::logging;
use crate::capability::Logging;

use std::io::Cursor;
use std::sync::Arc;

use anyhow::{anyhow, Context as _};
use async_trait::async_trait;
use serde_json::json;
use tokio::io::sink;
use tokio::sync::Mutex;
use tracing::{instrument, trace};

pub mod logging_bindings {
    wasmtime::component::bindgen!({
        world: "logging",
        async: true,
        with: {
           "wasi:logging/logging": crate::capability::logging,
        },
    });
}

impl Instance {
    /// Set [`Logging`] handler for this [Instance].
    pub fn logging(&mut self, logging: Arc<dyn Logging + Send + Sync>) -> &mut Self {
        self.handler_mut().replace_logging(logging);
        self
    }

    /// Instantiates and returns an [`InterfaceInstance<logging_bindings::Logging>`] if exported by the [`Instance`].
    ///
    /// # Errors
    ///
    /// Fails if logging bindings are not exported by the [`Instance`]
    pub async fn into_logging(
        mut self,
    ) -> anyhow::Result<InterfaceInstance<logging_bindings::Logging>> {
        let bindings = if let Ok((bindings, _)) = logging_bindings::Logging::instantiate_async(
            &mut self.store,
            &self.component,
            &self.linker,
        )
        .await
        {
            InterfaceBindings::Interface(bindings)
        } else {
            self.as_guest_bindings()
                .await
                .map(InterfaceBindings::Guest)
                .context("failed to instantiate `wasi:logging/logging` interface")?
        };
        Ok(InterfaceInstance {
            store: Mutex::new(self.store),
            bindings,
        })
    }
}

#[async_trait]
impl logging::Host for Ctx {
    #[instrument(skip_all)]
    async fn log(
        &mut self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()> {
        self.handler.log(level, context, message).await
    }
}

#[async_trait]
impl Logging for InterfaceInstance<logging_bindings::Logging> {
    #[instrument(skip(self))]
    async fn log(
        &self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()> {
        let mut store = self.store.lock().await;
        match &self.bindings {
            InterfaceBindings::Guest(guest) => {
                let level = match level {
                    logging::Level::Trace => "trace",
                    logging::Level::Debug => "debug",
                    logging::Level::Info => "info",
                    logging::Level::Warn => "warn",
                    logging::Level::Error => "error",
                    logging::Level::Critical => "critical",
                };
                let request = serde_json::to_vec(&json!({
                    "level": level,
                    "context": context,
                    "message": message,
                }))
                .context("failed to encode request")?;
                guest
                    .call(
                        &mut store,
                        "wasi:logging/logging.log",
                        Cursor::new(request),
                        sink(),
                    )
                    .await
                    .context("failed to call actor")?
                    .map_err(|e| anyhow!(e))
            }
            InterfaceBindings::Interface(bindings) => {
                // NOTE: It appears that unifying the `Level` type is not possible currently
                use logging_bindings::exports::wasi::logging::logging::Level;
                let level = match level {
                    logging::Level::Trace => Level::Trace,
                    logging::Level::Debug => Level::Debug,
                    logging::Level::Info => Level::Info,
                    logging::Level::Warn => Level::Warn,
                    logging::Level::Error => Level::Error,
                    logging::Level::Critical => Level::Critical,
                };
                trace!("call `wasi:logging/logging.log`");
                bindings
                    .wasi_logging_logging()
                    .call_log(&mut *store, level, &context, &message)
                    .await
            }
        }
    }
}
