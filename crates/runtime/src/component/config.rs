use async_trait::async_trait;
use tracing::instrument;

use crate::capability::config::{self, runtime, store};

use super::{Ctx, Handler};

/// `wasi:config/store` implementation
#[async_trait]
pub trait Config {
    /// Handle `wasi:config/store.get`
    async fn get(&self, key: &str) -> anyhow::Result<Result<Option<String>, config::store::Error>>;

    /// Handle `wasi:config/store.get_all`
    async fn get_all(&self) -> anyhow::Result<Result<Vec<(String, String)>, config::store::Error>>;
}

#[async_trait]
impl<H: Handler> store::Host for Ctx<H> {
    #[instrument(skip(self))]
    async fn get(
        &mut self,
        key: String,
    ) -> anyhow::Result<Result<Option<String>, config::store::Error>> {
        self.attach_parent_context();
        Config::get(&self.handler, &key).await
    }

    #[instrument(skip_all)]
    async fn get_all(
        &mut self,
    ) -> anyhow::Result<Result<Vec<(String, String)>, config::store::Error>> {
        self.attach_parent_context();
        self.handler.get_all().await
    }
}

impl From<config::store::Error> for config::runtime::ConfigError {
    fn from(err: config::store::Error) -> Self {
        match err {
            store::Error::Upstream(err) => Self::Upstream(err),
            store::Error::Io(err) => Self::Io(err),
        }
    }
}

#[async_trait]
impl<H: Handler> runtime::Host for Ctx<H> {
    #[instrument(skip(self))]
    async fn get(
        &mut self,
        key: String,
    ) -> anyhow::Result<Result<Option<String>, config::runtime::ConfigError>> {
        self.attach_parent_context();
        let res = Config::get(&self.handler, &key).await?;
        Ok(res.map_err(Into::into))
    }

    #[instrument(skip_all)]
    async fn get_all(
        &mut self,
    ) -> anyhow::Result<Result<Vec<(String, String)>, config::runtime::ConfigError>> {
        self.attach_parent_context();
        let res = self.handler.get_all().await?;
        Ok(res.map_err(Into::into))
    }
}
