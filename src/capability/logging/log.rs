use super::Invocation;

use crate::capability::Handle;

use core::ops::{Deref, DerefMut};

use anyhow::Result;
use async_trait::async_trait;
use log::{Level, Log, Record};
use tracing::instrument;
use wascap::jwt;

/// A logging capability wrapping an arbitrary [`log::Log`] implementation.
pub struct Logging<T = &'static dyn ::log::Log>(T);

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

impl<T> DerefMut for Logging<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Default for Logging {
    fn default() -> Self {
        Self(::log::logger())
    }
}

impl From<super::Level> for ::log::Level {
    fn from(level: super::Level) -> Self {
        match level {
            super::Level::Debug => Level::Debug,
            super::Level::Info => Level::Info,
            super::Level::Warn => Level::Warn,
            super::Level::Error => Level::Error,
        }
    }
}

#[async_trait]
impl<T: Log> Handle<Invocation> for Logging<T> {
    #[instrument(skip(self))]
    async fn handle(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        _binding: String,
        invocation: Invocation,
    ) -> Result<Option<Vec<u8>>> {
        match invocation {
            Invocation::WriteLog { level, text } => {
                self.log(
                    &Record::builder()
                        .level(level.into())
                        .target(&claims.subject)
                        .args(format_args!("{text}"))
                        .build(),
                );
                Ok(None)
            }
        }
    }
}
