use core::convert::Infallible;

use wascap::jwt;

/// A logging capability, which discards all logging statements
pub struct Logging;

impl super::Logging for Logging {
    type Error = Infallible;

    fn debug(&self, _: &jwt::Claims<jwt::Actor>, _: String) -> Result<(), Self::Error> {
        Ok(())
    }

    fn info(&self, _: &jwt::Claims<jwt::Actor>, _: String) -> Result<(), Self::Error> {
        Ok(())
    }

    fn warn(&self, _: &jwt::Claims<jwt::Actor>, _: String) -> Result<(), Self::Error> {
        Ok(())
    }

    fn error(&self, _: &jwt::Claims<jwt::Actor>, _: String) -> Result<(), Self::Error> {
        Ok(())
    }
}
