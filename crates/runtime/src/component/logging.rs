use super::{Ctx, Handler};

use crate::capability::logging::logging;

use async_trait::async_trait;
use tracing::instrument;

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

//impl<H: Handler> Instance<H> {
//    /// Instantiates and returns an [`InterfaceInstance<logging_bindings::Logging>`] if exported by the [`Instance`].
//    ///
//    /// # Errors
//    ///
//    /// Fails if logging bindings are not exported by the [`Instance`]
//    pub async fn into_logging(
//        mut self,
//    ) -> anyhow::Result<InterfaceInstance<logging_bindings::Logging, H>> {
//        let (bindings, _) =
//            logging_bindings::Logging::instantiate_pre(&mut self.store, &self.instance_pre).await?;
//        Ok(InterfaceInstance {
//            store: Mutex::new(self.store),
//            bindings,
//        })
//    }
//}

#[async_trait]
impl<H: Handler> logging::Host for Ctx<H> {
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

//#[async_trait]
//impl<H: Handler> Logging for InterfaceInstance<logging_bindings::Logging, H> {
//    #[instrument(skip(self))]
//    async fn log(
//        &self,
//        level: logging::Level,
//        context: String,
//        message: String,
//    ) -> anyhow::Result<()> {
//        let mut store = self.store.lock().await;
//        // NOTE: It appears that unifying the `Level` type is not possible currently
//        use logging_bindings::exports::wasi::logging::logging::Level;
//        let level = match level {
//            logging::Level::Trace => Level::Trace,
//            logging::Level::Debug => Level::Debug,
//            logging::Level::Info => Level::Info,
//            logging::Level::Warn => Level::Warn,
//            logging::Level::Error => Level::Error,
//            logging::Level::Critical => Level::Critical,
//        };
//        trace!("call `wasi:logging/logging.log`");
//        self.bindings
//            .wasi_logging_logging()
//            .call_log(&mut *store, level, &context, &message)
//            .await
//    }
//}
