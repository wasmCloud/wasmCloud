use crate::wasmcloud::bus::host;

use serde::Serialize;
use wasmcloud_compat::logging::LogEntry;

/// A log level, describing a kind of message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Level {
    // NOTE: Legacy implementation lacked Trace level
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    // NOTE: Legacy implementation lacked Critical level
    Critical,
}

impl From<Level> for &'static str {
    fn from(level: Level) -> Self {
        match level {
            Level::Trace => "trace",
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
            Level::Critical => "critical",
        }
    }
}
impl From<Level> for String {
    fn from(level: Level) -> Self {
        let level: &'static str = level.into();
        level.into()
    }
}

/// Emit a log message.
///
/// A log message has a `level` describing what kind of message is being sent, a context, which is an uninterpreted string meant to help consumers group similar messages, and a string containing the message text.
pub fn log(level: Level, context: &str, message: &str) {
    let level = level.into();
    let text = if context == "" {
        message.into()
    } else {
        format!("context: {context}; {message}")
    };
    let pld = rmp_serde::to_vec(&LogEntry { level, text })
        .expect("failed to serialize `Logging.WriteLog` request");
    host::call_sync(None, "wasmcloud:builtin:logging/Logging.WriteLog", &pld)
        .expect("failed to call `Logging.WriteLog`");
}
