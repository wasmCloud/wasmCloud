use super::host;

use serde::Serialize;

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
}

/// Emit a log message.
///
/// A log message has a `level` describing what kind of message is being sent, a context, which is an uninterpreted string meant to help consumers group similar messages, and a string containing the message text.
pub fn log(level: Level, cx: &str, msg: &str) {
    impl From<Level> for &'static str {
        fn from(level: Level) -> Self {
            match level {
                Level::Trace => "trace",
                Level::Debug => "debug",
                Level::Info => "info",
                Level::Warn => "warn",
                Level::Error => "error",
            }
        }
    }
    impl From<Level> for String {
        fn from(level: Level) -> Self {
            let level: &'static str = level.into();
            level.into()
        }
    }

    #[derive(Serialize)]
    struct LogEntry {
        #[serde(default)]
        pub level: String,
        /// message to log
        #[serde(default)]
        pub text: String,
    }

    let level = level.into();
    let text = if cx == "" {
        msg.into()
    } else {
        format!("context: {cx}; {msg}")
    };
    let pld = rmp_serde::to_vec(&LogEntry { level, text })
        .expect("failed to serialize `Logging.WriteLog` request");
    host::call(
        "",
        "wasmcloud:builtin:logging",
        "Logging.WriteLog",
        Some(&pld),
    )
    .expect("failed to call `Logging.WriteLog`");
}
