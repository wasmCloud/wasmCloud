//! Reusable types related to links on wasmCloud lattices
//!
//! NOTE: In the future, generated types to enable easy interoperation with [wasi:logging][wasi-logging] should live here.
//!
//! [wasi-logging]: <https://github.com/WebAssembly/wasi-logging>

use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
    Critical,
}

impl FromStr for Level {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "error" => Ok(Self::Error),
            "warn" => Ok(Self::Warn),
            "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "trace" => Ok(Self::Trace),
            "critical" => Ok(Self::Critical),
            _ => Err(format!("unknown log level: {s}")),
        }
    }
}

impl From<tracing::Level> for Level {
    fn from(level: tracing::Level) -> Self {
        match level {
            tracing::Level::ERROR => Self::Error,
            tracing::Level::WARN => Self::Warn,
            tracing::Level::INFO => Self::Info,
            tracing::Level::DEBUG => Self::Debug,
            tracing::Level::TRACE => Self::Trace,
        }
    }
}

impl Default for Level {
    fn default() -> Self {
        Self::Info
    }
}
