use super::{Ctx, Instance};

use crate::capability::config::{self, runtime};
use crate::capability::Config;

use std::sync::Arc;

use async_trait::async_trait;
use tracing::instrument;

impl Instance {
    /// Set [`Config`] handler for this [Instance].
    pub fn config(&mut self, config: Arc<dyn Config + Send + Sync>) -> &mut Self {
        self.handler_mut().replace_config(config);
        self
    }
}

#[async_trait]
impl runtime::Host for Ctx {
    #[instrument(skip(self))]
    async fn get(
        &mut self,
        key: String,
    ) -> anyhow::Result<Result<Option<String>, config::runtime::ConfigError>> {
        self.handler.get(&key).await
    }

    #[instrument(skip_all)]
    async fn get_all(
        &mut self,
    ) -> anyhow::Result<Result<Vec<(String, String)>, config::runtime::ConfigError>> {
        self.handler.get_all().await
    }
}
