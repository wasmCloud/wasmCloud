use super::{Ctx, Handler};

use crate::capability::secrets::store::{HostSecret, Secret, SecretValue};
use crate::capability::secrets::{self, reveal, store};

use std::sync::Arc;

use async_trait::async_trait;
use tracing::instrument;
use wasmtime::component::Resource;

/// `wasmcloud:secrets` implementation
#[async_trait]
pub trait Secrets {
    /// Handle `wasmcloud:secrets/store.get`
    async fn get(
        &self,
        key: &str,
    ) -> anyhow::Result<Result<secrets::store::Secret, secrets::store::SecretsError>>;

    /// Handle `wasmcloud:secrets/reveal.reveal`
    async fn reveal(
        &self,
        secret: secrets::reveal::Secret,
    ) -> anyhow::Result<secrets::reveal::SecretValue>;
}

#[async_trait]
impl<H: Handler> HostSecret for Ctx<H> {
    async fn drop(&mut self, secret: Resource<Secret>) -> anyhow::Result<()> {
        self.table.delete(secret)?;
        Ok(())
    }
}

#[async_trait]
impl<H: Handler> store::Host for Ctx<H> {
    #[instrument(skip(self))]
    async fn get(
        &mut self,
        key: String,
    ) -> anyhow::Result<Result<Resource<Secret>, store::SecretsError>> {
        self.attach_parent_context();
        let secret = Secrets::get(&self.handler, &key).await?;
        if let Some(err) = secret.err() {
            Ok(Err(err))
        } else {
            let secret_resource = self.table.push(Arc::new(key))?;
            Ok(Ok(secret_resource))
        }
    }
}

#[async_trait]
impl<H: Handler> reveal::Host for Ctx<H> {
    #[instrument(skip(self))]
    async fn reveal(&mut self, secret: Resource<Secret>) -> anyhow::Result<SecretValue> {
        self.attach_parent_context();
        let key = self.table.get(&secret)?;
        let secret_value = self.handler.reveal(key.clone()).await?;
        Ok(secret_value)
    }
}
