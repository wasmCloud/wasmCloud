use super::{Ctx, Instance};

use crate::capability::secrets::store::{HostSecret, Secret, SecretValue};
use crate::capability::secrets::{reveal, store};
use crate::capability::Secrets;

use std::sync::Arc;

use async_trait::async_trait;
use tracing::instrument;
use wasmtime::component::Resource;

impl Instance {
    /// Set [`Secrets`] handler for this [Instance].
    pub fn secrets(&mut self, secrets: Arc<dyn Secrets + Send + Sync>) -> &mut Self {
        self.handler_mut().replace_secrets(secrets);
        self
    }
}

impl HostSecret for Ctx {
    fn drop(&mut self, secret: Resource<Secret>) -> std::result::Result<(), anyhow::Error> {
        self.table.delete(secret)?;
        Ok(())
    }
}

#[async_trait]
impl store::Host for Ctx {
    #[instrument(skip(self))]
    async fn get(
        &mut self,
        key: String,
    ) -> anyhow::Result<Result<Resource<Secret>, store::SecretsError>> {
        let secret = self.handler.get(&key).await?;
        if let Some(err) = secret.err() {
            Ok(Err(err))
        } else {
            let secret_resource = self.table.push(Arc::new(key))?;
            Ok(Ok(secret_resource))
        }
    }
}

#[async_trait]
impl reveal::Host for Ctx {
    #[instrument(skip(self))]
    async fn reveal(&mut self, secret: Resource<Secret>) -> anyhow::Result<SecretValue> {
        let key = self.table.get(&secret)?;
        let secret_value = self.handler.reveal(key.clone()).await?;
        Ok(secret_value)
    }
}
