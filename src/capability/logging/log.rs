#![cfg(feature = "log")]

use core::convert::Infallible;
use core::ops::Deref;

use async_trait::async_trait;
use log::{Level, Log, Record};
use wascap::jwt;

/// A logging capability wrapping an arbitrary [`log::Log`] implementation.
pub struct Logging<T>(T);

impl<T: Log> From<T> for Logging<T> {
    fn from(l: T) -> Self {
        Self(l)
    }
}

impl<T> Deref for Logging<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: Log> Logging<T> {
    fn log_text(&self, level: Level, claims: &jwt::Claims<jwt::Actor>, text: impl AsRef<str>) {
        let text = text.as_ref();
        self.log(
            &Record::builder()
                .level(level)
                .target(&claims.subject)
                .args(format_args!("{text}"))
                .build(),
        );
    }
}

#[async_trait]
impl<T: Log> super::Logging for Logging<T> {
    type Error = Infallible;

    async fn debug(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        text: String,
    ) -> Result<(), Self::Error> {
        self.log_text(Level::Debug, claims, text);
        Ok(())
    }

    async fn info(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        text: String,
    ) -> Result<(), Self::Error> {
        self.log_text(Level::Info, claims, text);
        Ok(())
    }

    async fn warn(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        text: String,
    ) -> Result<(), Self::Error> {
        self.log_text(Level::Warn, claims, text);
        Ok(())
    }

    async fn error(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        text: String,
    ) -> Result<(), Self::Error> {
        self.log_text(Level::Error, claims, text);
        Ok(())
    }
}
