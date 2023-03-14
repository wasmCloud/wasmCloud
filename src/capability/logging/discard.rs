use core::convert::Infallible;

use async_trait::async_trait;
use wascap::jwt;

/// A logging capability, which discards all logging statements
pub struct Logging;

#[async_trait]
impl super::Logging for Logging {
    type Error = Infallible;

    async fn debug(&self, _: &jwt::Claims<jwt::Actor>, _: String) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn info(&self, _: &jwt::Claims<jwt::Actor>, _: String) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn warn(&self, _: &jwt::Claims<jwt::Actor>, _: String) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn error(&self, _: &jwt::Claims<jwt::Actor>, _: String) -> Result<(), Self::Error> {
        Ok(())
    }
}
