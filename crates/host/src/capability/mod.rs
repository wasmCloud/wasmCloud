#[allow(missing_docs)]
mod bindgen {
    wasmtime::component::bindgen!({
        world: "interfaces",
        async: true,
    });
}

pub use bindgen::wasi::logging::logging;
pub use bindgen::wasmcloud::bus::host;
pub use bindgen::Interfaces;

use logging::Host;
use rand::{thread_rng, Rng, RngCore};

use core::fmt::Debug;

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use futures::lock::Mutex;
use tracing::instrument;

fn format_opt<T>(opt: &Option<T>) -> &'static str {
    if opt.is_some() {
        "set"
    } else {
        "unset"
    }
}

#[derive(Clone, Default)]
pub(crate) struct Handler {
    host: Option<Arc<Mutex<dyn host::Host + Sync + Send>>>,
    logging: Option<Arc<Mutex<dyn logging::Host + Sync + Send>>>,
}

impl Debug for Handler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handler")
            .field("host", &format_opt(&self.host))
            .field("logging", &format_opt(&self.logging))
            .finish()
    }
}

impl Handler {
    /// Writes a log to configured [`log::Host`]
    #[instrument]
    async fn write_log(&mut self, level: &str, text: String) -> anyhow::Result<()> {
        let level = match level {
            // NOTE: Trace level is currently missing from wasmbus logging protocol
            "trace" => logging::Level::Trace,
            "debug" => logging::Level::Debug,
            "info" => logging::Level::Info,
            "warn" => logging::Level::Warn,
            "error" => logging::Level::Error,
            level => bail!("unsupported log level `{level}`"),
        };
        self.log(level, String::new(), text).await
    }

    /// Generates a UUIDv4
    #[instrument]
    async fn generate_guid(&self) -> anyhow::Result<uuid::Uuid> {
        let mut buf = uuid::Bytes::default();
        thread_rng()
            .try_fill_bytes(&mut buf)
            .context("failed to fill buffer")?;
        Ok(uuid::Builder::from_random_bytes(buf).into_uuid())
    }

    /// Generates a random [u32]
    #[instrument]
    async fn random_32(&self) -> u32 {
        thread_rng().next_u32()
    }

    /// Generates a random [u32] within inclusive range from `min` to `max`
    #[instrument]
    async fn random_in_range(&self, min: u32, max: u32) -> u32 {
        thread_rng().gen_range(min..=max)
    }
}

#[async_trait]
impl host::Host for Handler {
    #[instrument]
    async fn call(
        &mut self,
        binding: String,
        namespace: String,
        operation: String,
        payload: Option<Vec<u8>>,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, String>> {
        match (namespace.as_str(), operation.as_str()) {
            ("wasmcloud:builtin:logging", "Logging.WriteLog") => {
                let payload = payload.context("payload cannot be empty")?;
                let wasmcloud_interface_logging::LogEntry { level, text } =
                    rmp_serde::from_slice(payload.as_ref())
                        .context("failed to deserialize log entry")?;
                Ok(self
                    .write_log(&level, text)
                    .await
                    .map_err(|e| e.to_string())
                    .map(|()| None))
            }
            ("wasmcloud:builtin:numbergen", "NumberGen.GenerateGuid") => {
                match self.generate_guid().await {
                    Err(e) => Ok(Err(e.to_string())),
                    Ok(guid) => Ok(rmp_serde::to_vec(&guid.to_string())
                        .context("failed to serialize value")
                        .map_err(|e| e.to_string())
                        .map(Some)),
                }
            }
            ("wasmcloud:builtin:numbergen", "NumberGen.Random32") => {
                Ok(rmp_serde::to_vec(&self.random_32().await)
                    .context("failed to serialize value")
                    .map_err(|e| e.to_string())
                    .map(Some))
            }
            ("wasmcloud:builtin:numbergen", "NumberGen.RandomInRange") => {
                let payload = payload.context("payload cannot be empty")?;
                let wasmcloud_interface_numbergen::RangeLimit { min, max } =
                    rmp_serde::from_slice(&payload).context("failed to deserialize range limit")?;
                Ok(rmp_serde::to_vec(&self.random_in_range(min, max).await)
                    .context("failed to serialize value")
                    .map_err(|e| e.to_string())
                    .map(Some))
            }
            _ => {
                if let Some(ref host) = self.host {
                    host.lock()
                        .await
                        .call(binding, namespace, operation, payload)
                        .await
                } else {
                    Ok(Err(format!("host cannot handle `{namespace}.{operation}`")))
                }
            }
        }
    }
}

#[async_trait]
impl logging::Host for Handler {
    #[instrument]
    async fn log(
        &mut self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()> {
        if let Some(ref logging) = self.logging {
            logging.lock().await.log(level, context, message).await
        } else {
            // discard all log invocations by default
            Ok(())
        }
    }
}

/// A [Handler] builder used to configure it
#[derive(Clone, Default)]
pub(crate) struct HandlerBuilder {
    /// [`host::Host`] handler
    pub host: Option<Arc<Mutex<dyn host::Host + Sync + Send>>>,
    /// [`logging::Host`] handler
    pub logging: Option<Arc<Mutex<dyn logging::Host + Sync + Send>>>,
}

impl HandlerBuilder {
    /// Set [`host::Host`] handler
    pub fn host(self, host: impl host::Host + Sync + Send + 'static) -> Self {
        Self {
            host: Some(Arc::new(Mutex::new(host))),
            ..self
        }
    }

    /// Set [`logging::Host`] handler
    pub fn logging(self, logging: impl logging::Host + Sync + Send + 'static) -> Self {
        Self {
            logging: Some(Arc::new(Mutex::new(logging))),
            ..self
        }
    }

    /// Build a [`Handler`] from this [`HandlerBuilder`]
    pub fn build(self) -> Handler {
        self.into()
    }
}

impl Debug for HandlerBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandlerBuilder")
            .field("host", &format_opt(&self.host))
            .field("logging", &format_opt(&self.logging))
            .finish()
    }
}

impl From<Handler> for HandlerBuilder {
    fn from(Handler { host, logging }: Handler) -> Self {
        Self { host, logging }
    }
}

impl From<HandlerBuilder> for Handler {
    fn from(HandlerBuilder { host, logging }: HandlerBuilder) -> Self {
        Self { host, logging }
    }
}
