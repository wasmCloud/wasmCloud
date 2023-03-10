#![cfg(feature = "log")]

use core::convert::Infallible;
use core::ops::Deref;

use log::{Level, Log, Record};

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
    fn log_text(&self, level: Level, text: impl AsRef<str>) {
        let text = text.as_ref();
        self.log(
            &Record::builder()
                .level(level)
                .args(format_args!("{text}"))
                .build(),
        );
    }
}

impl<T: Log> super::Logging for Logging<T> {
    type Error = Infallible;

    fn debug(&self, text: String) -> Result<(), Self::Error> {
        self.log_text(Level::Debug, text);
        Ok(())
    }

    fn info(&self, text: String) -> Result<(), Self::Error> {
        self.log_text(Level::Info, text);
        Ok(())
    }

    fn warn(&self, text: String) -> Result<(), Self::Error> {
        self.log_text(Level::Warn, text);
        Ok(())
    }

    fn error(&self, text: String) -> Result<(), Self::Error> {
        self.log_text(Level::Error, text);
        Ok(())
    }
}
