use async_trait::async_trait;
use tracing::instrument;

use crate::capability::logging::logging;

use super::{Ctx, Handler};

pub mod unversioned_logging_bindings {
    wasmtime::component::bindgen!({
        world: "unversioned-logging",
        async: true,
        with: {
           "wasi:logging/logging": crate::capability::unversioned_logging,
        },
    });
}

pub mod logging_bindings {
    wasmtime::component::bindgen!({
        world: "logging",
        async: true,
        with: {
           "wasi:logging/logging": crate::capability::logging,
        },
    });
}

/// `wasi:logging/logging` implementation
#[async_trait]
pub trait Logging {
    /// Handle `wasi:logging/logging.log`
    async fn log(
        &self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()>;
}

impl<H: Handler> logging::Host for Ctx<H> {
    #[instrument(skip_all)]
    async fn log(
        &mut self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()> {
        self.attach_parent_context();
        self.handler.log(level, context, message).await
    }
}

impl<H: Handler> crate::capability::unversioned_logging::logging::Host for Ctx<H> {
    #[instrument(skip_all)]
    async fn log(
        &mut self,
        level: crate::capability::unversioned_logging::logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()> {
        use crate::capability::unversioned_logging::logging::Level;

        self.attach_parent_context();
        // NOTE(thomastaylor312): I couldn't figure out the proper incantation for using `with` to
        // avoid this. If there is a better way, we can fix it
        let level = match level {
            Level::Trace => logging::Level::Trace,
            Level::Debug => logging::Level::Debug,
            Level::Info => logging::Level::Info,
            Level::Warn => logging::Level::Warn,
            Level::Error => logging::Level::Error,
            Level::Critical => logging::Level::Critical,
        };
        self.handler.log(level, context, message).await
    }
}
