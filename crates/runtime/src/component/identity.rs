use async_trait::async_trait;
use tracing::instrument;

use crate::capability::identity::{self, store};

use super::{Ctx, Handler};

/// `wasmcloud:identity/store` implementation
#[async_trait]
pub trait Identity {
    /// Handle `wasmcloud:identity/store.get`
    async fn get(
        &self,
        audience: &str,
    ) -> anyhow::Result<Result<Option<String>, identity::store::Error>>;
}

impl<H: Handler> store::Host for Ctx<H> {
    #[instrument(skip(self))]
    async fn get(
        &mut self,
        audience: String,
    ) -> anyhow::Result<Result<Option<String>, identity::store::Error>> {
        self.attach_parent_context();
        Identity::get(&self.handler, &audience).await
    }
}
