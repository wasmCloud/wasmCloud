use super::{Ctx, Handler};

use crate::capability::config::{self, runtime};

use async_trait::async_trait;
use tracing::instrument;

/// `wasi:config/runtime` implementation
#[async_trait]
pub trait Config {
    /// Handle `wasi:config/runtime.get`
    async fn get(
        &self,
        key: &str,
    ) -> anyhow::Result<Result<Option<String>, config::runtime::ConfigError>>;

    /// Handle `wasi:config/runtime.get_all`
    async fn get_all(
        &self,
    ) -> anyhow::Result<Result<Vec<(String, String)>, config::runtime::ConfigError>>;
}

#[async_trait]
impl<H: Handler> runtime::Host for Ctx<H> {
    #[instrument(skip(self))]
    async fn get(
        &mut self,
        key: String,
    ) -> anyhow::Result<Result<Option<String>, config::runtime::ConfigError>> {
        Config::get(&self.handler, &key).await
    }

    #[instrument(skip_all)]
    async fn get_all(
        &mut self,
    ) -> anyhow::Result<Result<Vec<(String, String)>, config::runtime::ConfigError>> {
        self.handler.get_all().await
    }
}
