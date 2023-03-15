use crate::capability::Handle;

use super::Invocation;

use anyhow::Result;
use async_trait::async_trait;
use wascap::jwt;

/// A logging capability, which discards all logging statements
pub struct Logging;

#[async_trait]
impl Handle<Invocation> for Logging {
    async fn handle(
        &self,
        _: &jwt::Claims<jwt::Actor>,
        _: String,
        _: Invocation,
    ) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
}
